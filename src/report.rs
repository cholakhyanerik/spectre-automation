use std::fs::{self, File};
use std::path::Path;
use plotters::prelude::*;
use chrono::Local;
use serde::{Serialize, Deserialize};
// MetricPoint удален из импортов для исправления предупреждения (unused import)
use crate::monitor::TestResult; 

const DIR_CURRENT: &str = "reports/current";
const DIR_HISTORY: &str = "reports/history";
const HISTORY_FILE: &str = "reports/history/run_history.json";

#[derive(Serialize, Deserialize, Debug)]
pub struct RunHistoryRecord {
    pub date_time: String,
    pub executable: String,
    pub duration_secs: u64,
    pub avg_cpu: f32,
    pub max_cpu: f32,
    pub avg_gpu: f32,
    pub max_gpu: f32,
    pub avg_ram_mb: f64,
    pub max_ram_mb: u64,
}

pub fn init_reports_dir() {
    let current_path = Path::new(DIR_CURRENT);
    let history_path = Path::new(DIR_HISTORY);

    // 1. Очищаем папку current перед каждым запуском
    if current_path.exists() {
        let _ = fs::remove_dir_all(current_path);
    }
    fs::create_dir_all(current_path).expect("Не удалось создать директорию reports/current");
    
    // 2. Создаем history, если её нет (не очищаем, она хранит историю!)
    if !history_path.exists() {
        fs::create_dir_all(history_path).expect("Не удалось создать директорию reports/history");
    }
}

fn calculate_history_record(result: &TestResult) -> RunHistoryRecord {
    let count = result.metrics.len() as f32;
    let avg_cpu = if count > 0.0 { (result.metrics.iter().map(|p| p.cpu).sum::<f32>() / count).min(100.0) } else { 0.0 };
    let avg_gpu = if count > 0.0 { (result.metrics.iter().map(|p| p.gpu).sum::<f32>() / count).min(100.0) } else { 0.0 };
    let avg_ram = if count > 0.0 { result.metrics.iter().map(|p| p.ram_mb as f64).sum::<f64>() / count as f64 } else { 0.0 };

    let max_cpu = result.metrics.iter().map(|p| p.cpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_gpu = result.metrics.iter().map(|p| p.gpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_ram = result.metrics.iter().map(|p| p.ram_mb).max().unwrap_or(0);

    RunHistoryRecord {
        date_time: result.timestamp.clone(),
        executable: result.exe_name.clone(),
        duration_secs: result.duration_secs,
        avg_cpu,
        max_cpu,
        avg_gpu,
        max_gpu,
        avg_ram_mb: avg_ram,
        max_ram_mb: max_ram,
    }
}

pub fn save_run_to_history(result: &TestResult) {
    let record = calculate_history_record(result);
    let mut history: Vec<RunHistoryRecord> = vec![];

    // Читаем существующую историю, если файл есть
    if Path::new(HISTORY_FILE).exists() {
        if let Ok(content) = fs::read_to_string(HISTORY_FILE) {
            if let Ok(parsed) = serde_json::from_str(&content) {
                history = parsed;
            }
        }
    }

    history.push(record);

    // Перезаписываем обновленный файл истории
    if let Ok(file) = File::create(HISTORY_FILE) {
        let _ = serde_json::to_writer_pretty(file, &history);
        println!("📝 Запись добавлена в общую историю: {}", HISTORY_FILE);
    }
}

pub fn save_report_json(version_name: &str, result: &TestResult) {
    // Сохраняем в current без timestamp, чтобы имена были чистыми
    let filename = format!("{}/{}.json", DIR_CURRENT, version_name);
    let file = File::create(&filename).unwrap();
    serde_json::to_writer_pretty(file, result).expect("Не удалось записать JSON");
    println!("💾 Сырые данные сохранены в: {}", filename);
}

pub fn print_visual_report(title: &str, result: &TestResult) {
    let data = &result.metrics;
    if data.is_empty() { return; }
    
    let record = calculate_history_record(result);

    println!("\n┌─────────────────────────────────────────────────────────┐");
    println!("│ 📊 PERFORMANCE REPORT: {:<32} │", title.to_uppercase());
    println!("├─────────────────────────────────────────────────────────┤");
    println!("│ Исполняемый файл: {:<37} │", record.executable);
    println!("│ Дата запуска: {:<41} │", record.date_time);
    println!("│ Продолжительность: {:<2} сек                               │", record.duration_secs);
    println!("├───────────────────────┬────────────────┬────────────────┤");
    println!("│ РЕСУРСЫ               │ СРЕДНЕЕ        │ ПИКОВОЕ (MAX)  │");
    println!("├───────────────────────┼────────────────┼────────────────┤");
    println!("│ 🖥️  CPU Usage         │ {:>12.2}% │ {:>12.2}% │", record.avg_cpu, record.max_cpu);
    println!("│ 🎮 GPU Usage         │ {:>12.2}% │ {:>12.2}% │", record.avg_gpu, record.max_gpu);
    println!("│ 💾 RAM Allocation    │ {:>11.1} MB │ {:>11} MB │", record.avg_ram_mb, record.max_ram_mb);
    println!("└───────────────────────┴────────────────┴────────────────┘\n");
}

pub fn generate_single_chart(version_name: &str, result: &TestResult) {
    let filename = format!("{}/chart_{}.png", DIR_CURRENT, version_name);
    let data = &result.metrics;
    
    let root = BitMapBackend::new(&filename, (900, 750)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let record = calculate_history_record(result);
    let (upper, lower) = root.split_vertically(460);

    let mut chart = ChartBuilder::on(&upper)
        .caption(format!("Профиль: {} ({})", version_name.to_uppercase(), result.exe_name), ("sans-serif", 24).into_font())
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..data.len() + 1, 0.0f32..100.0f32) 
        .unwrap();

    chart.configure_mesh().x_desc("Время (сек)").y_desc("Нагрузка (%)").draw().unwrap();

    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap()
        .label("CPU (%)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap()
        .label("GPU (%)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));

    chart.configure_series_labels().background_style(WHITE.mix(0.8)).border_style(&BLACK).draw().unwrap();

    lower.draw(&Rectangle::new([(40, 10), (860, 250)], Into::<ShapeStyle>::into(&BLACK).stroke_width(2))).unwrap();
    let font_title = ("sans-serif", 18).into_font().style(FontStyle::Bold);
    let font_header = ("sans-serif", 14).into_font().style(FontStyle::Bold);
    let font_normal = ("sans-serif", 14).into_font();

    lower.draw(&Text::new("📊 SUMMARY REPORT", (60, 25), font_title.color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("Дата: {}", record.date_time), (500, 25), font_normal.clone().color(&BLACK))).unwrap();
    
    lower.draw(&Text::new("РЕСУРСЫ", (80, 70), font_header.color(&BLACK))).unwrap();
    lower.draw(&Text::new("СРЕДНЕЕ ЗНАЧЕНИЕ", (320, 70), font_header.color(&BLACK))).unwrap();
    lower.draw(&Text::new("ПИКОВОЕ (MAX)", (620, 70), font_header.color(&BLACK))).unwrap();
    
    lower.draw(&PathElement::new(vec![(50, 95), (850, 95)], &BLACK)).unwrap();

    lower.draw(&Text::new("🖥️  CPU Usage", (80, 115), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", record.avg_cpu), (320, 115), font_normal.clone().color(&RED))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", record.max_cpu), (620, 115), font_normal.clone().color(&RED))).unwrap();

    lower.draw(&Text::new("🎮  GPU Usage", (80, 155), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", record.avg_gpu), (320, 155), font_normal.clone().color(&BLUE))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", record.max_gpu), (620, 155), font_normal.clone().color(&BLUE))).unwrap();

    lower.draw(&Text::new("💾  RAM Allocation", (80, 195), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.1} MB", record.avg_ram_mb), (320, 195), font_normal.clone().color(&MAGENTA))).unwrap();
    lower.draw(&Text::new(format!("{} MB", record.max_ram_mb), (620, 195), font_normal.clone().color(&MAGENTA))).unwrap();

    root.present().unwrap(); // Принудительно завершаем отрисовку
    println!("📈 График сохранен в: {}", filename);
}

pub fn generate_comparison_chart(new_res: &TestResult, old_res: &TestResult) {
    let filename = format!("{}/comparison.png", DIR_CURRENT);
    
    let new_data = &new_res.metrics;
    let old_data = &old_res.metrics;

    let root = BitMapBackend::new(&filename, (1024, 850)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let max_len = new_data.len().max(old_data.len());
    let (upper, lower) = root.split_vertically(530);

    let mut chart = ChartBuilder::on(&upper)
        .caption("Сравнение версий", ("sans-serif", 28).into_font())
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..max_len + 1, 0.0f32..100.0f32)
        .unwrap();

    chart.configure_mesh().x_desc("Время (сек)").y_desc("Использование (%)").draw().unwrap();

    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap()
        .label("CPU (Актуальная)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.cpu)), &MAGENTA)).unwrap()
        .label("CPU (Старая)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], MAGENTA));
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap()
        .label("GPU (Актуальная)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.gpu)), &CYAN)).unwrap()
        .label("GPU (Старая)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], CYAN));

    chart.configure_series_labels().background_style(WHITE.mix(0.8)).border_style(&BLACK).draw().unwrap();

    let rec_new = calculate_history_record(new_res);
    let rec_old = calculate_history_record(old_res);

    let diff_cpu = rec_new.avg_cpu - rec_old.avg_cpu;
    let diff_gpu = rec_new.avg_gpu - rec_old.avg_gpu;
    let diff_ram = rec_new.avg_ram_mb - rec_old.avg_ram_mb;

    lower.draw(&Rectangle::new([(40, 10), (984, 280)], Into::<ShapeStyle>::into(&BLACK).stroke_width(2))).unwrap();
    
    let font_title = ("sans-serif", 18).into_font().style(FontStyle::Bold);
    let font_header = ("sans-serif", 14).into_font().style(FontStyle::Bold);
    let font_normal = ("sans-serif", 14).into_font();

    lower.draw(&Text::new("📊 COMPARISON SUMMARY REPORT", (60, 25), font_title.color(&BLACK))).unwrap();
    
    lower.draw(&Text::new("РЕСУРСЫ", (80, 70), font_header.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new("АКТУАЛЬНАЯ ВЕРСИЯ", (320, 70), font_header.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new("СТАРАЯ ВЕРСИЯ", (560, 70), font_header.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new("РАЗНИЦА (Δ)", (800, 70), font_header.clone().color(&BLACK))).unwrap();
    
    lower.draw(&PathElement::new(vec![(50, 95), (974, 95)], &BLACK)).unwrap();

    let draw_row = |y: i32, title: &str, new_val: f64, old_val: f64, diff_val: f64, unit: &str| {
        lower.draw(&Text::new(title, (80, y), font_normal.clone().color(&BLACK))).unwrap();
        lower.draw(&Text::new(format!("{:.2} {}", new_val, unit), (320, y), font_normal.clone().color(&BLACK))).unwrap();
        lower.draw(&Text::new(format!("{:.2} {}", old_val, unit), (560, y), font_normal.clone().color(&BLACK))).unwrap();

        let sign = if diff_val > 0.0 { "+" } else { "" };
        let diff_text = format!("{}{:.2} {}", sign, diff_val, unit);
        let color = if diff_val > 0.0 { &RED } else if diff_val < 0.0 { &GREEN } else { &BLACK };
        lower.draw(&Text::new(diff_text, (800, y), font_normal.clone().color(color))).unwrap();
    };

    draw_row(120, "🖥️  CPU Usage", rec_new.avg_cpu as f64, rec_old.avg_cpu as f64, diff_cpu as f64, "%");
    draw_row(170, "🎮  GPU Usage", rec_new.avg_gpu as f64, rec_old.avg_gpu as f64, diff_gpu as f64, "%");
    draw_row(220, "💾  RAM Allocation", rec_new.avg_ram_mb, rec_old.avg_ram_mb, diff_ram, "MB");

    root.present().unwrap();
    println!("👉 Сводный сравнительный график сохранен в: {}", filename);
}

pub fn print_comparison_report(new_res: &TestResult, old_res: &TestResult) {
    if new_res.metrics.is_empty() || old_res.metrics.is_empty() { return; }

    let rec_new = calculate_history_record(new_res);
    let rec_old = calculate_history_record(old_res);

    let diff_cpu = rec_new.avg_cpu - rec_old.avg_cpu;
    let diff_gpu = rec_new.avg_gpu - rec_old.avg_gpu;
    let diff_ram = rec_new.avg_ram_mb - rec_old.avg_ram_mb;

    let format_diff = |val: f64, unit: &str| -> String {
        let sign = if val > 0.0 { "+" } else { "" };
        let text = format!("[{}{:.1}{}]", sign, val, unit);
        let padded = format!("{:>12}", text); 
        
        if val > 0.0 { format!("\x1b[31m{}\x1b[0m", padded) } 
        else if val < 0.0 { format!("\x1b[32m{}\x1b[0m", padded) } 
        else { padded }
    };

    let diff_cpu_str = format_diff(diff_cpu as f64, "%");
    let diff_gpu_str = format_diff(diff_gpu as f64, "%");
    let diff_ram_str = format_diff(diff_ram, " MB");

    println!("\n┌───────────────────────┬────────────────────┬─────────────────┬──────────────┐");
    println!("│ 📊 COMPARISON REPORT: ACTUAL VS OLD                                         │");
    println!("├───────────────────────┬────────────────────┬─────────────────┬──────────────┤");
    println!("│ РЕСУРСЫ               │ АКТУАЛЬНАЯ ВЕРСИЯ  │ СТАРАЯ ВЕРСИЯ   │ РАЗНИЦА (Δ)  │");
    println!("├───────────────────────┼────────────────────┼─────────────────┼──────────────┤");
    println!("│ 🖥️  CPU Usage         │ {:>17.2}% │ {:>14.2}% │ {} │", rec_new.avg_cpu, rec_old.avg_cpu, diff_cpu_str);
    println!("│ 🎮 GPU Usage         │ {:>17.2}% │ {:>14.2}% │ {} │", rec_new.avg_gpu, rec_old.avg_gpu, diff_gpu_str);
    println!("│ 💾 RAM Allocation    │ {:>14.1} MB │ {:>11.1} MB │ {} │", rec_new.avg_ram_mb, rec_old.avg_ram_mb, diff_ram_str);
    println!("└───────────────────────┴────────────────────┴─────────────────┴──────────────┘\n");
}

/// Функция для архивации всех отчетов из current в history с добавлением метки времени
pub fn archive_current_run() {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let current_path = Path::new(DIR_CURRENT);
    let history_path = Path::new(DIR_HISTORY);

    if let Ok(entries) = fs::read_dir(current_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) {
                    let filename = format!("{}_{}.{}", stem.to_string_lossy(), timestamp, ext.to_string_lossy());
                    let target_path = history_path.join(filename);
                    let _ = fs::copy(&path, &target_path);
                }
            }
        }
    }
    println!("📂 Все отчёты текущего запуска скопированы в архив (reports/history/)");
}