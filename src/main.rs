mod config;
mod gpu;
mod monitor;
mod report;
mod runner;

use config::Config;
use std::path::Path;

fn check_terminal_state() -> bool {
    let db_path = "C:\\Users\\chola\\AppData\\Local\\spectre-terminal\\exchanges.db";
    
    if Path::new(db_path).exists() {
        println!("💾 Найдена существующая база exchanges.db!");
        println!("🔥 ТЕСТ: Замер производительности при восстановлении сохраненного стейта (стаканы, кластеры, подписки).");
        true
    } else {
        println!("ℹ️  Файл exchanges.db не найден. Тест начнется с чистого экрана (стейт пуст).");
        false
    }
}

#[tokio::main]
async fn main() {
    println!("====================================================");
    println!("🚀 Запуск Spectre-Automation: Сбор метрик десктоп GUI");
    println!("====================================================");

    // 1. Создаем или очищаем папку /reports
    report::init_reports_dir();

    // 2. Загружаем конфигурацию из .env
    let cfg = Config::load();
    println!("ℹ️  Настроенная длительность тестов: {} сек", cfg.test_duration_secs);

    // 3. Проверяем состояние базы данных перед запуском
    check_terminal_state();

    // --- ТЕСТИРУЕМ АКТУАЛЬНУЮ ВЕРСИЮ ---
    println!("\n[Шаг 1] Запуск АКТУАЛЬНОЙ версии spectre-terminal...");
    let mut process_new = runner::run_app(&cfg.app_path_new);
    let pid_new = process_new.id();

    // Мониторим на основе динамической длительности из конфига
    let monitor_handle_new = tokio::spawn(monitor::start_monitoring(pid_new, cfg.test_duration_secs));
    
    // Эмуляция кликов пользователя
    runner::execute_ui_scenario();

    let metrics_new = monitor_handle_new.await.unwrap();
    let _ = process_new.kill(); // Закрываем терминал после теста
    println!("✅ Тест актуальной версии завершен.");

    // Генерируем красивую визуальную таблицу для актуальной версии
    report::print_visual_report("Actual Version", &metrics_new);

    // Сохраняем отчеты в папку /reports
    report::save_report_json("actual_version", &metrics_new);

    // --- ПРОВЕРЯЕМ СТАРУЮ ВЕРСИЮ (Опционально) ---
    if let Some(app_path_old) = cfg.app_path_old {
        println!("\n[Шаг 2] Обнаружена СТАРАЯ версия. Запуск для сравнения стейтов...");
        
        let mut process_old = runner::run_app(&app_path_old);
        let pid_old = process_old.id();

        // Также используем динамическую длительность для старой версии
        let monitor_handle_old = tokio::spawn(monitor::start_monitoring(pid_old, cfg.test_duration_secs));
        
        runner::execute_ui_scenario();

        let metrics_old = monitor_handle_old.await.unwrap();
        let _ = process_old.kill();
        println!("✅ Тест старой версии завершен.");

        // Генерируем красивую визуальную таблицу для старой версии
        report::print_visual_report("Old Version", &metrics_old);

        // 🚀 ВЫЗОВ НОВОЙ ТАБЛИЦЫ СРАВНЕНИЯ В КОНСОЛЬ
        report::print_comparison_report(&metrics_new, &metrics_old);

        report::save_report_json("old_version", &metrics_old);
        report::generate_comparison_chart(&metrics_new, &metrics_old);
    } else {
        println!("\n[Инфо] Старая версия не указана. Генерируем одиночный график...");
        report::generate_single_chart("actual_version", &metrics_new);
    }

    println!("\n🎉 Тестирование успешно завершено! Отчеты сохранены в 'reports/'");
    println!("====================================================");
}