use std::fs::{self, File};
use std::path::Path;
use plotters::prelude::*;
use crate::monitor::MetricPoint;

const REPORTS_DIR: &str = "reports";

pub fn init_reports_dir() {
    let path = Path::new(REPORTS_DIR);
    if path.exists() {
        let _ = fs::remove_dir_all(path);
    }
    fs::create_dir_all(path).expect("Не удалось создать директорию reports");
}

pub fn save_report_json(version_name: &str, data: &[MetricPoint]) {
    let filename = format!("{}/report_{}.json", REPORTS_DIR, version_name);
    let file = File::create(&filename).unwrap();
    serde_json::to_writer_pretty(file, data).expect("Не удалось записать JSON");
    println!("💾 Данные бенчмарка сохранены в: {}", filename);
}

pub fn print_visual_report(title: &str, data: &[MetricPoint]) {
    if data.is_empty() {
        return;
    }
    let count = data.len() as f32;
    let avg_cpu = (data.iter().map(|p| p.cpu).sum::<f32>() / count).min(100.0);
    let avg_gpu = (data.iter().map(|p| p.gpu).sum::<f32>() / count).min(100.0);
    let avg_ram = data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / count as f64;

    let max_cpu = data.iter().map(|p| p.cpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_gpu = data.iter().map(|p| p.gpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_ram = data.iter().map(|p| p.ram_mb).max().unwrap_or(0);

    println!("\n┌─────────────────────────────────────────────────────────┐");
    println!("│ 📊 PERFORMANCE REPORT: {:<32} │", title.to_uppercase());
    println!("├─────────────────────────────────────────────────────────┤");
    println!("│ Статус:           🟢 SUCCESS                            │");
    println!("│ Продолжительность: {:<2} сек                                 │", data.len());
    println!("├───────────────────────┬────────────────┬────────────────┤");
    println!("│ РЕСУРСЫ               │ СРЕДНЕЕ        │ ПИКОВОЕ (MAX)  │");
    println!("├───────────────────────┼────────────────┼────────────────┤");
    println!("│ 🖥️  CPU Usage         │ {:>12.2}% │ {:>12.2}% │", avg_cpu, max_cpu);
    println!("│ 🎮 GPU Usage         │ {:>12.2}% │ {:>12.2}% │", avg_gpu, max_gpu);
    println!("│ 💾 RAM Allocation    │ {:>11.1} MB │ {:>11} MB │", avg_ram, max_ram);
    println!("└───────────────────────┴────────────────┴────────────────┘\n");
}

pub fn generate_single_chart(version_name: &str, data: &[MetricPoint]) {
    let filename = format!("{}/chart_{}.png", REPORTS_DIR, version_name);
    
    // Увеличиваем высоту изображения до 750px, чтобы снизу поместился блок результатов
    let root = BitMapBackend::new(&filename, (900, 750)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    // 1. Рассчитываем метрики производительности с гарантированным лимитом в 100%
    let count = data.len() as f32;
    let avg_cpu = if count > 0.0 { (data.iter().map(|p| p.cpu).sum::<f32>() / count).min(100.0) } else { 0.0 };
    let avg_gpu = if count > 0.0 { (data.iter().map(|p| p.gpu).sum::<f32>() / count).min(100.0) } else { 0.0 };
    let avg_ram = if count > 0.0 { data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / count as f64 } else { 0.0 };

    let max_cpu = data.iter().map(|p| p.cpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_gpu = data.iter().map(|p| p.gpu).fold(0.0f32, |a, b| a.max(b)).min(100.0);
    let max_ram = data.iter().map(|p| p.ram_mb).max().unwrap_or(0);

    // Разделяем рабочую область на верхнюю (график) и нижнюю (таблица)
    let (upper, lower) = root.split_vertically(460);

    // --- ОТРИСОВКА ГРАФИКА (Верхняя область) ---
    let mut chart = ChartBuilder::on(&upper)
        .caption(format!("Тест производительности: {}", version_name.to_uppercase()), ("sans-serif", 24).into_font())
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..data.len() + 1, 0.0f32..100.0f32) 
        .unwrap();

    chart.configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Нагрузка (%)")
        .draw()
        .unwrap();

    // Линия CPU
    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap()
        .label("CPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));

    // Линия GPU
    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap()
        .label("GPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));

    chart.configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(&BLACK)
        .draw()
        .unwrap();

    // --- ОТРИСОВКА ТАБЛИЦЫ РЕЗУЛЬТАТОВ (Нижняя область) ---
    lower.draw(&Rectangle::new([(40, 10), (860, 250)], Into::<ShapeStyle>::into(&BLACK).stroke_width(2))).unwrap();
    
    let font_title = ("sans-serif", 18).into_font().style(FontStyle::Bold);
    let font_header = ("sans-serif", 14).into_font().style(FontStyle::Bold);
    let font_normal = ("sans-serif", 14).into_font();

    lower.draw(&Text::new("📊 PERFORMANCE SUMMARY REPORT (BENCHMARK)", (60, 25), font_title.color(&BLACK))).unwrap();
    
    lower.draw(&Text::new("РЕСУРСЫ", (80, 70), font_header.color(&BLACK))).unwrap();
    lower.draw(&Text::new("СРЕДНЕЕ ЗНАЧЕНИЕ", (320, 70), font_header.color(&BLACK))).unwrap();
    lower.draw(&Text::new("ПИКОВОЕ (MAX)", (620, 70), font_header.color(&BLACK))).unwrap();
    
    lower.draw(&PathElement::new(vec![(50, 95), (850, 95)], &BLACK)).unwrap();

    // Строка CPU Usage 
    lower.draw(&Text::new("🖥️  CPU Usage", (80, 115), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", avg_cpu), (320, 115), font_normal.clone().color(&RED))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", max_cpu), (620, 115), font_normal.clone().color(&RED))).unwrap();

    // Строка GPU Usage 
    lower.draw(&Text::new("🎮  GPU Usage", (80, 155), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", avg_gpu), (320, 155), font_normal.clone().color(&BLUE))).unwrap();
    lower.draw(&Text::new(format!("{:.2} %", max_gpu), (620, 155), font_normal.clone().color(&BLUE))).unwrap();

    // Строка RAM Allocation
    lower.draw(&Text::new("💾  RAM Allocation", (80, 195), font_normal.clone().color(&BLACK))).unwrap();
    lower.draw(&Text::new(format!("{:.1} MB", avg_ram), (320, 195), font_normal.clone().color(&MAGENTA))).unwrap();
    lower.draw(&Text::new(format!("{} MB", max_ram), (620, 195), font_normal.clone().color(&MAGENTA))).unwrap();

    println!("📈 Сводный график с таблицей метрик сохранен в: {}", filename);
}

// ---------------------------------------------------------
// ОБНОВЛЕННАЯ ФУНКЦИЯ ДЛЯ ГРАФИКА СРАВНЕНИЯ (С ТАБЛИЦЕЙ ВНИЗУ)
// ---------------------------------------------------------
pub fn generate_comparison_chart(new_data: &[MetricPoint], old_data: &[MetricPoint]) {
    let filename = format!("{}/comparison_report.png", REPORTS_DIR);
    
    // Увеличиваем высоту изображения, чтобы поместилась таблица
    let root = BitMapBackend::new(&filename, (1024, 850)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let max_len = new_data.len().max(old_data.len());

    // Разделяем область: верх для графика, низ для таблицы
    let (upper, lower) = root.split_vertically(530);

    // --- ОТРИСОВКА ГРАФИКА (Верхняя область) ---
    let mut chart = ChartBuilder::on(&upper)
        .caption("Сравнение версий: Актуальная vs Старая", ("sans-serif", 28).into_font())
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..max_len + 1, 0.0f32..100.0f32)
        .unwrap();

    chart.configure_mesh().x_desc("Время сценария (секунды)").y_desc("Использование ресурсов (%)").draw().unwrap();

    // Линии (Добавили легенды, чтобы они отображались)
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap()
        .label("CPU (Актуальная)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));
        
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.cpu)), &MAGENTA)).unwrap()
        .label("CPU (Старая)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], MAGENTA));
        
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap()
        .label("GPU (Актуальная)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));
        
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.gpu)), &CYAN)).unwrap()
        .label("GPU (Старая)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], CYAN));

    chart.configure_series_labels().background_style(WHITE.mix(0.8)).border_style(&BLACK).draw().unwrap();

    // --- ОТРИСОВКА ТАБЛИЦЫ СРАВНЕНИЯ (Нижняя область) ---
    
    // 1. Считаем средние значения
    let new_count = new_data.len().max(1) as f32;
    let new_avg_cpu = (new_data.iter().map(|p| p.cpu).sum::<f32>() / new_count).min(100.0);
    let new_avg_gpu = (new_data.iter().map(|p| p.gpu).sum::<f32>() / new_count).min(100.0);
    let new_avg_ram = new_data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / new_count as f64;

    let old_count = old_data.len().max(1) as f32;
    let old_avg_cpu = (old_data.iter().map(|p| p.cpu).sum::<f32>() / old_count).min(100.0);
    let old_avg_gpu = (old_data.iter().map(|p| p.gpu).sum::<f32>() / old_count).min(100.0);
    let old_avg_ram = old_data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / old_count as f64;

    // 2. Считаем разницу (Δ)
    let diff_cpu = new_avg_cpu - old_avg_cpu;
    let diff_gpu = new_avg_gpu - old_avg_gpu;
    let diff_ram = new_avg_ram - old_avg_ram;

    // Отрисовываем рамку таблицы
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

    // Замыкание для отрисовки строки
    let draw_row = |y: i32, title: &str, new_val: f64, old_val: f64, diff_val: f64, unit: &str| {
        lower.draw(&Text::new(title, (80, y), font_normal.clone().color(&BLACK))).unwrap();
        lower.draw(&Text::new(format!("{:.2} {}", new_val, unit), (320, y), font_normal.clone().color(&BLACK))).unwrap();
        lower.draw(&Text::new(format!("{:.2} {}", old_val, unit), (560, y), font_normal.clone().color(&BLACK))).unwrap();

        let sign = if diff_val > 0.0 { "+" } else { "" };
        let diff_text = format!("{}{:.2} {}", sign, diff_val, unit);
        
        // Цвет: красный если стало хуже (больше ресурсов), зеленый если стало лучше
        let color = if diff_val > 0.0 { &RED } else if diff_val < 0.0 { &GREEN } else { &BLACK };
        lower.draw(&Text::new(diff_text, (800, y), font_normal.clone().color(color))).unwrap();
    };

    draw_row(120, "🖥️  CPU Usage", new_avg_cpu as f64, old_avg_cpu as f64, diff_cpu as f64, "%");
    draw_row(170, "🎮  GPU Usage", new_avg_gpu as f64, old_avg_gpu as f64, diff_gpu as f64, "%");
    draw_row(220, "💾  RAM Allocation", new_avg_ram, old_avg_ram, diff_ram, "MB");

    println!("👉 Сводный сравнительный график с таблицей сохранен в: {}", filename);
}

// ---------------------------------------------------------
// ФУНКЦИЯ ДЛЯ ВЫВОДА ТАБЛИЦЫ СРАВНЕНИЯ В КОНСОЛЬ
// ---------------------------------------------------------
pub fn print_comparison_report(new_data: &[MetricPoint], old_data: &[MetricPoint]) {
    if new_data.is_empty() || old_data.is_empty() {
        return;
    }

    let new_count = new_data.len() as f32;
    let new_avg_cpu = (new_data.iter().map(|p| p.cpu).sum::<f32>() / new_count).min(100.0);
    let new_avg_gpu = (new_data.iter().map(|p| p.gpu).sum::<f32>() / new_count).min(100.0);
    let new_avg_ram = new_data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / new_count as f64;

    let old_count = old_data.len() as f32;
    let old_avg_cpu = (old_data.iter().map(|p| p.cpu).sum::<f32>() / old_count).min(100.0);
    let old_avg_gpu = (old_data.iter().map(|p| p.gpu).sum::<f32>() / old_count).min(100.0);
    let old_avg_ram = old_data.iter().map(|p| p.ram_mb as f64).sum::<f64>() / old_count as f64;

    let diff_cpu = new_avg_cpu - old_avg_cpu;
    let diff_gpu = new_avg_gpu - old_avg_gpu;
    let diff_ram = new_avg_ram - old_avg_ram;

    let format_diff = |val: f64, unit: &str| -> String {
        let sign = if val > 0.0 { "+" } else { "" };
        let text = format!("[{}{:.1}{}]", sign, val, unit);
        let padded = format!("{:>12}", text); 
        
        if val > 0.0 {
            format!("\x1b[31m{}\x1b[0m", padded) 
        } else if val < 0.0 {
            format!("\x1b[32m{}\x1b[0m", padded) 
        } else {
            padded 
        }
    };

    let diff_cpu_str = format_diff(diff_cpu as f64, "%");
    let diff_gpu_str = format_diff(diff_gpu as f64, "%");
    let diff_ram_str = format_diff(diff_ram, " MB");

    let new_cpu_str = format!("{:.2}%", new_avg_cpu);
    let old_cpu_str = format!("{:.2}%", old_avg_cpu);
    let new_gpu_str = format!("{:.2}%", new_avg_gpu);
    let old_gpu_str = format!("{:.2}%", old_avg_gpu);
    let new_ram_str = format!("{:.1} MB", new_avg_ram);
    let old_ram_str = format!("{:.1} MB", old_avg_ram);

    println!("\n┌───────────────────────┬────────────────────┬─────────────────┬──────────────┐");
    println!("│ 📊 COMPARISON REPORT: ACTUAL VS OLD                                         │");
    println!("├───────────────────────┬────────────────────┬─────────────────┬──────────────┤");
    println!("│ РЕСУРСЫ               │ АКТУАЛЬНАЯ ВЕРСИЯ  │ СТАРАЯ ВЕРСИЯ   │ РАЗНИЦА (Δ)  │");
    println!("├───────────────────────┼────────────────────┼─────────────────┼──────────────┤");
    println!("│ 🖥️  CPU Usage         │ {:>18} │ {:>15} │ {} │", new_cpu_str, old_cpu_str, diff_cpu_str);
    println!("│ 🎮 GPU Usage         │ {:>18} │ {:>15} │ {} │", new_gpu_str, old_gpu_str, diff_gpu_str);
    println!("│ 💾 RAM Allocation    │ {:>18} │ {:>15} │ {} │", new_ram_str, old_ram_str, diff_ram_str);
    println!("└───────────────────────┴────────────────────┴─────────────────┴──────────────┘\n");
}