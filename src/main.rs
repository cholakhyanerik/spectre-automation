mod config;
mod gpu;
mod monitor;
mod report;
mod runner;

use config::Config;
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
/// Паника внутри шага не роняет прогон: сценарий важен, но замер важнее.
async fn run_blocking_step<F>(name: &str, step: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(e) = tokio::task::spawn_blocking(step).await {
        eprintln!("⚠️  {} не отработал ({}). Замер продолжается.", name, e);
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
    monitor::terminate_app(pid, &match_processes);

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

    // --- ТЕСТИРУЕМ АКТУАЛЬНУЮ ВЕРСИЮ ---
    let result_new = match run_single_test(
        "[Шаг 1]",
        "АКТУАЛЬНОЙ версии",
        &cfg.app_path_new,
        cfg.test_duration_secs,
        cfg.match_processes.clone(),
    )
    .await
    {
        Some(r) => r,
        None => {
            eprintln!("❌ Не удалось протестировать актуальную версию. Завершение работы.");
            return;
        }
    };
    println!("✅ Тест актуальной версии завершен.");

    // Сохраняем и выводим отчеты
    report::print_visual_report(&result_new);
    report::save_report_json("actual", &result_new);
    report::save_run_to_history(&result_new);

    // --- ПРОВЕРЯЕМ СТАРУЮ ВЕРСИЮ ---
    let mut compared = false;
    if let Some(app_path_old) = cfg.app_path_old {
        if let Some(result_old) = run_single_test(
            "[Шаг 2] Обнаружена",
            "СТАРОЙ версии",
            &app_path_old,
            cfg.test_duration_secs,
            cfg.match_processes.clone(),
        )
        .await
        {
            println!("✅ Тест старой версии завершен.");

            report::print_visual_report(&result_old);
            report::save_report_json("old", &result_old);
            report::save_run_to_history(&result_old);

            // Сравнительные отчеты
            report::print_comparison_report(&result_new, &result_old);
            if let Err(e) = report::generate_comparison_chart(&result_new, &result_old) {
                eprintln!("⚠️  Не удалось построить сравнительный график: {}", e);
            }
            compared = true;
        } else {
            eprintln!("⚠️  Старую версию протестировать не удалось — строим одиночный график актуальной.");
        }
    } else {
        println!("\n[Инфо] Старая версия не указана. Генерируем одиночный график...");
    }

    if !compared
        && let Err(e) = report::generate_single_chart("actual", &result_new)
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