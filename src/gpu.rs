use tokio::process::Command;
use nvml_wrapper::Nvml;

pub struct GpuMonitor {
    nvml: Option<Nvml>,
}

impl GpuMonitor {
    pub fn new() -> Self {
        // Инициализируем Nvml один раз за сессию тестирования для стабильности
        Self {
            nvml: Nvml::init().ok(),
        }
    }

    pub async fn get_gpu_usage(&self) -> f32 {
        // 1. Пробуем NVIDIA (очень быстрый синхронный запрос)
        if let Some(ref nvml) = self.nvml {
            if let Ok(device) = nvml.device_by_index(0) {
                if let Ok(util) = device.utilization_rates() {
                    return util.gpu as f32;
                }
            }
        }

        // 2. Кроссплатформенный фоллбек (Windows PowerShell для Intel/AMD)
        // Сделан асинхронным, чтобы не блокировать 1-секундный тик мониторинга!
        if cfg!(target_os = "windows") {
            let output = Command::new("powershell")
                .args([
                    "-Command",
                    "(Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction SilentlyContinue).CounterSamples | Measure-Object -Property CookedValue -Sum | Select-Object -ExpandProperty Sum"
                ])
                .output()
                .await;

            if let Ok(out) = output {
                return String::from_utf8_lossy(&out.stdout).trim().parse::<f32>().unwrap_or(0.0);
            }
        }
        
        0.0
    }
}