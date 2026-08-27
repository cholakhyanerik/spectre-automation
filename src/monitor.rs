use std::collections::HashSet;
use tokio::time::{Duration, Instant, sleep, sleep_until};
use sysinfo::{Pid, System};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Local};
use crate::gpu::GpuMonitor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub second: usize,
    pub cpu: f32,
    pub ram_mb: u64,
    pub gpu: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub timestamp: String,
    pub exe_name: String,
    pub duration_secs: u64,
    /// Платформа, на которой проводился замер (Windows / Linux / macOS).
    #[serde(default)]
    pub platform: String,
    /// Имя обнаруженного GPU-адаптера (для контекста в отчётах).
    #[serde(default)]
    pub gpu_name: String,
    /// Каким по счёту шёл этот замер внутри одного запуска харнесса: 1 — первым,
    /// 2 — вторым. Величина не измеренная, а организационная, но без неё числа
    /// нельзя интерпретировать через месяцы: первая сборка платит за холодный
    /// кэш ФС, прогрев шейдеров и первичную инициализацию, вторая — нет.
    /// `0` означает «неизвестно» и стоит в записях, сделанных до появления поля.
    #[serde(default)]
    pub run_position: u8,
    pub metrics: Vec<MetricPoint>,
}

pub async fn start_monitoring(
    pid: u32,
    duration_secs: u64,
    exe_name: String,
    match_patterns: Vec<String>,
    run_position: u8,
) -> TestResult {
    let mut sys = System::new_all();
    let mut history = Vec::new();
    let main_pid = Pid::from(pid as usize);
    let start_time: DateTime<Local> = Local::now();

    // Полный набор подстрок имён, относящихся к приложению:
    // имя запущенного бинаря (ловит мультиоконный режим: spectre-terminal.exe (2))
    // + паттерны модалок/семейства из конфига (ловит "Spectre Settings" как отд. приложение).
    let mut patterns: Vec<String> = match_patterns;
    let exe_lc = exe_name.to_lowercase();
    if !exe_lc.is_empty() && !patterns.iter().any(|p| p == &exe_lc) {
        patterns.push(exe_lc);
    }
    
    // Инициализация монитора GPU (без повторных пересозданий)
    let gpu_monitor = GpuMonitor::new();
    println!("🎮 Обнаружен GPU: {}", gpu_monitor.name);

    // Первоначальное обновление для инициализации счетчиков (важно для точного CPU)
    sys.refresh_all();
    sleep(Duration::from_millis(200)).await;

    // Определяем количество логических ядер процессора
    let cpu_count = sys.cpus().len() as f32;
    let safe_cpu_count = if cpu_count > 0.0 { cpu_count } else { 1.0 };

    // Точка отсчёта для семплинга с фиксированным шагом в 1 сек.
    // Дедлайн n-го замера = loop_start + n секунд. Если итерация заняла дольше
    // секунды (на Windows опрос GPU через PowerShell бывает медленным),
    // sleep_until вернётся сразу — дрейф не накапливается, ось времени не «плывёт».
    let loop_start = Instant::now();

    for second in 1..=duration_secs {
        sys.refresh_all();

        // Агрегируем метрики по всем процессам приложения:
        // дерево потомков ∪ процессы, чьё имя совпало с паттернами (дедуп по PID).
        let (total_cpu, total_ram_bytes) = collect_app_metrics(&sys, main_pid, &patterns);

        // Приводим CPU к стандартным 0-100% и ограничиваем сверху.
        let normalized_cpu = (total_cpu / safe_cpu_count).min(100.0);

        // Переводим байты в Мегабайты
        let ram_mb = total_ram_bytes / 1_048_576;

        // Асинхронно получаем GPU без блокировки потока.
        let gpu_usage = gpu_monitor.get_gpu_usage().await.min(100.0);

        history.push(MetricPoint {
            second: second as usize,
            cpu: normalized_cpu,
            ram_mb,
            gpu: gpu_usage,
        });

        sleep_until(loop_start + Duration::from_secs(second)).await;
    }

    TestResult {
        timestamp: start_time.format("%Y-%m-%d %H:%M:%S").to_string(),
        exe_name,
        duration_secs,
        platform: crate::config::platform_name().to_string(),
        gpu_name: gpu_monitor.name.clone(),
        run_position,
        metrics: history,
    }
}

/// Возвращает множество PID, входящих в дерево процессов (главный + все потомки
/// любой глубины). Считается по родительским ссылкам методом фиксированной точки.
fn collect_subtree(sys: &System, main_pid: Pid) -> HashSet<Pid> {
    let mut tree: HashSet<Pid> = HashSet::new();
    tree.insert(main_pid);

    loop {
        let mut added = false;
        for (pid, process) in sys.processes() {
            if tree.contains(pid) {
                continue;
            }
            if let Some(parent) = process.parent()
                && tree.contains(&parent) {
                    tree.insert(*pid);
                    added = true;
                }
        }
        if !added {
            break;
        }
    }
    tree
}

/// true, если имя процесса содержит любой из паттернов (нижний регистр).
fn name_matches(name_lc: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| name_lc.contains(p.as_str()))
}

/// Агрегирует CPU и RAM по всем процессам приложения: объединение дерева
/// потомков и процессов, чьё имя совпало с паттернами. Каждый процесс
/// учитывается ровно один раз (итерация по уникальным PID).
fn collect_app_metrics(sys: &System, main_pid: Pid, patterns: &[String]) -> (f32, u64) {
    let subtree = collect_subtree(sys, main_pid);
    // Не учитываем сам процесс теста (его имя "spectre-automation" тоже ловится паттерном).
    let self_pid = Pid::from(std::process::id() as usize);

    let mut total_cpu = 0.0f32;
    let mut total_ram: u64 = 0;

    for (pid, process) in sys.processes() {
        if *pid == self_pid {
            continue;
        }
        let name_lc = process.name().to_string_lossy().to_lowercase();
        let belongs = subtree.contains(pid) || name_matches(&name_lc, patterns);
        if belongs {
            total_cpu += process.cpu_usage();
            total_ram += process.memory();
        }
    }

    (total_cpu, total_ram)
}

/// Сколько раз перепроверять, что процессы приложения действительно исчезли,
/// и сколько ждать перед каждой проверкой.
///
/// Пауза обязательна: между отправкой сигнала и исчезновением процесса из
/// списка проходит время, и проверка без паузы дала бы ложную тревогу на
/// каждом прогоне. Стоит она здесь дёшево — замер уже кончился, мерить нечего
/// (Правило 1 запрещает тратить машину ВО ВРЕМЯ замера, а не после него).
/// В обычном прогоне хватает первой проверки; полторы секунды набегают только
/// когда что-то и правда не умирает, то есть ровно тогда, когда есть о чём
/// говорить вслух.
const TERMINATE_CHECKS: usize = 5;
const TERMINATE_PAUSE: Duration = Duration::from_millis(300);

/// Процесс, намеченный к завершению: PID и имя в нижнем регистре.
///
/// Имя хранится не ради вывода. При перепроверке по нему отличают выжившее
/// окно приложения от постороннего процесса, которому ОС успела отдать
/// освободившийся PID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    pid: Pid,
    name: String,
}

/// Снимает с живого `System` пары «PID, имя в нижнем регистре».
///
/// Нужен, чтобы выбор целей (`select_targets`) не зависел от `&System` и
/// проверялся тестами: ошибиться там можно молча и в обе стороны — не убить
/// выжившего или убить постороннего.
fn name_snapshot(sys: &System) -> Vec<(Pid, String)> {
    sys.processes()
        .iter()
        .map(|(pid, process)| (*pid, process.name().to_string_lossy().to_lowercase()))
        .collect()
}

/// Кто из снимка процессов относится к приложению и подлежит завершению.
///
/// `known` — цели предыдущего прохода, и нужен он только при ПЕРЕПРОВЕРКЕ, зато
/// там незаменим. К этому моменту главный процесс уже мёртв, дерево потомков
/// пересчитать не по чему, и остаётся одно сопоставление имён — то есть
/// вспомогательное окно, чьё имя с паттернами не совпадает, перестало бы
/// считаться процессом приложения и не попало бы ни в добивание, ни в
/// предупреждение. Ровно тот молчаливый отказ, ради которого всё и затевалось.
///
/// Совпадение по `known` требует И PID, И имени: PID освобождается сразу и
/// достаётся первому попавшемуся новому процессу. Обвинить посторонний процесс
/// в том, что он «пережил завершение», — ложная тревога того же сорта, что и
/// молчание о настоящем выжившем, и обесценивает предупреждение так же быстро.
fn select_targets(
    snapshot: &[(Pid, String)],
    subtree: &HashSet<Pid>,
    patterns: &[String],
    self_pid: Pid,
    known: &[Target],
) -> Vec<Target> {
    snapshot
        .iter()
        // Критично: НЕ убиваем сам процесс теста (его имя тоже совпадает
        // с паттерном). Проверка стоит первой и без неё харнесс на некоторых
        // наборах MATCH_PROCESSES прибьёт себя на первом же прогоне.
        .filter(|(pid, _)| *pid != self_pid)
        .filter(|(pid, name)| {
            subtree.contains(pid)
                || name_matches(name, patterns)
                || known.iter().any(|t| t.pid == *pid && &t.name == name)
        })
        .map(|(pid, name)| Target { pid: *pid, name: name.clone() })
        .collect()
}

/// Шлёт сигнал завершения каждой цели и запоминает тех, кому его не удалось
/// даже отправить.
///
/// `kill()` отвечает на вопрос «сигнал отправлен?», а не «процесс умер?» — это
/// написано прямо в его доккомментарии в `sysinfo`. Поэтому здесь копится
/// только ПОДСКАЗКА к будущему предупреждению (отказ — обычно нет прав или
/// процесс системный), а вывод о результате делается по факту: по тому, кто
/// остался в системе.
fn kill_targets(sys: &System, targets: &[Target], refused: &mut HashSet<Pid>) {
    for target in targets {
        let Some(process) = sys.process(target.pid) else {
            continue;
        };
        // Помним ИСХОД ПОСЛЕДНЕЙ попытки: отказ на первом проходе и успех на
        // втором — это не «нет прав», и подсказка не должна утверждать обратное.
        if process.kill() {
            refused.remove(&target.pid);
        } else {
            refused.insert(target.pid);
        }
    }
}

/// Говорит вслух о процессах приложения, переживших завершение.
///
/// Молчать здесь нельзя: осиротевшее окно доживёт до следующего прогона и
/// попадёт в ЕГО метрики через совпадение имени — потребление одной сборки
/// прибавится к числам другой. В отчёте это выглядит обычной регрессией:
/// красный цвет, проценты, вердикт «хуже», — и отличить её там нечем.
fn warn_survivors(survivors: &[Target], refused: &HashSet<Pid>) {
    eprintln!(
        "⚠️  Завершить приложение удалось не полностью: в системе осталось процессов — {}.",
        survivors.len()
    );
    for target in survivors {
        let why = if refused.contains(&target.pid) {
            "сигнал завершения отклонён — нет прав или процесс системный"
        } else {
            "сигнал принят, но процесс не исчез"
        };
        eprintln!("    • PID {} — {} ({})", target.pid, target.name, why);
    }
    eprintln!("    Они доживут до следующего прогона и попадут в ЕГО метрики через");
    eprintln!("    совпадение имени: потребление этой сборки прибавится к чужой, и");
    eprintln!("    в отчёте это будет выглядеть обычной регрессией. Закройте их");
    eprintln!("    вручную (диспетчер задач, Get-Process, ps) перед следующим замером.");
}

/// Завершает все процессы приложения (главный, доп. окна и модалки-приложения),
/// чтобы осиротевшие окна не перетекали в следующий прогон и не искажали замер.
///
/// Это не одиночный выстрел, а цикл с проверкой ФАКТА. `kill()` возвращает
/// `bool`, и раньше он отбрасывался целиком — а значит, выживший процесс не
/// оставлял ни одного следа: прогон заканчивался успехом, и его потребление
/// всплывало в следующем замере под именем чужой сборки. Проверять при этом
/// надо не код возврата (он говорит лишь об отправке сигнала), а то, кто
/// остался в системе, — это Правило 6.
///
/// Готового `kill_and_wait()` в `sysinfo` мы не берём намеренно: его
/// доккомментарий обещает БЕСКОНЕЧНЫЙ ЦИКЛ на процессе, который не убивается.
/// Подвесить харнесс насмерть после трёхминутного замера — хуже, чем выжившее
/// окно, о котором сказано вслух.
pub fn terminate_app(main_pid: u32, patterns: &[String]) {
    let mut sys = System::new_all();
    sys.refresh_all();

    let main_pid = Pid::from(main_pid as usize);
    let self_pid = Pid::from(std::process::id() as usize);

    // Дерево потомков считается ОДИН раз, на первом проходе, и дальше живёт
    // в списке целей. Пересчитывать его на каждой проверке нельзя: главный
    // процесс к этому моменту уже мёртв (`kill` + `wait` сделаны вызывающей
    // стороной), его PID свободен, и достаться он может постороннему — тогда
    // «дерево потомков» указало бы на чужие процессы, и мы убили бы их.
    let subtree = collect_subtree(&sys, main_pid);
    let no_subtree = HashSet::new();

    let mut targets = select_targets(&name_snapshot(&sys), &subtree, patterns, self_pid, &[]);
    let mut refused: HashSet<Pid> = HashSet::new();
    kill_targets(&sys, &targets, &mut refused);

    for check in 1..=TERMINATE_CHECKS {
        std::thread::sleep(TERMINATE_PAUSE);
        // `refresh_all` выкидывает из списка мёртвые процессы — именно поэтому
        // «остался в списке» здесь означает «жив», а не «когда-то был».
        sys.refresh_all();

        targets = select_targets(
            &name_snapshot(&sys),
            &no_subtree,
            patterns,
            self_pid,
            &targets,
        );
        if targets.is_empty() {
            return;
        }
        // На последней проверке бить уже поздно: перепроверить результат нечем,
        // и предупреждение говорило бы о процессах, которым не дали ни секунды
        // на смерть, — то есть врало бы ровно так же, как молчание.
        if check < TERMINATE_CHECKS {
            kill_targets(&sys, &targets, &mut refused);
        }
    }

    warn_survivors(&targets, &refused);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Сопоставление идёт по ВХОЖДЕНИЮ подстроки, и на этом держится учёт
    /// мультиоконного режима: «spectre-terminal.exe (2)» — то же приложение.
    /// Перестань оно ловиться — потребление дополнительных окон молча выпадет
    /// из замера, и сборка покажется легче, чем она есть.
    #[test]
    fn name_matches_by_substring() {
        let p = patterns(&["spectre-terminal"]);
        assert!(name_matches("spectre-terminal.exe", &p));
        assert!(name_matches("spectre-terminal.exe (2)", &p));
        assert!(!name_matches("notepad.exe", &p));
    }

    /// Сопоставление РЕГИСТРОЗАВИСИМО, и это контракт с вызывающими: имя
    /// процесса к нижнему регистру приводят `collect_app_metrics` и
    /// `terminate_app`, паттерны — `Config::load`. Уберут приведение с любой
    /// стороны — совпадений не будет вовсе, а выглядеть это будет как
    /// «приложение ничего не потребляло».
    #[test]
    fn name_matches_relies_on_caller_lowercasing() {
        let p = patterns(&["spectre-terminal"]);
        assert!(!name_matches("Spectre-Terminal.exe", &p));
        assert!(name_matches(&"Spectre-Terminal.exe".to_lowercase(), &p));
    }

    /// Пустой СПИСОК паттернов не совпадает ни с чем, а вот пустая СТРОКА
    /// в списке совпадает со всем: `contains("")` истинно для любого имени.
    /// По этим же паттернам процессы в конце прогона убиваются, так что один
    /// пустой паттерн — это «выкосить всё, до чего дотянемся». Отсеивается он
    /// в конфиге (`default_match_patterns` и разбор MATCH_PROCESSES), а не здесь.
    #[test]
    fn empty_pattern_matches_everything() {
        assert!(!name_matches("spectre-terminal.exe", &[]));
        assert!(name_matches("совершенно-посторонний-процесс", &patterns(&[""])));
    }

    /// Старые `actual.json` обязаны читаться и после появления новых полей:
    /// `TestResult` — публичный формат, и архив в `reports/history` копится
    /// месяцами. Пропади `#[serde(default)]` у `run_position` — разбор упал бы
    /// на КАЖДОЙ записи, сделанной до этой версии.
    #[test]
    fn test_result_without_run_position_still_parses() {
        let json = r#"{
            "timestamp": "2026-07-09 18:13:49",
            "exe_name": "future-optimization.exe",
            "duration_secs": 360,
            "platform": "Windows",
            "gpu_name": "Quadro P2200",
            "metrics": [{ "second": 1, "cpu": 0.5, "ram_mb": 230, "gpu": 7.0 }]
        }"#;

        let parsed: TestResult =
            serde_json::from_str(json).expect("старый actual.json перестал читаться");
        assert_eq!(parsed.run_position, 0, "неизвестная очередь обязана быть нулём, а не выдумкой");
        assert_eq!(parsed.metrics.len(), 1);
    }

    /// Почему дефолтный паттерн — полное имя бинаря, а не префикс семейства
    /// (доккомментарий `config::default_match_patterns`): короткая подстрока
    /// ловит чужие процессы, и они не просто попадут в метрики — их убьют.
    #[test]
    fn short_prefix_catches_strangers() {
        assert!(name_matches("devenv.exe", &patterns(&["dev"])));
    }

    fn snapshot(items: &[(usize, &str)]) -> Vec<(Pid, String)> {
        items.iter().map(|(pid, name)| (Pid::from(*pid), name.to_string())).collect()
    }

    fn tree(pids: &[usize]) -> HashSet<Pid> {
        pids.iter().map(|pid| Pid::from(*pid)).collect()
    }

    fn pids_of(targets: &[Target]) -> Vec<usize> {
        targets.iter().map(|t| usize::from(t.pid)).collect()
    }

    /// Самая дорогая ошибка этого файла: харнесс убивает сам себя посреди
    /// прогона. Проверяется худший случай — собственный процесс и по имени
    /// совпал (`MATCH_PROCESSES` задаёт человек, одной подстроки хватит),
    /// и в дерево потомков попал, и числится среди целей прошлого прохода.
    /// Ни один из трёх путей не должен его пропустить.
    #[test]
    fn own_process_is_never_targeted() {
        let self_pid = Pid::from(100);
        let known = vec![Target { pid: self_pid, name: "spectre-automation.exe".to_string() }];
        let snap = snapshot(&[(100, "spectre-automation.exe"), (200, "spectre-terminal.exe")]);

        let targets =
            select_targets(&snap, &tree(&[100, 200]), &patterns(&["spectre"]), self_pid, &known);

        assert_eq!(pids_of(&targets), vec![200], "харнесс попал в список на убийство");
    }

    /// Ради чего у выбора целей вообще появился параметр `known`.
    ///
    /// На перепроверке главного процесса уже нет, дерево потомков пересчитать
    /// не по чему, и вспомогательный процесс приложения, чьё имя с паттернами
    /// не совпадает, опознаётся ТОЛЬКО по списку прошлого прохода. Потеряйся
    /// он — выживший не попал бы ни в добивание, ни в предупреждение, и его
    /// потребление уехало бы в следующий замер под чужим именем.
    #[test]
    fn recheck_keeps_orphan_outside_the_name_patterns() {
        let self_pid = Pid::from(1);
        let p = patterns(&["spectre-terminal"]);

        let first = snapshot(&[(200, "spectre-terminal.exe"), (300, "crashpad_handler.exe")]);
        let targets = select_targets(&first, &tree(&[200, 300]), &p, self_pid, &[]);
        assert_eq!(pids_of(&targets), vec![200, 300], "первый проход потерял потомка");

        let second = snapshot(&[(300, "crashpad_handler.exe")]);
        let survivors = select_targets(&second, &HashSet::new(), &p, self_pid, &targets);
        assert_eq!(
            pids_of(&survivors),
            vec![300],
            "выживший вне паттернов имён потерялся при перепроверке"
        );
    }

    /// Обратная ошибка, такая же молчаливая: PID освобождается сразу, и ОС
    /// отдаёт его первому попавшемуся процессу. Объявить того выжившим — значит
    /// и убить постороннего, и напечатать предупреждение на ровном месте;
    /// предупреждение, которое врёт, перестают читать первым.
    #[test]
    fn recycled_pid_is_not_mistaken_for_a_survivor() {
        let self_pid = Pid::from(1);
        let known = vec![Target { pid: Pid::from(300), name: "crashpad_handler.exe".to_string() }];
        let snap = snapshot(&[(300, "notepad.exe")]);

        let survivors = select_targets(
            &snap,
            &HashSet::new(),
            &patterns(&["spectre-terminal"]),
            self_pid,
            &known,
        );

        assert!(
            survivors.is_empty(),
            "чужой процесс с переиспользованным PID объявлен выжившим: {survivors:?}"
        );
    }

    /// Окно, открывшееся уже после первого прохода (приложение доспавнивает их
    /// и в момент завершения), обязано попасть в добивание по имени: иначе
    /// осиротеет именно оно.
    #[test]
    fn window_opened_after_the_first_pass_is_still_targeted() {
        let self_pid = Pid::from(1);
        let known = vec![Target { pid: Pid::from(200), name: "spectre-terminal.exe".to_string() }];
        let snap = snapshot(&[(400, "spectre-terminal.exe (2)")]);

        let late = select_targets(
            &snap,
            &HashSet::new(),
            &patterns(&["spectre-terminal"]),
            self_pid,
            &known,
        );

        assert_eq!(pids_of(&late), vec![400], "позднее окно не попало в добивание");
    }
}