use std::time::Duration;
use sysinfo::{Pid, System}; 
use serde::{Serialize, Deserialize};
use crate::gpu;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub second: usize,
    pub cpu: f32,
    pub ram_mb: u64,
    pub gpu: f32,
}

pub async fn start_monitoring(pid: u32, duration_secs: u64) -> Vec<MetricPoint> {
    let mut sys = System::new_all();
    let mut history = Vec::new();
    let main_pid = Pid::from(pid as usize);

    // Первоначальное обновление для инициализации счетчиков
    sys.refresh_all();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Определяем количество логических ядер процессора
    let cpu_count = sys.cpus().len() as f32;
    let safe_cpu_count = if cpu_count > 0.0 { cpu_count } else { 1.0 };

    for second in 1..=duration_secs {
        sys.refresh_all();
        
        // Переменные для агрегации метрик со всего дерева процессов
        let mut total_cpu = 0.0;
        let mut total_ram_bytes = 0;

        // Собираем метрики по всему дереву процессов
        collect_tree_metrics(&sys, main_pid, &mut total_cpu, &mut total_ram_bytes);
        
        // 1. Приводим CPU к стандартным 0-100% (делением на кол-во ядер) и ограничиваем до 100.0
        let mut normalized_cpu = total_cpu / safe_cpu_count;
        if normalized_cpu > 100.0 {
            normalized_cpu = 100.0;
        }

        // Переводим байты в Мегабайты
        let ram_mb = total_ram_bytes / 1_048_576; 
        
        // Получаем и нормализуем GPU (также гарантируем не более 100%)
        let mut gpu_usage = gpu::get_gpu_usage();
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

    history
}

/// Рекурсивная функция для обхода дерева процессов и суммирования их ресурсов
fn collect_tree_metrics(sys: &System, pid: Pid, total_cpu: &mut f32, total_ram: &mut u64) {
    if let Some(process) = sys.process(pid) {
        *total_cpu += process.cpu_usage();
        *total_ram += process.memory();
    }

    // Ищем все дочерние процессы, у которых parent_pid равен текущему pid
    for (p_pid, process) in sys.processes() {
        if let Some(parent) = process.parent() {
            if parent == pid {
                collect_tree_metrics(sys, *p_pid, total_cpu, total_ram);
            }
        }
    }
}