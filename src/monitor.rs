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

/// Завершает все процессы приложения (главный, доп. окна и модалки-приложения),
/// чтобы осиротевшие окна не перетекали в следующий прогон и не искажали замер.
pub fn terminate_app(main_pid: u32, patterns: &[String]) {
    let mut sys = System::new_all();
    sys.refresh_all();

    let main_pid = Pid::from(main_pid as usize);
    let subtree = collect_subtree(&sys, main_pid);
    // Критично: НЕ убиваем сам процесс теста (его имя тоже совпадает с паттерном).
    let self_pid = Pid::from(std::process::id() as usize);

    for (pid, process) in sys.processes() {
        if *pid == self_pid {
            continue;
        }
        let name_lc = process.name().to_string_lossy().to_lowercase();
        if subtree.contains(pid) || name_matches(&name_lc, patterns) {
            process.kill();
        }
    }
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
}