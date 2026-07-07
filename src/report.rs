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

/// Результат построения графика: либо успех, либо любая ошибка рисования/ввода-вывода.
/// Позволяет не ронять весь прогон паникой, если PNG заблокирован (открыт в просмотрщике).
type ChartResult = Result<(), Box<dyn std::error::Error>>;

// Палитра отчётов (мягкие современные тона)
const C_CPU: RGBColor = RGBColor(229, 57, 53); // красный
const C_GPU: RGBColor = RGBColor(30, 136, 229); // синий
const C_RAM: RGBColor = RGBColor(142, 36, 170); // фиолетовый
const C_CPU_OLD: RGBColor = RGBColor(255, 152, 0); // оранжевый
const C_GPU_OLD: RGBColor = RGBColor(0, 172, 193); // бирюзовый
const C_GRID_BG: RGBColor = RGBColor(248, 249, 250); // почти белый фон графика
const C_INK: RGBColor = RGBColor(33, 37, 41); // тёмный текст
const C_MUTED: RGBColor = RGBColor(120, 124, 128); // приглушённый текст

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
    if Path::new(HISTORY_FILE).exists()
        && let Ok(content) = fs::read_to_string(HISTORY_FILE)
        && let Ok(parsed) = serde_json::from_str(&content)
    {
        history = parsed;
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
    let file = match File::create(&filename) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("⚠️  Не удалось создать {}: {}", filename, e);
            return;
        }
    };
    if let Err(e) = serde_json::to_writer_pretty(file, result) {
        eprintln!("⚠️  Не удалось записать JSON {}: {}", filename, e);
        return;
    }
    println!("💾 Сырые данные сохранены в: {}", filename);
}

pub fn print_visual_report(title: &str, result: &TestResult) {
    let data = &result.metrics;
    if data.is_empty() { return; }
    
    let record = calculate_history_record(result);

    let platform = if result.platform.is_empty() { "—" } else { result.platform.as_str() };
    let gpu_name = if result.gpu_name.is_empty() { "—" } else { result.gpu_name.as_str() };

    println!("\n┌─────────────────────────────────────────────────────────┐");
    println!("│ 📊 PERFORMANCE REPORT: {:<32} │", title.to_uppercase());
    println!("├─────────────────────────────────────────────────────────┤");
    println!("│ Исполняемый файл: {:<37} │", record.executable);
    println!("│ Платформа: {:<44} │", platform);
    println!("│ GPU: {:<50} │", gpu_name);
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

pub fn generate_single_chart(version_name: &str, result: &TestResult) -> ChartResult {
    let filename = format!("{}/chart_{}.png", DIR_CURRENT, version_name);
    let data = &result.metrics;

    let root = BitMapBackend::new(&filename, (900, 750)).into_drawing_area();
    root.fill(&WHITE)?;

    let record = calculate_history_record(result);
    let (upper, lower) = root.split_vertically(460);

    let mut chart = ChartBuilder::on(&upper)
        .caption(
            format!("Профиль: {} ({})", version_name.to_uppercase(), result.exe_name),
            ("sans-serif", 24).into_font().style(FontStyle::Bold).color(&C_INK),
        )
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(45)
        .build_cartesian_2d(0..data.len() + 1, 0.0f32..100.0f32)?;

    // Лёгкий фон области построения
    chart.plotting_area().fill(&C_GRID_BG)?;

    chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Нагрузка (%)")
        .axis_desc_style(("sans-serif", 15).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 12).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(RGBColor(225, 228, 232))
        .draw()?;

    // Заливка области под линиями + сами линии (CPU, GPU)
    chart.draw_series(AreaSeries::new(
        data.iter().map(|p| (p.second, p.cpu)),
        0.0,
        C_CPU.mix(0.12),
    ))?;
    chart
        .draw_series(LineSeries::new(
            data.iter().map(|p| (p.second, p.cpu)),
            C_CPU.stroke_width(3),
        ))?
        .label("CPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU.stroke_width(3)));

    chart.draw_series(AreaSeries::new(
        data.iter().map(|p| (p.second, p.gpu)),
        0.0,
        C_GPU.mix(0.12),
    ))?;
    chart
        .draw_series(LineSeries::new(
            data.iter().map(|p| (p.second, p.gpu)),
            C_GPU.stroke_width(3),
        ))?
        .label("GPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU.stroke_width(3)));

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(RGBColor(225, 228, 232))
        .label_font(("sans-serif", 13).into_font().color(&C_INK))
        .draw()?;

    lower.draw(&Rectangle::new([(40, 10), (860, 280)], Into::<ShapeStyle>::into(C_GRID_BG).filled()))?;
    lower.draw(&Rectangle::new([(40, 10), (860, 280)], Into::<ShapeStyle>::into(RGBColor(225, 228, 232)).stroke_width(2)))?;
    let font_title = ("sans-serif", 18).into_font().style(FontStyle::Bold);
    let font_header = ("sans-serif", 14).into_font().style(FontStyle::Bold);
    let font_normal = ("sans-serif", 14).into_font();

    lower.draw(&Text::new("📊 SUMMARY REPORT", (60, 25), font_title.color(&C_INK)))?;
    lower.draw(&Text::new(format!("Дата: {}", record.date_time), (560, 25), font_normal.clone().color(&C_MUTED)))?;

    // Контекст окружения
    let platform = if result.platform.is_empty() { "—" } else { result.platform.as_str() };
    let gpu_name = if result.gpu_name.is_empty() { "—" } else { result.gpu_name.as_str() };
    lower.draw(&Text::new(format!("Платформа: {}    GPU: {}", platform, gpu_name), (60, 55), font_normal.clone().color(&C_MUTED)))?;

    lower.draw(&Text::new("РЕСУРСЫ", (80, 95), font_header.color(&C_INK)))?;
    lower.draw(&Text::new("СРЕДНЕЕ ЗНАЧЕНИЕ", (320, 95), font_header.color(&C_INK)))?;
    lower.draw(&Text::new("ПИКОВОЕ (MAX)", (620, 95), font_header.color(&C_INK)))?;

    lower.draw(&PathElement::new(vec![(50, 118), (850, 118)], RGBColor(225, 228, 232)))?;

    lower.draw(&Text::new("🖥️  CPU Usage", (80, 138), font_normal.clone().color(&C_INK)))?;
    lower.draw(&Text::new(format!("{:.2} %", record.avg_cpu), (320, 138), font_normal.clone().color(&C_CPU)))?;
    lower.draw(&Text::new(format!("{:.2} %", record.max_cpu), (620, 138), font_normal.clone().color(&C_CPU)))?;

    lower.draw(&Text::new("🎮  GPU Usage", (80, 178), font_normal.clone().color(&C_INK)))?;
    lower.draw(&Text::new(format!("{:.2} %", record.avg_gpu), (320, 178), font_normal.clone().color(&C_GPU)))?;
    lower.draw(&Text::new(format!("{:.2} %", record.max_gpu), (620, 178), font_normal.clone().color(&C_GPU)))?;

    lower.draw(&Text::new("💾  RAM Allocation", (80, 218), font_normal.clone().color(&C_INK)))?;
    lower.draw(&Text::new(format!("{:.1} MB", record.avg_ram_mb), (320, 218), font_normal.clone().color(&C_RAM)))?;
    lower.draw(&Text::new(format!("{} MB", record.max_ram_mb), (620, 218), font_normal.clone().color(&C_RAM)))?;

    root.present()?; // Принудительно завершаем отрисовку
    println!("📈 График сохранен в: {}", filename);
    Ok(())
}

pub fn generate_comparison_chart(new_res: &TestResult, old_res: &TestResult) -> ChartResult {
    let filename = format!("{}/comparison.png", DIR_CURRENT);

    let new_data = &new_res.metrics;
    let old_data = &old_res.metrics;

    let root = BitMapBackend::new(&filename, (1024, 850)).into_drawing_area();
    root.fill(&WHITE)?;

    let max_len = new_data.len().max(old_data.len());
    let (upper, lower) = root.split_vertically(530);

    let mut chart = ChartBuilder::on(&upper)
        .caption("Сравнение версий", ("sans-serif", 28).into_font().style(FontStyle::Bold).color(&C_INK))
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(45)
        .build_cartesian_2d(0..max_len + 1, 0.0f32..100.0f32)?;

    chart.plotting_area().fill(&C_GRID_BG)?;

    chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Использование (%)")
        .axis_desc_style(("sans-serif", 15).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 12).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(RGBColor(225, 228, 232))
        .draw()?;

    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.cpu)), C_CPU.stroke_width(3)))?
        .label("CPU (Актуальная)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU.stroke_width(3)));
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.cpu)), C_CPU_OLD.stroke_width(2)))?
        .label("CPU (Старая)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU_OLD.stroke_width(2)));
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.gpu)), C_GPU.stroke_width(3)))?
        .label("GPU (Актуальная)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU.stroke_width(3)));
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.gpu)), C_GPU_OLD.stroke_width(2)))?
        .label("GPU (Старая)").legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU_OLD.stroke_width(2)));

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(RGBColor(225, 228, 232))
        .label_font(("sans-serif", 13).into_font().color(&C_INK))
        .draw()?;

    let rec_new = calculate_history_record(new_res);
    let rec_old = calculate_history_record(old_res);

    let diff_cpu = rec_new.avg_cpu - rec_old.avg_cpu;
    let diff_gpu = rec_new.avg_gpu - rec_old.avg_gpu;
    let diff_ram = rec_new.avg_ram_mb - rec_old.avg_ram_mb;

    lower.draw(&Rectangle::new([(40, 10), (984, 280)], Into::<ShapeStyle>::into(C_GRID_BG).filled()))?;
    lower.draw(&Rectangle::new([(40, 10), (984, 280)], Into::<ShapeStyle>::into(RGBColor(225, 228, 232)).stroke_width(2)))?;

    let font_title = ("sans-serif", 18).into_font().style(FontStyle::Bold);
    let font_header = ("sans-serif", 14).into_font().style(FontStyle::Bold);
    let font_normal = ("sans-serif", 14).into_font();

    lower.draw(&Text::new("📊 COMPARISON SUMMARY REPORT", (60, 25), font_title.color(&C_INK)))?;

    // Контекст: платформа и GPU актуальной версии
    let platform = if new_res.platform.is_empty() { "—" } else { new_res.platform.as_str() };
    let gpu_name = if new_res.gpu_name.is_empty() { "—" } else { new_res.gpu_name.as_str() };
    lower.draw(&Text::new(format!("Платформа: {}    GPU: {}", platform, gpu_name), (520, 25), font_normal.clone().color(&C_MUTED)))?;

    lower.draw(&Text::new("РЕСУРСЫ", (80, 70), font_header.clone().color(&C_INK)))?;
    lower.draw(&Text::new("АКТУАЛЬНАЯ ВЕРСИЯ", (320, 70), font_header.clone().color(&C_INK)))?;
    lower.draw(&Text::new("СТАРАЯ ВЕРСИЯ", (560, 70), font_header.clone().color(&C_INK)))?;
    lower.draw(&Text::new("РАЗНИЦА (Δ)", (800, 70), font_header.clone().color(&C_INK)))?;

    lower.draw(&PathElement::new(vec![(50, 95), (974, 95)], RGBColor(225, 228, 232)))?;

    let draw_row = |y: i32, title: &str, new_val: f64, old_val: f64, diff_val: f64, unit: &str| -> ChartResult {
        lower.draw(&Text::new(title, (80, y), font_normal.clone().color(&C_INK)))?;
        lower.draw(&Text::new(format!("{:.2} {}", new_val, unit), (320, y), font_normal.clone().color(&C_INK)))?;
        lower.draw(&Text::new(format!("{:.2} {}", old_val, unit), (560, y), font_normal.clone().color(&C_INK)))?;

        let sign = if diff_val > 0.0 { "+" } else { "" };
        let diff_text = format!("{}{:.2} {}", sign, diff_val, unit);
        let color = if diff_val > 0.0 { &RED } else if diff_val < 0.0 { &GREEN } else { &C_INK };
        lower.draw(&Text::new(diff_text, (800, y), font_normal.clone().color(color)))?;
        Ok(())
    };

    draw_row(120, "🖥️  CPU Usage", rec_new.avg_cpu as f64, rec_old.avg_cpu as f64, diff_cpu as f64, "%")?;
    draw_row(170, "🎮  GPU Usage", rec_new.avg_gpu as f64, rec_old.avg_gpu as f64, diff_gpu as f64, "%")?;
    draw_row(220, "💾  RAM Allocation", rec_new.avg_ram_mb, rec_old.avg_ram_mb, diff_ram, "MB")?;

    root.present()?;
    println!("👉 Сводный сравнительный график сохранен в: {}", filename);
    Ok(())
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
            if path.is_file()
                && let (Some(stem), Some(ext)) = (path.file_stem(), path.extension())
            {
                let filename = format!("{}_{}.{}", stem.to_string_lossy(), timestamp, ext.to_string_lossy());
                let target_path = history_path.join(filename);
                let _ = fs::copy(&path, &target_path);
            }
        }
    }
    println!("📂 Все отчёты текущего запуска скопированы в архив (reports/history/)");
}