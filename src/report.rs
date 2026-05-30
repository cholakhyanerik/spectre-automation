use std::fs::{self, File};
use std::path::Path;
use plotters::prelude::*;
use crate::monitor::MetricPoint;

const REPORTS_DIR: &str = "reports";

// Функция инициализации: удаляет старую папку и создает чистую новую
pub fn init_reports_dir() {
    let path = Path::new(REPORTS_DIR);
    if path.exists() {
        // Удаляем старую папку со всеми файлами внутри
        let _ = fs::remove_dir_all(path);
    }
    // Создаем пустую папку reports
    fs::create_dir_all(path).expect("Не удалось создать директорию reports");
}

// Сохранение в JSON внутрь папки reports
pub fn save_report_json(version_name: &str, data: &[MetricPoint]) {
    let filename = format!("{}/report_{}.json", REPORTS_DIR, version_name);
    let file = File::create(&filename).unwrap();
    serde_json::to_writer_pretty(file, data).expect("Не удалось записать JSON");
    println!("📊 Данные бенчмарка сохранены в: {}", filename);
}

// Одиночный график внутрь папки reports
pub fn generate_single_chart(version_name: &str, data: &[MetricPoint]) {
    let filename = format!("{}/chart_{}.png", REPORTS_DIR, version_name);
    let root = BitMapBackend::new(&filename, (800, 600)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let mut chart = ChartBuilder::on(&root)
        .caption(format!("Тест производительности: {}", version_name), ("sans-serif", 24).into_font())
        .margin(15)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..data.len() + 1, 0.0f32..100.0f32) 
        .unwrap();

    chart.configure_mesh().x_desc("Время (сек)").y_desc("Нагрузка (%)").draw().unwrap();

    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap()
        .label("CPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED));

    chart.draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap()
        .label("GPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE));

    chart.configure_series_labels().background_style(WHITE.mix(0.8)).draw().unwrap();
    println!("📈 График производительности сохранен в: {}", filename);
}

// Сравнительный график внутрь папки reports
pub fn generate_comparison_chart(new_data: &[MetricPoint], old_data: &[MetricPoint]) {
    let filename = format!("{}/comparison_report.png", REPORTS_DIR);
    let root = BitMapBackend::new(&filename, (1024, 768)).into_drawing_area();
    root.fill(&WHITE).unwrap();

    let max_len = new_data.len().max(old_data.len());

    let mut chart = ChartBuilder::on(&root)
        .caption("Сравнение версий: Актуальная vs Старая", ("sans-serif", 28).into_font())
        .margin(20)
        .x_label_area_size(40)
        .y_label_area_size(40)
        .build_cartesian_2d(0..max_len + 1, 0.0f32..100.0f32)
        .unwrap();

    chart.configure_mesh().x_desc("Время сценария (секунды)").y_desc("Использование ресурсов (%)").draw().unwrap();

    // Линии CPU
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.cpu)), &RED)).unwrap().label("CPU (Актуальная)");
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.cpu)), &MAGENTA)).unwrap().label("CPU (Старая)");
    // Линии GPU
    chart.draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.gpu)), &BLUE)).unwrap().label("GPU (Актуальная)");
    chart.draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.gpu)), &CYAN)).unwrap().label("GPU (Старая)");

    chart.configure_series_labels().background_style(WHITE.mix(0.8)).border_style(&BLACK).draw().unwrap();
    println!("👉 Сводный сравнительный график сохранен в: {}", filename);
}