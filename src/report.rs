use std::fs::{self, File};
use std::path::Path;
use plotters::prelude::*;
use chrono::Local;
use serde::{Serialize, Deserialize};
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
const C_RAM_OLD: RGBColor = RGBColor(186, 104, 200); // светло-фиолетовый
const C_GRID_BG: RGBColor = RGBColor(248, 249, 250); // почти белый фон графика
const C_HEADER_BG: RGBColor = RGBColor(33, 37, 41); // тёмная шапка
const C_HEADER_SUB: RGBColor = RGBColor(170, 174, 178); // подзаголовок на тёмной шапке
const C_INK: RGBColor = RGBColor(33, 37, 41); // тёмный текст
const C_MUTED: RGBColor = RGBColor(120, 124, 128); // приглушённый текст
const C_LINE: RGBColor = RGBColor(225, 228, 232); // разделители
const C_GOOD: RGBColor = RGBColor(46, 125, 50); // зелёный (стало лучше)
const C_BAD: RGBColor = RGBColor(198, 40, 40); // красный (стало хуже)

// ────────────────────────── Статистика ──────────────────────────

/// Полный набор описательных статистик по одной метрике за прогон.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
pub struct Stat {
    pub min: f64,
    pub avg: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
    /// Стандартное отклонение — насколько «дёргается» метрика во времени.
    pub stddev: f64,
}

impl Stat {
    fn from(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let n = values.len();
        let avg = values.iter().sum::<f64>() / n as f64;

        let variance = values.iter().map(|v| (v - avg).powi(2)).sum::<f64>() / n as f64;

        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let median = if n.is_multiple_of(2) {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        } else {
            sorted[n / 2]
        };

        // Ближайший ранг: индекс = round(0.95 * (n - 1)).
        let p95_idx = ((n - 1) as f64 * 0.95).round() as usize;

        Self {
            min: sorted[0],
            avg,
            median,
            p95: sorted[p95_idx],
            max: sorted[n - 1],
            stddev: variance.sqrt(),
        }
    }
}

/// Все три метрики прогона, разложенные по статистикам.
pub struct RunStats {
    pub cpu: Stat,
    pub gpu: Stat,
    pub ram: Stat,
    pub samples: usize,
}

fn compute_stats(result: &TestResult) -> RunStats {
    let cpu: Vec<f64> = result.metrics.iter().map(|p| p.cpu.min(100.0) as f64).collect();
    let gpu: Vec<f64> = result.metrics.iter().map(|p| p.gpu.min(100.0) as f64).collect();
    let ram: Vec<f64> = result.metrics.iter().map(|p| p.ram_mb as f64).collect();

    RunStats {
        cpu: Stat::from(&cpu),
        gpu: Stat::from(&gpu),
        ram: Stat::from(&ram),
        samples: result.metrics.len(),
    }
}

// ────────────────────────── История прогонов ──────────────────────────

/// Запись в сводной истории. Поля min/median/p95/stddev помечены `serde(default)`:
/// без этого старые записи (где их нет) не распарсятся, а `save_run_to_history`
/// молча заменит всю историю пустым вектором.
#[derive(Serialize, Deserialize, Debug)]
pub struct RunHistoryRecord {
    pub date_time: String,
    pub executable: String,
    pub duration_secs: u64,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub gpu_name: String,
    #[serde(default)]
    pub samples: usize,

    #[serde(default)]
    pub min_cpu: f32,
    pub avg_cpu: f32,
    #[serde(default)]
    pub median_cpu: f32,
    #[serde(default)]
    pub p95_cpu: f32,
    pub max_cpu: f32,
    #[serde(default)]
    pub stddev_cpu: f32,

    #[serde(default)]
    pub min_gpu: f32,
    pub avg_gpu: f32,
    #[serde(default)]
    pub median_gpu: f32,
    #[serde(default)]
    pub p95_gpu: f32,
    pub max_gpu: f32,
    #[serde(default)]
    pub stddev_gpu: f32,

    #[serde(default)]
    pub min_ram_mb: f64,
    pub avg_ram_mb: f64,
    #[serde(default)]
    pub median_ram_mb: f64,
    #[serde(default)]
    pub p95_ram_mb: f64,
    pub max_ram_mb: u64,
    #[serde(default)]
    pub stddev_ram_mb: f64,
}

fn build_history_record(result: &TestResult) -> RunHistoryRecord {
    let s = compute_stats(result);
    RunHistoryRecord {
        date_time: result.timestamp.clone(),
        executable: result.exe_name.clone(),
        duration_secs: result.duration_secs,
        platform: result.platform.clone(),
        gpu_name: result.gpu_name.clone(),
        samples: s.samples,

        min_cpu: s.cpu.min as f32,
        avg_cpu: s.cpu.avg as f32,
        median_cpu: s.cpu.median as f32,
        p95_cpu: s.cpu.p95 as f32,
        max_cpu: s.cpu.max as f32,
        stddev_cpu: s.cpu.stddev as f32,

        min_gpu: s.gpu.min as f32,
        avg_gpu: s.gpu.avg as f32,
        median_gpu: s.gpu.median as f32,
        p95_gpu: s.gpu.p95 as f32,
        max_gpu: s.gpu.max as f32,
        stddev_gpu: s.gpu.stddev as f32,

        min_ram_mb: s.ram.min,
        avg_ram_mb: s.ram.avg,
        median_ram_mb: s.ram.median,
        p95_ram_mb: s.ram.p95,
        max_ram_mb: s.ram.max as u64,
        stddev_ram_mb: s.ram.stddev,
    }
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

pub fn save_run_to_history(result: &TestResult) {
    let record = build_history_record(result);
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

// ────────────────────────── Консольные отчёты ──────────────────────────

const COL_M: usize = 22; // ширина колонки названия метрики
const COL_V: usize = 11; // ширина колонки значения

/// Полная внутренняя ширина таблицы (между внешними рамками).
const TABLE_W: usize = COL_M + 2 + (COL_V + 2) * 5 + 5;

fn rule(left: &str, mid: &str, right: &str) -> String {
    let m = "─".repeat(COL_M + 2);
    let v = "─".repeat(COL_V + 2);
    format!("{left}{m}{mid}{v}{mid}{v}{mid}{v}{mid}{v}{mid}{v}{right}")
}

fn banner(left: &str, right: &str) -> String {
    format!("{left}{}{right}", "─".repeat(TABLE_W))
}

fn banner_row(text: &str) -> String {
    format!("│{:<w$}│", format!(" {}", text), w = TABLE_W)
}

/// Строка таблицы: название метрики + пять значений.
fn stat_row(label: &str, s: &Stat, unit: &str, decimals: usize) -> String {
    let f = |v: f64| format!("{:.*}{}", decimals, v, unit);
    format!(
        "│ {:<m$} │ {:>v$} │ {:>v$} │ {:>v$} │ {:>v$} │ {:>v$} │",
        label,
        f(s.min),
        f(s.avg),
        f(s.median),
        f(s.p95),
        f(s.max),
        m = COL_M,
        v = COL_V,
    )
}

pub fn print_visual_report(result: &TestResult) {
    if result.metrics.is_empty() {
        return;
    }
    let s = compute_stats(result);

    let platform = if result.platform.is_empty() { "—" } else { result.platform.as_str() };
    let gpu_name = if result.gpu_name.is_empty() { "—" } else { result.gpu_name.as_str() };

    println!();
    println!("{}", banner("┌", "┐"));
    println!("{}", banner_row(&format!("PERFORMANCE REPORT — {}", result.exe_name)));
    println!("{}", banner("├", "┤"));
    println!("{}", banner_row(&format!("Платформа: {}   GPU: {}", platform, gpu_name)));
    println!(
        "{}",
        banner_row(&format!(
            "Запуск: {}   Длительность: {} сек   Замеров: {}",
            result.timestamp, result.duration_secs, s.samples
        ))
    );
    println!("{}", rule("├", "┬", "┤"));
    println!(
        "│ {:<m$} │ {:>v$} │ {:>v$} │ {:>v$} │ {:>v$} │ {:>v$} │",
        "РЕСУРС", "МИН", "СРЕДНЕЕ", "МЕДИАНА", "P95", "ПИК (MAX)",
        m = COL_M,
        v = COL_V,
    );
    println!("{}", rule("├", "┼", "┤"));
    println!("{}", stat_row("CPU Usage", &s.cpu, "%", 2));
    println!("{}", stat_row("GPU Usage", &s.gpu, "%", 2));
    println!("{}", stat_row("RAM Allocation", &s.ram, " MB", 0));
    println!("{}", rule("└", "┴", "┘"));
    println!(
        "  Разброс (σ):  CPU {:.2}%   GPU {:.2}%   RAM {:.1} MB",
        s.cpu.stddev, s.gpu.stddev, s.ram.stddev
    );
    println!();
}

/// Форматирует дельту с ANSI-цветом: рост потребления — красный, снижение — зелёный.
/// Цвет накладывается ПОСЛЕ выравнивания, иначе escape-последовательности
/// съедают ширину колонки.
fn colored_diff(val: f64, unit: &str, width: usize) -> String {
    let sign = if val > 0.0 { "+" } else { "" };
    let padded = format!("{:>width$}", format!("{}{:.2}{}", sign, val, unit));

    if val > 0.0 {
        format!("\x1b[31m{}\x1b[0m", padded)
    } else if val < 0.0 {
        format!("\x1b[32m{}\x1b[0m", padded)
    } else {
        padded
    }
}

/// Относительное изменение в процентах. `None`, если базовое значение нулевое.
fn percent_change(new: f64, old: f64) -> Option<f64> {
    if old.abs() < f64::EPSILON {
        None
    } else {
        Some((new - old) / old * 100.0)
    }
}

fn fmt_pct_change(new: f64, old: f64, width: usize) -> String {
    match percent_change(new, old) {
        Some(p) => {
            let sign = if p > 0.0 { "+" } else { "" };
            let padded = format!("{:>width$}", format!("{}{:.1}%", sign, p));
            if p > 0.0 {
                format!("\x1b[31m{}\x1b[0m", padded)
            } else if p < 0.0 {
                format!("\x1b[32m{}\x1b[0m", padded)
            } else {
                padded
            }
        }
        None => format!("{:>width$}", "—"),
    }
}

/// Обрезает имя до `max` символов, чтобы длинный exe не ломал вёрстку таблицы.
fn truncate(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        let cut: String = name.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}

pub fn print_comparison_report(new_res: &TestResult, old_res: &TestResult) {
    if new_res.metrics.is_empty() || old_res.metrics.is_empty() {
        return;
    }

    let sn = compute_stats(new_res);
    let so = compute_stats(old_res);

    const CM: usize = 22; // метрика
    const CV: usize = 15; // значение версии
    const CD: usize = 13; // дельта
    const CP: usize = 10; // дельта в %
    let inner = CM + 2 + CV + 2 + CV + 2 + CD + 2 + CP + 2 + 4;

    let name_new = truncate(&new_res.exe_name, CV);
    let name_old = truncate(&old_res.exe_name, CV);

    let line = |l: &str, m: &str, r: &str| {
        format!(
            "{l}{}{m}{}{m}{}{m}{}{m}{}{r}",
            "─".repeat(CM + 2),
            "─".repeat(CV + 2),
            "─".repeat(CV + 2),
            "─".repeat(CD + 2),
            "─".repeat(CP + 2),
        )
    };

    println!();
    println!("┌{}┐", "─".repeat(inner));
    println!("│{:<w$}│", format!(" COMPARISON REPORT — {} vs {}", name_new, name_old), w = inner);
    println!("{}", line("├", "┬", "┤"));
    println!(
        "│ {:<CM$} │ {:>CV$} │ {:>CV$} │ {:>CD$} │ {:>CP$} │",
        "РЕСУРС (среднее)", name_new, name_old, "РАЗНИЦА (Δ)", "Δ %",
    );
    println!("{}", line("├", "┼", "┤"));

    let rows: [(&str, f64, f64, &str, usize); 3] = [
        ("CPU Usage", sn.cpu.avg, so.cpu.avg, "%", 2),
        ("GPU Usage", sn.gpu.avg, so.gpu.avg, "%", 2),
        ("RAM Allocation", sn.ram.avg, so.ram.avg, " MB", 1),
    ];

    for (label, new_v, old_v, unit, dec) in rows {
        println!(
            "│ {:<CM$} │ {:>CV$} │ {:>CV$} │ {} │ {} │",
            label,
            format!("{:.*}{}", dec, new_v, unit),
            format!("{:.*}{}", dec, old_v, unit),
            colored_diff(new_v - old_v, unit, CD),
            fmt_pct_change(new_v, old_v, CP),
        );
    }

    println!("{}", line("├", "┼", "┤"));

    // Пиковые значения — отдельным блоком: регрессия часто видна только в пике.
    let peaks: [(&str, f64, f64, &str, usize); 3] = [
        ("CPU пик (max)", sn.cpu.max, so.cpu.max, "%", 2),
        ("GPU пик (max)", sn.gpu.max, so.gpu.max, "%", 2),
        ("RAM пик (max)", sn.ram.max, so.ram.max, " MB", 1),
    ];

    for (label, new_v, old_v, unit, dec) in peaks {
        println!(
            "│ {:<CM$} │ {:>CV$} │ {:>CV$} │ {} │ {} │",
            label,
            format!("{:.*}{}", dec, new_v, unit),
            format!("{:.*}{}", dec, old_v, unit),
            colored_diff(new_v - old_v, unit, CD),
            fmt_pct_change(new_v, old_v, CP),
        );
    }

    println!("{}", line("└", "┴", "┘"));
    println!("  Красным — рост потребления, зелёным — снижение.");
    println!();
}

// ────────────────────────── Графики ──────────────────────────

/// Отступ текста шапки от левого края холста; столько же оставляется справа.
const HEADER_PAD: i32 = 28;

/// Предел для имени видеокарты в подзаголовке шапки. «NVIDIA GeForce RTX 4070
/// Laptop GPU» — ровно 34 знака и проходит целиком; всё, что длиннее, режется,
/// чтобы не вытолкнуть за край дату и число замеров, стоящие следом.
const GPU_NAME_MAX: usize = 34;

/// Разделитель между постоянной частью заголовка и именами.
const HEADER_DASH: &str = "  —  ";
/// Разделитель между двумя именами в заголовке сравнения.
const HEADER_VS: &str = "  vs  ";

/// Ширина строки в пикселях при данном шрифте.
///
/// Считает сам `plotters` — тем же перебором глифов с их шириной и кернингом,
/// которым потом и рисует, так что это не оценка, а факт. Оценка по среднему
/// знаку остаётся ровно на один случай: шрифт не нашёлся в системе. Тогда
/// рисовать всё равно нечем, и приблизительная ширина хуже не сделает.
fn text_width(text: &str, font: &FontDesc<'_>) -> u32 {
    match font.box_size(text) {
        Ok((w, _)) => w,
        Err(_) => (text.chars().count() as f64 * font.get_size() * 0.6) as u32,
    }
}

/// Обрезает строку так, чтобы она заняла не больше `max_w` пикселей.
///
/// В отличие от `truncate`, предел здесь в пикселях, а не в знаках: у кегля 22
/// «W» втрое шире «i», и предел в знаках пришлось бы брать по самой широкой
/// букве — то есть резать нормальные заголовки задолго до края холста.
fn fit_to_width(text: &str, font: &FontDesc<'_>, max_w: u32) -> String {
    if text_width(text, font) <= max_w {
        return text.to_string();
    }
    // Ширина растёт от числа знаков монотонно, поэтому самый длинный
    // влезающий префикс ищется двоичным поиском, а не перебором подряд.
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate = format!("{}…", chars[..mid].iter().collect::<String>());
        if text_width(&candidate, font) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    format!("{}…", chars[..lo].iter().collect::<String>())
}

/// Собирает заголовок шапки, ужимая имена бинарей под ширину холста.
///
/// Имена сюда приходят как есть — это названия веток и тикетов, они длинные, —
/// а `plotters` рисует за границей холста молча: ни ошибки, ни предупреждения,
/// просто пропадает то, ради чего отчёт открывают. Постоянная часть
/// (`COMPARISON —`, `vs`) ширину не выбирает: она вычитается первой, а остаток
/// делится между именами поровну. Обрезать уже собранную строку целиком
/// нельзя — в сравнении первое длинное имя съело бы и «vs», и второе имя.
fn header_title(prefix: &str, names: &[&str], font: &FontDesc<'_>, max_w: u32) -> String {
    let mut skeleton = format!("{}{}", prefix, HEADER_DASH);
    for _ in 1..names.len() {
        skeleton.push_str(HEADER_VS);
    }
    let per_name = max_w.saturating_sub(text_width(&skeleton, font)) / names.len().max(1) as u32;

    let fitted: Vec<String> = names.iter().map(|n| fit_to_width(n, font, per_name)).collect();
    let title = format!("{}{}{}", prefix, HEADER_DASH, fitted.join(HEADER_VS));

    // Вторая обрезка — уже по всей строке: на узком холсте не влезает и одна
    // постоянная часть, и тогда делить между именами просто нечего.
    fit_to_width(&title, font, max_w)
}

/// Рисует тёмную шапку отчёта: имена бинарей + контекст окружения.
fn draw_header<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    prefix: &str,
    names: &[&str],
    subtitle: &str,
) -> ChartResult
where
    DB::ErrorType: 'static,
{
    let (w, _) = area.dim_in_pixel();
    let avail = (w as i32 - HEADER_PAD * 2).max(0) as u32;

    let f_title = ("sans-serif", 22).into_font().style(FontStyle::Bold);
    let f_sub = ("sans-serif", 13).into_font();

    area.fill(&C_HEADER_BG)?;
    area.draw(&Text::new(
        header_title(prefix, names, &f_title, avail),
        (HEADER_PAD, 14),
        f_title.color(&WHITE),
    ))?;
    area.draw(&Text::new(
        fit_to_width(subtitle, &f_sub, avail),
        (HEADER_PAD, 42),
        f_sub.color(&C_HEADER_SUB),
    ))?;
    Ok(())
}

/// Заголовок сводной таблицы под графиками.
fn draw_table_frame<DB: DrawingBackend>(
    area: &DrawingArea<DB, plotters::coord::Shift>,
    x0: i32,
    x1: i32,
) -> ChartResult
where
    DB::ErrorType: 'static,
{
    let (_, h) = area.dim_in_pixel();
    let y1 = h as i32 - 14;
    area.draw(&Rectangle::new([(x0, 8), (x1, y1)], Into::<ShapeStyle>::into(C_GRID_BG).filled()))?;
    area.draw(&Rectangle::new([(x0, 8), (x1, y1)], Into::<ShapeStyle>::into(C_LINE).stroke_width(2)))?;
    Ok(())
}

fn ram_upper_bound(values: &[f64]) -> f64 {
    let max = values.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 { 100.0 } else { max * 1.18 }
}

pub fn generate_single_chart(version_name: &str, result: &TestResult) -> ChartResult {
    let filename = format!("{}/chart_{}.png", DIR_CURRENT, version_name);
    let data = &result.metrics;
    if data.is_empty() {
        return Ok(());
    }
    let s = compute_stats(result);

    let root = BitMapBackend::new(&filename, (1000, 920)).into_drawing_area();
    root.fill(&WHITE)?;

    let (header, rest) = root.split_vertically(70);
    let (load_area, rest2) = rest.split_vertically(370);
    let (ram_area, table_area) = rest2.split_vertically(250);

    let platform = if result.platform.is_empty() { "—" } else { result.platform.as_str() };
    let gpu_name = if result.gpu_name.is_empty() {
        "—".to_string()
    } else {
        truncate(&result.gpu_name, GPU_NAME_MAX)
    };

    draw_header(
        &header,
        "PERFORMANCE REPORT",
        &[result.exe_name.as_str()],
        &format!(
            "{}   |   GPU: {}   |   {}   |   {} сек, {} замеров",
            platform, gpu_name, result.timestamp, result.duration_secs, s.samples
        ),
    )?;

    // ── График нагрузки CPU / GPU ──
    let mut chart = ChartBuilder::on(&load_area)
        .caption(
            "Нагрузка CPU / GPU",
            ("sans-serif", 17).into_font().style(FontStyle::Bold).color(&C_INK),
        )
        .margin(18)
        .margin_top(8)
        .x_label_area_size(38)
        .y_label_area_size(48)
        .build_cartesian_2d(0..data.len() + 1, 0.0f32..100.0f32)?;

    chart.plotting_area().fill(&C_GRID_BG)?;
    chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Нагрузка (%)")
        .y_label_formatter(&|v| format!("{:.0}", v))
        .axis_desc_style(("sans-serif", 13).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 11).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(C_LINE)
        .draw()?;

    chart.draw_series(AreaSeries::new(data.iter().map(|p| (p.second, p.cpu)), 0.0, C_CPU.mix(0.10)))?;
    chart
        .draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.cpu)), C_CPU.stroke_width(3)))?
        .label("CPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU.stroke_width(3)));

    chart.draw_series(AreaSeries::new(data.iter().map(|p| (p.second, p.gpu)), 0.0, C_GPU.mix(0.10)))?;
    chart
        .draw_series(LineSeries::new(data.iter().map(|p| (p.second, p.gpu)), C_GPU.stroke_width(3)))?
        .label("GPU (%)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU.stroke_width(3)));

    // Пунктир среднего CPU — глазу сразу видно, где базовая линия.
    chart.draw_series(LineSeries::new(
        (0..=data.len()).map(|x| (x, s.cpu.avg as f32)),
        C_CPU.mix(0.55).stroke_width(1),
    ))?;

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(C_LINE)
        .label_font(("sans-serif", 12).into_font().color(&C_INK))
        .draw()?;

    // ── График RAM (собственная ось в MB) ──
    let ram_vals: Vec<f64> = data.iter().map(|p| p.ram_mb as f64).collect();
    let ram_max = ram_upper_bound(&ram_vals);

    let mut ram_chart = ChartBuilder::on(&ram_area)
        .caption(
            "Потребление RAM",
            ("sans-serif", 17).into_font().style(FontStyle::Bold).color(&C_INK),
        )
        .margin(18)
        .margin_top(8)
        .x_label_area_size(38)
        .y_label_area_size(58)
        .build_cartesian_2d(0..data.len() + 1, 0.0f64..ram_max)?;

    ram_chart.plotting_area().fill(&C_GRID_BG)?;
    ram_chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Память (MB)")
        .y_label_formatter(&|v| format!("{:.0}", v))
        .axis_desc_style(("sans-serif", 13).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 11).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(C_LINE)
        .draw()?;

    ram_chart.draw_series(AreaSeries::new(
        data.iter().map(|p| (p.second, p.ram_mb as f64)),
        0.0,
        C_RAM.mix(0.12),
    ))?;
    ram_chart
        .draw_series(LineSeries::new(
            data.iter().map(|p| (p.second, p.ram_mb as f64)),
            C_RAM.stroke_width(3),
        ))?
        .label("RAM (MB)")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_RAM.stroke_width(3)));

    ram_chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(C_LINE)
        .label_font(("sans-serif", 12).into_font().color(&C_INK))
        .draw()?;

    // ── Сводная таблица: MIN / AVG / MEDIAN / P95 / MAX / σ ──
    draw_table_frame(&table_area, 30, 970)?;

    let f_title = ("sans-serif", 15).into_font().style(FontStyle::Bold);
    let f_head = ("sans-serif", 12).into_font().style(FontStyle::Bold);
    let f_norm = ("sans-serif", 13).into_font();

    table_area.draw(&Text::new("SUMMARY", (52, 22), f_title.color(&C_INK)))?;

    // Колонки: подпись + 6 числовых
    let cols = [52, 250, 372, 494, 616, 738, 862];
    let heads = ["РЕСУРС", "МИН", "СРЕДНЕЕ", "МЕДИАНА", "P95", "ПИК (MAX)", "РАЗБРОС σ"];
    for (x, h) in cols.iter().zip(heads) {
        table_area.draw(&Text::new(h, (*x, 56), f_head.clone().color(&C_MUTED)))?;
    }
    table_area.draw(&PathElement::new(vec![(44, 78), (956, 78)], C_LINE))?;

    let rows: [(&str, &Stat, &RGBColor, &str, usize); 3] = [
        ("CPU Usage", &s.cpu, &C_CPU, "%", 2),
        ("GPU Usage", &s.gpu, &C_GPU, "%", 2),
        ("RAM Allocation", &s.ram, &C_RAM, " MB", 0),
    ];

    for (i, (label, st, color, unit, dec)) in rows.iter().enumerate() {
        let y = 100 + i as i32 * 38;
        table_area.draw(&Text::new(*label, (cols[0], y), f_norm.clone().color(&C_INK)))?;
        let vals = [st.min, st.avg, st.median, st.p95, st.max, st.stddev];
        for (j, v) in vals.iter().enumerate() {
            table_area.draw(&Text::new(
                format!("{:.*}{}", dec, v, unit),
                (cols[j + 1], y),
                f_norm.clone().color(*color),
            ))?;
        }
    }

    root.present()?; // Принудительно завершаем отрисовку
    println!("📈 График сохранен в: {}", filename);
    Ok(())
}

pub fn generate_comparison_chart(new_res: &TestResult, old_res: &TestResult) -> ChartResult {
    let filename = format!("{}/comparison.png", DIR_CURRENT);

    let new_data = &new_res.metrics;
    let old_data = &old_res.metrics;
    if new_data.is_empty() || old_data.is_empty() {
        return Ok(());
    }

    let sn = compute_stats(new_res);
    let so = compute_stats(old_res);

    let root = BitMapBackend::new(&filename, (1180, 990)).into_drawing_area();
    root.fill(&WHITE)?;

    let (header, rest) = root.split_vertically(70);
    let (load_area, rest2) = rest.split_vertically(390);
    let (ram_area, table_area) = rest2.split_vertically(250);

    let platform = if new_res.platform.is_empty() { "—" } else { new_res.platform.as_str() };
    let gpu_name = if new_res.gpu_name.is_empty() {
        "—".to_string()
    } else {
        truncate(&new_res.gpu_name, GPU_NAME_MAX)
    };

    draw_header(
        &header,
        "COMPARISON",
        &[new_res.exe_name.as_str(), old_res.exe_name.as_str()],
        &format!(
            "{}   |   GPU: {}   |   {}   |   {} сек",
            platform, gpu_name, new_res.timestamp, new_res.duration_secs
        ),
    )?;

    let max_len = new_data.len().max(old_data.len());

    // ── CPU / GPU обеих версий ──
    let mut chart = ChartBuilder::on(&load_area)
        .caption(
            "Нагрузка CPU / GPU",
            ("sans-serif", 17).into_font().style(FontStyle::Bold).color(&C_INK),
        )
        .margin(18)
        .margin_top(8)
        .x_label_area_size(38)
        .y_label_area_size(48)
        .build_cartesian_2d(0..max_len + 1, 0.0f32..100.0f32)?;

    chart.plotting_area().fill(&C_GRID_BG)?;
    chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Использование (%)")
        .y_label_formatter(&|v| format!("{:.0}", v))
        .axis_desc_style(("sans-serif", 13).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 11).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(C_LINE)
        .draw()?;

    chart
        .draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.cpu)), C_CPU.stroke_width(3)))?
        .label(format!("CPU — {}", new_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU.stroke_width(3)));
    chart
        .draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.cpu)), C_CPU_OLD.stroke_width(2)))?
        .label(format!("CPU — {}", old_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_CPU_OLD.stroke_width(2)));
    chart
        .draw_series(LineSeries::new(new_data.iter().map(|p| (p.second, p.gpu)), C_GPU.stroke_width(3)))?
        .label(format!("GPU — {}", new_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU.stroke_width(3)));
    chart
        .draw_series(LineSeries::new(old_data.iter().map(|p| (p.second, p.gpu)), C_GPU_OLD.stroke_width(2)))?
        .label(format!("GPU — {}", old_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_GPU_OLD.stroke_width(2)));

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(C_LINE)
        .label_font(("sans-serif", 12).into_font().color(&C_INK))
        .draw()?;

    // ── RAM обеих версий ──
    let mut ram_vals: Vec<f64> = new_data.iter().map(|p| p.ram_mb as f64).collect();
    ram_vals.extend(old_data.iter().map(|p| p.ram_mb as f64));
    let ram_max = ram_upper_bound(&ram_vals);

    let mut ram_chart = ChartBuilder::on(&ram_area)
        .caption(
            "Потребление RAM",
            ("sans-serif", 17).into_font().style(FontStyle::Bold).color(&C_INK),
        )
        .margin(18)
        .margin_top(8)
        .x_label_area_size(38)
        .y_label_area_size(58)
        .build_cartesian_2d(0..max_len + 1, 0.0f64..ram_max)?;

    ram_chart.plotting_area().fill(&C_GRID_BG)?;
    ram_chart
        .configure_mesh()
        .x_desc("Время (сек)")
        .y_desc("Память (MB)")
        .y_label_formatter(&|v| format!("{:.0}", v))
        .axis_desc_style(("sans-serif", 13).into_font().color(&C_MUTED))
        .label_style(("sans-serif", 11).into_font().color(&C_MUTED))
        .light_line_style(WHITE.mix(0.0))
        .bold_line_style(C_LINE)
        .draw()?;

    ram_chart
        .draw_series(LineSeries::new(
            new_data.iter().map(|p| (p.second, p.ram_mb as f64)),
            C_RAM.stroke_width(3),
        ))?
        .label(format!("RAM — {}", new_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_RAM.stroke_width(3)));
    ram_chart
        .draw_series(LineSeries::new(
            old_data.iter().map(|p| (p.second, p.ram_mb as f64)),
            C_RAM_OLD.stroke_width(2),
        ))?
        .label(format!("RAM — {}", old_res.exe_name))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], C_RAM_OLD.stroke_width(2)));

    ram_chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(C_LINE)
        .label_font(("sans-serif", 12).into_font().color(&C_INK))
        .draw()?;

    // ── Сравнительная таблица ──
    draw_table_frame(&table_area, 30, 1150)?;

    let f_title = ("sans-serif", 15).into_font().style(FontStyle::Bold);
    let f_head = ("sans-serif", 12).into_font().style(FontStyle::Bold);
    let f_norm = ("sans-serif", 13).into_font();

    table_area.draw(&Text::new("COMPARISON SUMMARY", (52, 20), f_title.color(&C_INK)))?;

    let cols = [52, 330, 530, 730, 900, 1030];
    let heads = [
        "РЕСУРС".to_string(),
        truncate(&new_res.exe_name, 22),
        truncate(&old_res.exe_name, 22),
        "РАЗНИЦА (Δ)".to_string(),
        "Δ %".to_string(),
        "ВЕРДИКТ".to_string(),
    ];
    for (x, h) in cols.iter().zip(heads.iter()) {
        table_area.draw(&Text::new(h.clone(), (*x, 52), f_head.clone().color(&C_MUTED)))?;
    }
    table_area.draw(&PathElement::new(vec![(44, 74), (1136, 74)], C_LINE))?;

    // Средние и пиковые значения в одной таблице.
    let rows: [(&str, f64, f64, &str, usize); 6] = [
        ("CPU — среднее", sn.cpu.avg, so.cpu.avg, "%", 2),
        ("CPU — пик (max)", sn.cpu.max, so.cpu.max, "%", 2),
        ("GPU — среднее", sn.gpu.avg, so.gpu.avg, "%", 2),
        ("GPU — пик (max)", sn.gpu.max, so.gpu.max, "%", 2),
        ("RAM — среднее", sn.ram.avg, so.ram.avg, " MB", 1),
        ("RAM — пик (max)", sn.ram.max, so.ram.max, " MB", 1),
    ];

    for (i, (label, new_v, old_v, unit, dec)) in rows.iter().enumerate() {
        let y = 94 + i as i32 * 30;
        let diff = new_v - old_v;
        let color = if diff > 0.0 { &C_BAD } else if diff < 0.0 { &C_GOOD } else { &C_INK };
        let sign = if diff > 0.0 { "+" } else { "" };

        table_area.draw(&Text::new(*label, (cols[0], y), f_norm.clone().color(&C_INK)))?;
        table_area.draw(&Text::new(format!("{:.*}{}", dec, new_v, unit), (cols[1], y), f_norm.clone().color(&C_INK)))?;
        table_area.draw(&Text::new(format!("{:.*}{}", dec, old_v, unit), (cols[2], y), f_norm.clone().color(&C_INK)))?;
        table_area.draw(&Text::new(format!("{}{:.*}{}", sign, dec, diff, unit), (cols[3], y), f_norm.clone().color(color)))?;

        let pct = match percent_change(*new_v, *old_v) {
            Some(p) => format!("{}{:.1}%", if p > 0.0 { "+" } else { "" }, p),
            None => "—".to_string(),
        };
        table_area.draw(&Text::new(pct, (cols[4], y), f_norm.clone().color(color)))?;

        let verdict = if diff > 0.0 { "хуже" } else if diff < 0.0 { "лучше" } else { "без изм." };
        table_area.draw(&Text::new(verdict, (cols[5], y), f_norm.clone().color(color)))?;
    }

    root.present()?;
    println!("👉 Сводный сравнительный график сохранен в: {}", filename);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Медиана — то, чем в отчётах отвечают на вопрос «сколько потребляет»,
    /// поэтому обе ветки выбора (чётное / нечётное число замеров) проверяются явно.
    #[test]
    fn median_even_and_odd() {
        // Чётное число значений: среднее двух центральных.
        assert_eq!(Stat::from(&[1.0, 2.0, 3.0, 4.0]).median, 2.5);
        // Нечётное: ровно центральное.
        assert_eq!(Stat::from(&[1.0, 2.0, 3.0]).median, 2.0);
    }

    /// Пустой набор замеров возможен (прогон не дал ни одной точки) и не должен
    /// ронять отчёт выходом за границы отсортированного вектора.
    #[test]
    fn empty_values_give_zeros() {
        let s = Stat::from(&[]);
        assert_eq!((s.min, s.avg, s.median, s.p95, s.max), (0.0, 0.0, 0.0, 0.0, 0.0));
    }

    /// Один-единственный замер — вырожденный, но достижимый случай: прогон на
    /// секунду или потеря всех точек кроме одной. Все пять статистик обязаны
    /// сойтись к этому значению, а p95 — не выйти за границу вектора.
    #[test]
    fn single_value_collapses_to_itself() {
        let s = Stat::from(&[42.5]);
        assert_eq!((s.min, s.avg, s.median, s.p95, s.max), (42.5, 42.5, 42.5, 42.5, 42.5));
        assert_eq!(s.stddev, 0.0);
    }

    /// Индекс p95 — единственное место в `Stat::from`, где номер элемента
    /// вычисляется, а не берётся с края отсортированного вектора. Ошибка на
    /// единицу здесь либо роняет прогон паникой уже ПОСЛЕ замера, либо молча
    /// подменяет p95 максимумом — и в отчёте одно от другого неотличимо.
    #[test]
    fn p95_index_stays_in_bounds() {
        // 95-й перцентиль набора 1..=100 — это 95: round(99 × 0,95) = индекс 94.
        let hundred: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(Stat::from(&hundred).p95, 95.0);
        // На двадцати точках: round(19 × 0,95) = 18, то есть девятнадцатая.
        let twenty: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(Stat::from(&twenty).p95, 19.0);
        // На двух точках округление уводит индекс на последнюю.
        assert_eq!(Stat::from(&[1.0, 2.0]).p95, 2.0);

        // Ни на одной длине прогона индекс не должен выходить за вектор:
        // паника здесь случилась бы после того, как замер уже сделан и
        // повторить его нечем.
        for n in 1..=200usize {
            let values: Vec<f64> = (1..=n).map(|v| v as f64).collect();
            let s = Stat::from(&values);
            assert!(
                s.median <= s.p95 && s.p95 <= s.max,
                "n = {n}: p95 вне отрезка [медиана, максимум]"
            );
        }
    }

    /// Нулевая база — не «изменение на бесконечность», а «сравнивать не с чем».
    /// Ноль в старой версии здесь обычен: метрика GPU, которая не смогла
    /// измериться, приезжает нулём (Правило 6). Деление на него дало бы в
    /// отчёте `+inf%` или `NaN%` — величину, с виду неотличимую от настоящей.
    #[test]
    fn percent_change_has_no_answer_for_zero_base() {
        assert_eq!(percent_change(5.0, 0.0), None);
        assert_eq!(percent_change(0.0, 0.0), None);
        // И в консольной таблице на этом месте должен стоять прочерк.
        assert_eq!(fmt_pct_change(5.0, 0.0, 10).trim(), "—");
    }

    /// Изменение считается ОТ СТАРОГО значения: перепутанный знаменатель даёт
    /// правдоподобный процент не той величины.
    #[test]
    fn percent_change_is_relative_to_old() {
        assert_eq!(percent_change(12.0, 10.0), Some(20.0));
        assert_eq!(percent_change(8.0, 10.0), Some(-20.0));
        assert_eq!(percent_change(10.0, 10.0), Some(0.0));
    }

    /// Обрезка считает ЗНАКИ, а не байты. Имена сборок бывают кириллическими,
    /// и байтовый срез `&name[..max]` развалился бы паникой на середине символа
    /// посреди печати уже готового отчёта. Ширина колонки при этом соблюдается
    /// ровно: вылези результат на знак — рамка таблицы разъедется, и увидят это
    /// только глазами на настоящем прогоне.
    #[test]
    fn truncate_counts_chars_not_bytes() {
        // 15 — ширина колонки версии в сравнительной таблице (CV).
        let cut = truncate("Спектр-терминал-длинное-имя-ветки", 15);
        assert_eq!(cut, "Спектр-термина…");
        assert_eq!(cut.chars().count(), 15);
        // Байтов при этом заметно больше — на них и нельзя было опираться.
        assert!(cut.len() > 15, "кириллица уместилась в 15 байт: {cut}");
    }

    /// Имя, которое влезает целиком, не должно получить многоточие: лишний
    /// знак «…» читается в отчёте как обрезанное имя ветки.
    #[test]
    fn truncate_leaves_fitting_names_alone() {
        assert_eq!(truncate("spectre.exe", 15), "spectre.exe");
        // Ровно по границе — ещё не обрезаем.
        assert_eq!(truncate("Спектр-термина", 14), "Спектр-термина");
    }

    /// Пустые или нулевые данные RAM дают потолок оси 100, а не 0: ось `0..0`
    /// вырождает график в линию, и вместо «памяти намерили ноль» человек видит
    /// пустую картинку, по которой об отказе замера не догадаться.
    #[test]
    fn ram_axis_never_degenerates() {
        assert_eq!(ram_upper_bound(&[]), 100.0);
        assert_eq!(ram_upper_bound(&[0.0, 0.0, 0.0]), 100.0);
    }

    /// На нормальных данных потолок оси идёт ВЫШЕ пика: ляг линия ровно по
    /// верхней рамке — пик стал бы неотличим от полки, а полка означает отказ.
    #[test]
    fn ram_axis_leaves_headroom_above_peak() {
        let bound = ram_upper_bound(&[120.0, 512.0, 300.0]);
        assert!(bound > 512.0, "потолок оси не выше пика: {bound}");
        assert!((bound - 512.0 * 1.18).abs() < 1e-9, "потолок не 18 % над пиком: {bound}");
    }

    /// Шрифт шапки: тот же кегль и начертание, что в `draw_header`.
    fn header_font() -> FontDesc<'static> {
        ("sans-serif", 22).into_font().style(FontStyle::Bold)
    }

    /// Свободная ширина строки шапки для холста заданной ширины.
    fn header_avail(canvas_w: i32) -> u32 {
        (canvas_w - HEADER_PAD * 2) as u32
    }

    /// Дефект, ради которого писался `header_title`: длинные имена веток
    /// уезжали за правый край PNG, а `plotters` рисует там молча — ни ошибки,
    /// ни предупреждения, просто пропадает то, что сравнивали.
    #[test]
    fn long_names_stay_inside_canvas() {
        let font = header_font();
        let long = "TD-1055-text-rendering-refactor-with-a-very-long-branch-name.exe";

        // Сравнение (холст 1180). Одинаковым именам достаётся одинаковая доля:
        // обрезка всей строки целиком оставила бы от второго имени огрызок, и
        // проверка «в строке есть vs» такую починку бы пропустила.
        let avail = header_avail(1180);
        let title = header_title("COMPARISON", &[long, long], &font, avail);
        assert!(text_width(&title, &font) <= avail, "заголовок шире холста: {title}");
        let (head, tail) = title.split_once(HEADER_VS).expect("в заголовке сравнения нет «vs»");
        let head_name = head.trim_start_matches("COMPARISON").trim_start_matches(HEADER_DASH);
        assert_eq!(head_name, tail, "имена ужаты по-разному: {title}");
        assert!(tail.chars().count() > 10, "от имён остались огрызки: {title}");

        // Одиночный отчёт (холст 1000).
        let avail = header_avail(1000);
        let title = header_title("PERFORMANCE REPORT", &[long], &font, avail);
        assert!(text_width(&title, &font) <= avail, "заголовок шире холста: {title}");
    }

    /// Обрезка не должна трогать имена нормальной длины: многоточие там, где
    /// всё влезало, — такая же порча отчёта, только в другую сторону.
    #[test]
    fn short_names_are_left_alone() {
        let font = header_font();
        assert_eq!(
            header_title("PERFORMANCE REPORT", &["spectre.exe"], &font, header_avail(1000)),
            "PERFORMANCE REPORT  —  spectre.exe",
        );
        assert_eq!(
            header_title("COMPARISON", &["new.exe", "old.exe"], &font, header_avail(1180)),
            "COMPARISON  —  new.exe  vs  old.exe",
        );
    }

    /// `fit_to_width` меряет пиксели, а не знаки: строка из узких букв должна
    /// пережить обрезку длиннее, чем такая же по числу знаков из широких.
    /// Заодно это проверка, что шрифт вообще нашёлся: не нашёлся — обе строки
    /// померяются по среднему знаку, окажутся равной длины, и тест упадёт.
    #[test]
    fn fit_to_width_counts_pixels_not_chars() {
        let font = header_font();
        let narrow = fit_to_width(&"i".repeat(200), &font, 300);
        let wide = fit_to_width(&"W".repeat(200), &font, 300);
        assert!(narrow.ends_with('…') && wide.ends_with('…'));
        assert!(
            narrow.chars().count() > wide.chars().count(),
            "узких знаков влезло не больше широких: {} против {}",
            narrow.chars().count(),
            wide.chars().count(),
        );
    }
}
