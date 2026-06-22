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

    let exe_name_new = extract_filename(&cfg.app_path_new);

    // --- ТЕСТИРУЕМ АКТУАЛЬНУЮ ВЕРСИЮ ---
    println!("\n[Шаг 1] Запуск АКТУАЛЬНОЙ версии ({})...", exe_name_new);
    let mut process_new = runner::run_app(&cfg.app_path_new);
    let pid_new = process_new.id();

    // Мониторим
    let monitor_handle_new = tokio::spawn(monitor::start_monitoring(
        pid_new,
        cfg.test_duration_secs,
        exe_name_new,
        cfg.match_processes.clone(),
    ));
    runner::execute_ui_scenario();

    let result_new = monitor_handle_new.await.unwrap();
    // Завершаем главный процесс и все окна/модалки приложения
    let _ = process_new.kill();
    monitor::terminate_app(pid_new, &cfg.match_processes);
    println!("✅ Тест актуальной версии завершен.");

    // Сохраняем и выводим отчеты
    report::print_visual_report("Actual Version", &result_new);
    report::save_report_json("actual", &result_new);
    report::save_run_to_history(&result_new); 

    // --- ПРОВЕРЯЕМ СТАРУЮ ВЕРСИЮ ---
    if let Some(app_path_old) = cfg.app_path_old {
        let exe_name_old = extract_filename(&app_path_old);
        println!("\n[Шаг 2] Обнаружена СТАРАЯ версия ({}). Запуск для сравнения...", exe_name_old);
        
        let mut process_old = runner::run_app(&app_path_old);
        let pid_old = process_old.id();

        let monitor_handle_old = tokio::spawn(monitor::start_monitoring(
            pid_old,
            cfg.test_duration_secs,
            exe_name_old,
            cfg.match_processes.clone(),
        ));
        runner::execute_ui_scenario();

        let result_old = monitor_handle_old.await.unwrap();
        let _ = process_old.kill();
        monitor::terminate_app(pid_old, &cfg.match_processes);
        println!("✅ Тест старой версии завершен.");

        report::print_visual_report("Old Version", &result_old);
        report::save_report_json("old", &result_old);
        report::save_run_to_history(&result_old);

        // Сравнительные отчеты
        report::print_comparison_report(&result_new, &result_old);
        report::generate_comparison_chart(&result_new, &result_old);
    } else {
        println!("\n[Инфо] Старая версия не указана. Генерируем одиночный график...");
        report::generate_single_chart("actual", &result_new);
    }

    // --- АРХИВАЦИЯ РЕЗУЛЬТАТОВ ---
    // Копируем все файлы текущего прогона в history/ с временными метками
    report::archive_current_run();

    println!("\n🎉 Тестирование успешно завершено! Отчеты сохранены в 'reports/'");
    println!("====================================================");
}