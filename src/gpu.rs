use std::process::Command;
use nvml_wrapper::Nvml;

pub fn get_gpu_usage() -> f32 {
    // 1. Пробуем NVIDIA
    if let Ok(nvml) = Nvml::init() {
        if let Ok(device) = nvml.device_by_index(0) {
            if let Ok(util) = device.utilization_rates() {
                return util.gpu as f32;
            }
        }
    }

    // 2. Кроссплатформенный фоллбек (Windows PowerShell для Intel/AMD встроенных)
    if cfg!(target_os = "windows") {
        let output = Command::new("powershell")
            .args([
                "-Command",
                "(Get-Counter '\\GPU Engine(*)\\Utilization Percentage').CounterSamples | Measure-Object -Property CookedValue -Sum | Select-Object -ExpandProperty Sum"
            ])
            .output();

        if let Ok(out) = output {
            return String::from_utf8_lossy(&out.stdout).trim().parse::<f32>().unwrap_or(0.0);
        }
    }
    
    0.0
}