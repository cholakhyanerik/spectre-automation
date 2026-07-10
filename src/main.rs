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
    runner::execute_ui_scenario();

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

    println!("\n🎉 Тестирование успешно завершено! Отчеты сохранены в 'reports/'");
    println!("====================================================");
}