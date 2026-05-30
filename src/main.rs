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

    // 3. Проверяем состояние базы данных перед запуском
    check_terminal_state();

    // --- ТЕСТИРУЕМ АКТУАЛЬНУЮ ВЕРСИЮ ---
    println!("\n[Шаг 1] Запуск АКТУАЛЬНОЙ версии spectre-terminal...");
    let mut process_new = runner::run_app(&cfg.app_path_new);
    let pid_new = process_new.id();

    // Мониторим 15 секунд (сбор логов каждую секунду)
    let monitor_handle_new = tokio::spawn(monitor::start_monitoring(pid_new, 15));
    
    // Эмуляция кликов пользователя
    runner::execute_ui_scenario();

    let metrics_new = monitor_handle_new.await.unwrap();
    let _ = process_new.kill(); // Закрываем терминал после теста
    println!("✅ Тест актуальной версии завершен.");

    // --- РАСЧЕТ И ВЫВОД СРЕДНИХ ЧИСЕЛ В КОНСОЛЬ ---
    if !metrics_new.is_empty() {
        let count = metrics_new.len() as f32;
        let avg_cpu: f32 = metrics_new.iter().map(|p| p.cpu).sum::<f32>() / count;
        let avg_gpu: f32 = metrics_new.iter().map(|p| p.gpu).sum::<f32>() / count;
        let avg_ram: f64 = metrics_new.iter().map(|p| p.ram_mb as f64).sum::<f64>() / count as f64;

        println!("\n📊 ===============================================");
        println!("🖥️  Средняя нагрузка CPU: {:.2}%", avg_cpu);
        println!("🎮 Средняя нагрузка GPU: {:.2}%", avg_gpu);
        println!("💾 Среднее потребление RAM: {:.1} MB", avg_ram);
        println!("===================================================\n");
    }

    // Сохраняем отчеты в папку /reports
    report::save_report_json("actual_version", &metrics_new);

    // --- ПРОВЕРЯЕМ СТАРУЮ ВЕРСИЮ (Опционально) ---
    if let Some(app_path_old) = cfg.app_path_old {
        println!("\n[Шаг 2] Обнаружена СТАРАЯ версия. Запуск для сравнения стейтов...");
        
        let mut process_old = runner::run_app(&app_path_old);
        let pid_old = process_old.id();

        let monitor_handle_old = tokio::spawn(monitor::start_monitoring(pid_old, 15));
        
        runner::execute_ui_scenario();

        let metrics_old = monitor_handle_old.await.unwrap();
        let _ = process_old.kill();
        println!("✅ Тест старой версии завершен.");

        // Вывод средних чисел для старой версии (для быстрого сравнения в консоли)
        if !metrics_old.is_empty() {
            let count = metrics_old.len() as f32;
            let avg_cpu: f32 = metrics_old.iter().map(|p| p.cpu).sum::<f32>() / count;
            let avg_gpu: f32 = metrics_old.iter().map(|p| p.gpu).sum::<f32>() / count;
            let avg_ram: f64 = metrics_old.iter().map(|p| p.ram_mb as f64).sum::<f64>() / count as f64;

            println!("\n📊 ===============================================");
            println!("🖥️  [Старая] Средняя нагрузка CPU: {:.2}%", avg_cpu);
            println!("🎮 [Старая] Средняя нагрузка GPU: {:.2}%", avg_gpu);
            println!("💾 [Старая] Среднее потребление RAM: {:.1} MB", avg_ram);
            println!("===================================================\n");
        }

        report::save_report_json("old_version", &metrics_old);
        report::generate_comparison_chart(&metrics_new, &metrics_old);
    } else {
        println!("\n[Инфо] Старая версия не указана. Генерируем одиночный график...");
        report::generate_single_chart("actual_version", &metrics_new);
    }

    println!("\n🎉 Тестирование успешно завершено! Отчеты сохранены в 'reports/'");
    println!("====================================================");
}