use std::collections::HashSet;
use std::time::Duration;
use sysinfo::{Pid, System};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Local};
use crate::gpu::GpuMonitor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub second: usize,
    pub cpu: f32,
    pub ram_mb: u64,
    pub gpu: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub timestamp: String,
    pub exe_name: String,
    pub duration_secs: u64,
    /// Платформа, на которой проводился замер (Windows / Linux / macOS).
    #[serde(default)]
    pub platform: String,
    /// Имя обнаруженного GPU-адаптера (для контекста в отчётах).
    #[serde(default)]
    pub gpu_name: String,
    pub metrics: Vec<MetricPoint>,
}

pub async fn start_monitoring(
    pid: u32,
    duration_secs: u64,
    exe_name: String,
    match_patterns: Vec<String>,
) -> TestResult {
    let mut sys = System::new_all();
    let mut history = Vec::new();
    let main_pid = Pid::from(pid as usize);
    let start_time: DateTime<Local> = Local::now();

    // Полный набор подстрок имён, относящихся к приложению:
    // имя запущенного бинаря (ловит мультиоконный режим: spectre-terminal.exe (2))
    // + паттерны модалок/семейства из конфига (ловит "Spectre Settings" как отд. приложение).
    let mut patterns: Vec<String> = match_patterns;
    let exe_lc = exe_name.to_lowercase();
    if !exe_lc.is_empty() && !patterns.iter().any(|p| p == &exe_lc) {
        patterns.push(exe_lc);
    }
    
    // Инициализация монитора GPU (без повторных пересозданий)
    let gpu_monitor = GpuMonitor::new();
    println!("🎮 Обнаружен GPU: {}", gpu_monitor.name);

    // Первоначальное обновление для инициализации счетчиков (важно для точного CPU)
    sys.refresh_all();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Определяем количество логических ядер процессора
    let cpu_count = sys.cpus().len() as f32;
    let safe_cpu_count = if cpu_count > 0.0 { cpu_count } else { 1.0 };

    for second in 1..=duration_secs {
        sys.refresh_all();
        
        // Агрегируем метрики по всем процессам приложения:
        // дерево потомков ∪ процессы, чьё имя совпало с паттернами (дедуп по PID).
        let (total_cpu, total_ram_bytes) = collect_app_metrics(&sys, main_pid, &patterns);
        
        // 1. Приводим CPU к стандартным 0-100% и ограничиваем
        let mut normalized_cpu = total_cpu / safe_cpu_count;
        if normalized_cpu > 100.0 {
            normalized_cpu = 100.0;
        }

        // Переводим байты в Мегабайты
        let ram_mb = total_ram_bytes / 1_048_576; 
        
        // 2. Асинхронно получаем GPU без блокировки потока
        let mut gpu_usage = gpu_monitor.get_gpu_usage().await;
        if gpu_usage > 100.0 {
            gpu_usage = 100.0;
        }

        history.push(MetricPoint {
            second: second as usize,
            cpu: normalized_cpu,
            ram_mb,
            gpu: gpu_usage,
        });
        
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    TestResult {
        timestamp: start_time.format("%Y-%m-%d %H:%M:%S").to_string(),
        exe_name,
        duration_secs,
        platform: crate::config::platform_name().to_string(),
        gpu_name: gpu_monitor.name.clone(),
        metrics: history,
    }
}

/// Возвращает множество PID, входящих в дерево процессов (главный + все потомки
/// любой глубины). Считается по родительским ссылкам методом фиксированной точки.
fn collect_subtree(sys: &System, main_pid: Pid) -> HashSet<Pid> {
    let mut tree: HashSet<Pid> = HashSet::new();
    tree.insert(main_pid);

    loop {
        let mut added = false;
        for (pid, process) in sys.processes() {
            if tree.contains(pid) {
                continue;
            }
            if let Some(parent) = process.parent() {
                if tree.contains(&parent) {
                    tree.insert(*pid);
                    added = true;
                }
            }
        }
        if !added {
            break;
        }
    }
    tree
}

/// true, если имя процесса содержит любой из паттернов (нижний регистр).
fn name_matches(name_lc: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| name_lc.contains(p.as_str()))
}

/// Агрегирует CPU и RAM по всем процессам приложения: объединение дерева
/// потомков и процессов, чьё имя совпало с паттернами. Каждый процесс
/// учитывается ровно один раз (итерация по уникальным PID).
fn collect_app_metrics(sys: &System, main_pid: Pid, patterns: &[String]) -> (f32, u64) {
    let subtree = collect_subtree(sys, main_pid);
    // Не учитываем сам процесс теста (его имя "spectre-automation" тоже ловится паттерном).
    let self_pid = Pid::from(std::process::id() as usize);

    let mut total_cpu = 0.0f32;
    let mut total_ram: u64 = 0;

    for (pid, process) in sys.processes() {
        if *pid == self_pid {
            continue;
        }
        let name_lc = process.name().to_string_lossy().to_lowercase();
        let belongs = subtree.contains(pid) || name_matches(&name_lc, patterns);
        if belongs {
            total_cpu += process.cpu_usage();
            total_ram += process.memory();
        }
    }

    (total_cpu, total_ram)
}

/// Завершает все процессы приложения (главный, доп. окна и модалки-приложения),
/// чтобы осиротевшие окна не перетекали в следующий прогон и не искажали замер.
pub fn terminate_app(main_pid: u32, patterns: &[String]) {
    let mut sys = System::new_all();
    sys.refresh_all();

    let main_pid = Pid::from(main_pid as usize);
    let subtree = collect_subtree(&sys, main_pid);
    // Критично: НЕ убиваем сам процесс теста (его имя тоже совпадает с паттерном).
    let self_pid = Pid::from(std::process::id() as usize);

    for (pid, process) in sys.processes() {
        if *pid == self_pid {
            continue;
        }
        let name_lc = process.name().to_string_lossy().to_lowercase();
        if subtree.contains(pid) || name_matches(&name_lc, patterns) {
            process.kill();
        }
    }
}