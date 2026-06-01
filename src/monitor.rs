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
    let pid_sys = Pid::from(pid as usize);

    sys.refresh_all();
    tokio::time::sleep(Duration::from_millis(100)).await;

    for second in 1..=duration_secs {
        sys.refresh_all();
        
        let mut cpu = 0.0;
        let mut ram = 0;
        
        if let Some(process) = sys.process(pid_sys) {
            cpu = process.cpu_usage();
            ram = process.memory() / 1_048_576; // Перевод байт в Мегабайты
        }
        
        let gpu = gpu::get_gpu_usage();

        history.push(MetricPoint {
            second: second as usize,
            cpu,
            ram_mb: ram,
            gpu,
        });
        
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    history
}