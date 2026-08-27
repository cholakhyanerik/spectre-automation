mod config;
mod gpu;
mod monitor;
mod report;
mod runner;

use config::{Config, RunOrder};
use std::path::Path;

fn check_terminal_state(db_path: &Path) -> bool {
    if db_path.exists() {
        println!("💾 Найдена существующая база exchanges.db: {}", db_path.display());
        println!("🔥 ТЕСТ: Замер производительности при восстановлении сохраненного стейта.");
        true
    } else {
        println!(
            "ℹ️  Файл exchanges.db не найден ({}). Тест начнется с чистого экрана (стейт пуст).",
            db_path.display()
        );
        false
    }
}

// Вспомогательная функция для извлечения имени файла из пути
fn extract_filename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown.exe".to_string())
}

/// Говорит вслух, что прогон короче выхода приложения на режим.
///
/// Отказываться мерить нельзя: маленькая длительность осмысленна, когда
/// проверяют сам харнесс. Но и промолчать нельзя — статистика посчитается и
/// напечатается как обычно (медиана, p95, вердикт «хуже / лучше»), только
/// относиться она будет к разгону приложения. Это худший исход из возможных
/// здесь: число есть, оно правдоподобно, и оно про другое.
fn warn_short_run(duration_secs: u64, settle_secs: u64) {
    if !config::is_short_run(duration_secs, settle_secs) {
        return;
    }
    eprintln!("⚠️  Замер короче выхода на режим: {} сек при пороге {} сек.", duration_secs, settle_secs);
    eprintln!("    Приложение всё это время ещё разгоняется, поэтому медиана, p95 и пик");
    eprintln!("    опишут ЗАПУСК, а не потребление, и сравнивать по ним сборки нельзя.");
    eprintln!("    Для сравнения задайте TEST_DURATION_SECS заведомо больше выхода на режим");
    eprintln!("    (от минуты, на практике — три). Порог меняется через SETTLE_SECS в .env,");
    eprintln!("    ноль его отключает.");
}

/// То же предупреждение одной строкой, рядом с готовыми числами.
///
/// Прогон печатает много, и к моменту, когда человек смотрит на таблицу,
/// предупреждение из начала уже уехало из окна терминала. А смотрят именно
/// на числа — оговорка обязана лежать там же, где они.
fn note_short_run(duration_secs: u64, settle_secs: u64) {
    if config::is_short_run(duration_secs, settle_secs) {
        eprintln!(
            "⚠️  Числа этого прогона сняты за {} сек — это ЗАПУСК приложения, а не \
             установившееся потребление (порог SETTLE_SECS = {} сек).",
            duration_secs, settle_secs
        );
    }
}

/// Какая сборка меряется на этом шаге прогона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// Актуальная, `APP_PATH_NEW`. Её провал — единственный, что роняет прогон.
    New,
    /// Старая, `APP_PATH_OLD`. Её провал — предупреждение, а не ошибка.
    Old,
}

/// Составляет очередь замеров на этот запуск харнесса.
///
/// Вынесено в чистую функцию, потому что ошибиться здесь можно молча и дорого.
/// Выпади отсюда актуальная сборка — прогон завершился бы «успешно», не измерив
/// ничего; переставься очередь не в ту сторону — числа поменялись бы местами
/// в пользу другой сборки, и отличить это по отчёту было бы нечем: колонки
/// в сравнительной таблице задаются не очередью, а `APP_PATH_NEW` / `_OLD`,
/// и стоят на своих местах при любом порядке замера.
///
/// Без старой сборки очередь ни на что не влияет: мерить всё равно нечего,
/// кроме актуальной, и `RUN_ORDER` в этом случае просто не при чём.
fn run_sequence(order: RunOrder, has_old: bool) -> Vec<Which> {
    if !has_old {
        return vec![Which::New];
    }
    match order {
        RunOrder::NewFirst => vec![Which::New, Which::Old],
        RunOrder::OldFirst => vec![Which::Old, Which::New],
    }
}

/// Говорит вслух, в какой очереди пойдут замеры и чем эта очередь стоит.
///
/// Порядок влияет на числа, а из отчёта он не виден ниоткуда, кроме этой
/// строки и поля `run_position` в JSON: первая сборка платит за холодный кэш
/// ФС и прогрев, вторая получает их даром.
fn announce_run_order(order: RunOrder, has_old: bool) {
    if !has_old {
        // Сравнивать не с чем. Но если человек ЗАДАЛ порядок и не задал старую
        // сборку, промолчать нельзя: он ждёт перестановки, а её не будет.
        if order != config::DEFAULT_RUN_ORDER {
            println!(
                "ℹ️  RUN_ORDER={} задан, но APP_PATH_OLD нет — переставлять нечего, меряем только актуальную.",
                order.as_str()
            );
        }
        return;
    }

    let (first, second) = match order {
        RunOrder::NewFirst => ("АКТУАЛЬНАЯ", "СТАРАЯ"),
        RunOrder::OldFirst => ("СТАРАЯ", "АКТУАЛЬНАЯ"),
    };
    println!("🔀 Очередь замера (RUN_ORDER={}): 1) {} → 2) {}.", order.as_str(), first, second);
    println!("   Первая сборка платит за холодный кэш ФС и прогрев, вторая получает их даром:");
    println!("   сильнее всего это видно на пике CPU и на первых секундах графика.");
}

/// Выполняет синхронный шаг прогона так, чтобы он НЕ занимал воркер Tokio.
///
/// Сценарий ввода спит блокирующим `std::thread::sleep` суммарно около 4,5 секунд
/// и переписать его на `tokio::time::sleep` нельзя: `enigo` синхронный. Прямой
/// вызов из async-функции занимал бы воркер рантайма целиком, и сходило бы это
/// с рук только по везению — потому что `#[tokio::main]` по умолчанию даёт
/// многопоточный рантайм и задача мониторинга уезжает на другой воркер. Переход
/// на `flavor = "current_thread"` (одна строка, выглядящая упрощением) превратил
/// бы это в 4,5 секунды без единого замера в начале КАЖДОГО прогона: ни ошибки
/// сборки, ни подсказки clippy, ни падения — график просто начался бы с пятой
/// секунды, а разгон приложения, самое интересное место замера, не попал бы в
/// отчёт вовсе. `spawn_blocking` уводит шаг в отдельный пул потоков и снимает
/// зависимость замера от устройства рантайма.
///
/// Тем же путём уходит завершение приложения: оно ждёт смерти процессов и
/// потому тоже блокирующее. Мерить в тот момент уже нечего, но образец,
/// который скопирует следующий рефакторинг, важнее сиюминутной безвредности.
///
/// Паника внутри шага не роняет прогон: сценарий важен, но замер важнее.
async fn run_blocking_step<F>(name: &str, step: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(e) = tokio::task::spawn_blocking(step).await {
        eprintln!("⚠️  Шаг «{}» не отработал ({}). Прогон продолжается.", name, e);
    }
}

/// Запускает приложение, снимает метрики за `duration_secs`, затем гарантированно
/// завершает и «пожинает» (`wait`) процесс со всем деревом окон/модалок.
///
/// Возвращает `None`, если приложение не удалось запустить или мониторинг упал —
/// вызывающая сторона решает, критично это (актуальная версия) или можно
/// пропустить (старая версия для сравнения). Никаких паник и утечек процессов:
/// очистка выполняется в любом случае.
async fn run_single_test(
    step: &str,
    label: &str,
    app_path: &str,
    duration_secs: u64,
    match_processes: Vec<String>,
    run_position: u8,
) -> Option<monitor::TestResult> {
    let exe_name = extract_filename(app_path);
    println!("\n{} Запуск {} ({})...", step, label, exe_name);

    let mut child = match runner::run_app(app_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ Не удалось запустить приложение '{}': {}", app_path, e);
            return None;
        }
    };
    let pid = child.id();

    let monitor_handle = tokio::spawn(monitor::start_monitoring(
        pid,
        duration_secs,
        exe_name,
        match_processes.clone(),
        run_position,
    ));
    // Сценарий блокирующий — уводим его с воркера рантайма, иначе замер
    // начинается только после него (см. `run_blocking_step`).
    run_blocking_step("Сценарий UI", runner::execute_ui_scenario).await;

    let result = match monitor_handle.await {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("❌ Задача мониторинга завершилась с ошибкой: {}", e);
            None
        }
    };

    // Очистка выполняется ВСЕГДA, даже если мониторинг упал: сначала гасим
    // сам процесс и «пожинаем» его (иначе остаётся зомби), затем добиваем
    // дерево окон/модалок, чтобы осиротевшие окна не искажали следующий прогон.
    let _ = child.kill();
    let _ = child.wait();
    // Добивание ждёт смерти процессов и проверяет ФАКТ, а не код возврата
    // (Правило 6), то есть спит блокирующим sleep до полутора секунд. Мерить
    // в этот момент уже нечего, но занимать воркер рантайма всё равно незачем:
    // уводим шаг в отдельный пул тем же способом, что и сценарий ввода.
    run_blocking_step("Завершение приложения", move || {
        monitor::terminate_app(pid, &match_processes)
    })
    .await;

    result
}

#[tokio::main]
async fn main() {
    println!("====================================================");
    println!("🚀 Запуск Spectre-Automation: Сбор метрик десктоп GUI");
    println!("====================================================");

    // 1. Создаем папку current и history (current при этом будет очищена!)
    report::init_reports_dir();

    // 2. Загружаем конфигурацию
    let cfg = Config::load();
    println!("🖥️  Платформа: {}", config::platform_name());
    println!("ℹ️  Настроенная длительность тестов: {} сек", cfg.test_duration_secs);
    warn_short_run(cfg.test_duration_secs, cfg.settle_secs);

    // 3. Проверяем состояние БД
    check_terminal_state(&cfg.db_path);

    // --- ЗАМЕРЫ В ЗАДАННОЙ ОЧЕРЕДИ ---
    // Очередь решает `RUN_ORDER`, а не порядок операторов здесь: первая сборка
    // платит за холодный кэш ФС и прогрев, и это систематическая фора второй.
    let has_old = cfg.app_path_old.is_some();
    let sequence = run_sequence(cfg.run_order, has_old);
    let total = sequence.len();
    announce_run_order(cfg.run_order, has_old);

    let mut result_new: Option<monitor::TestResult> = None;
    let mut result_old: Option<monitor::TestResult> = None;

    for (i, which) in sequence.iter().enumerate() {
        // Позиция замера уезжает в TestResult и дальше в архив: без неё через
        // месяцы не отличить сборку, мерившуюся на холодной машине, от второй.
        let position = (i + 1) as u8;
        let step = format!("[Шаг {}/{}]", position, total);

        match which {
            Which::New => {
                let Some(result) = run_single_test(
                    &step,
                    "АКТУАЛЬНОЙ версии",
                    &cfg.app_path_new,
                    cfg.test_duration_secs,
                    cfg.match_processes.clone(),
                    position,
                )
                .await
                else {
                    // Единственный провал, который роняет прогон, — и он роняет
                    // его в любой очереди, а не только когда идёт первым.
                    eprintln!("❌ Не удалось протестировать актуальную версию. Завершение работы.");
                    // При RUN_ORDER=old-first старая сборка к этому моменту уже
                    // измерена, и запись в истории для неё уже сделана. Уйти
                    // отсюда молча — значит выбросить готовый замер из-за чужого
                    // провала и оставить в истории строку без копии в архиве.
                    if result_old.is_some() {
                        report::archive_current_run();
                    }
                    return;
                };
                println!("✅ Тест актуальной версии завершен.");

                report::print_visual_report(&result);
                report::save_report_json("actual", &result);
                report::save_run_to_history(&result);
                result_new = Some(result);
            }
            Which::Old => {
                // `run_sequence` ставит сюда шаг только когда путь задан;
                // проверка оставлена, чтобы рассинхрон не превратился в панику.
                let Some(app_path_old) = cfg.app_path_old.as_deref() else {
                    continue;
                };
                if let Some(result) = run_single_test(
                    &step,
                    "СТАРОЙ версии",
                    app_path_old,
                    cfg.test_duration_secs,
                    cfg.match_processes.clone(),
                    position,
                )
                .await
                {
                    println!("✅ Тест старой версии завершен.");

                    report::print_visual_report(&result);
                    report::save_report_json("old", &result);
                    report::save_run_to_history(&result);
                    result_old = Some(result);
                }
            }
        }
    }

    // --- ОТЧЁТЫ ---
    if !has_old {
        println!("\n[Инфо] Старая версия не указана. Генерируем одиночный график...");
    } else if result_old.is_none() {
        eprintln!("⚠️  Старую версию протестировать не удалось — строим одиночный график актуальной.");
    }

    let compared = if let (Some(new), Some(old)) = (&result_new, &result_old) {
        report::print_comparison_report(new, old);
        if let Err(e) = report::generate_comparison_chart(new, old) {
            eprintln!("⚠️  Не удалось построить сравнительный график: {}", e);
        }
        true
    } else {
        false
    };

    if !compared
        && let Some(new) = &result_new
        && let Err(e) = report::generate_single_chart("actual", new)
    {
        eprintln!("⚠️  Не удалось построить график: {}", e);
    }

    // --- АРХИВАЦИЯ РЕЗУЛЬТАТОВ ---
    // Копируем все файлы текущего прогона в history/ с временными метками
    report::archive_current_run();

    note_short_run(cfg.test_duration_secs, cfg.settle_secs);

    println!("\n🎉 Тестирование успешно завершено! Отчеты сохранены в 'reports/'");
    println!("====================================================");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Очередь переставляется целиком и в обе стороны. Ошибка здесь не видна
    /// ниоткуда: в сравнительной таблице колонки задаются `APP_PATH_NEW` /
    /// `_OLD`, а не очередью, — то есть при перепутанных ветках отчёт выглядел
    /// бы ровно так же, а фору холодного старта получила бы другая сборка.
    #[test]
    fn run_order_swaps_the_whole_sequence() {
        assert_eq!(run_sequence(RunOrder::NewFirst, true), [Which::New, Which::Old]);
        assert_eq!(run_sequence(RunOrder::OldFirst, true), [Which::Old, Which::New]);
    }

    /// Без старой сборки очередь ни на что не влияет, но актуальная обязана
    /// остаться в любом случае. Выпади она — прогон завершился бы «успешно»,
    /// не измерив ничего и не сказав ни слова.
    #[test]
    fn actual_build_is_measured_in_every_sequence() {
        for order in [RunOrder::NewFirst, RunOrder::OldFirst] {
            assert_eq!(run_sequence(order, false), [Which::New], "порядок {order:?} без старой сборки");

            let with_old = run_sequence(order, true);
            assert_eq!(
                with_old.iter().filter(|w| **w == Which::New).count(),
                1,
                "актуальная сборка мерится не один раз при порядке {order:?}: {with_old:?}"
            );
            assert_eq!(
                with_old.iter().filter(|w| **w == Which::Old).count(),
                1,
                "старая сборка мерится не один раз при порядке {order:?}: {with_old:?}"
            );
        }
    }

    /// Регрессия на дефект «блокирующий sleep внутри async»: синхронный шаг
    /// прогона не должен занимать воркер рантайма.
    ///
    /// Проверяется на ОДНОПОТОЧНОМ рантайме намеренно: на многопоточном (каким
    /// его делает `#[tokio::main]` по умолчанию) прямой вызов проходит незаметно —
    /// фоновая задача просто уезжает на свободный воркер, и тест был бы зелёным
    /// при сломанном коде. Здесь воркер один, и если шаг выполняется прямо
    /// в async-контексте, фоновая задача — в бою это мониторинг — не получает
    /// ни одного опроса, пока шаг спит.
    #[test]
    fn blocking_step_lets_background_task_run() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("не удалось собрать однопоточный рантайм");

        let ticked = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ticked);

        rt.block_on(async move {
            let background = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                flag.store(true, Ordering::SeqCst);
            });

            run_blocking_step("тестовый шаг", || {
                std::thread::sleep(Duration::from_millis(500))
            })
            .await;

            assert!(
                ticked.load(Ordering::SeqCst),
                "фоновая задача не отработала за 500 мс: синхронный шаг занял воркер рантайма"
            );
            background.await.expect("фоновая задача не должна падать");
        });
    }
}