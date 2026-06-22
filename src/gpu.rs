use std::path::PathBuf;
use nvml_wrapper::Nvml;

/// Способ получения загрузки GPU, выбранный один раз при инициализации.
enum GpuBackend {
    /// NVIDIA через NVML (работает на Windows и Linux).
    Nvidia,
    /// Linux: чтение sysfs `gpu_busy_percent` (драйвер amdgpu / некоторые intel).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    LinuxSysfs(PathBuf),
    /// Windows: счётчик производительности "\GPU Engine(*)\Utilization Percentage"
    /// (покрывает AMD и Intel, для которых нет простого API).
    WindowsCounter,
    /// macOS / Apple Silicon: утилизация интегрированного GPU через ioreg
    /// (ключ "Device Utilization %" в PerformanceStatistics, без прав root).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    MacOsIoreg,
    /// GPU не обнаружен либо метрика недоступна на данной ОС.
    Unavailable,
}

pub struct GpuMonitor {
    // Держим Nvml живым на всю сессию: повторная инициализация дорогая и нестабильная.
    nvml: Option<Nvml>,
    backend: GpuBackend,
    /// Человекочитаемое имя адаптера для отчётов (например "NVIDIA GeForce RTX 4070").
    pub name: String,
}

impl GpuMonitor {
    pub fn new() -> Self {
        // 1. NVIDIA через NVML — самый точный и быстрый путь (Windows + Linux).
        let nvml = Nvml::init().ok();
        if let Some(ref n) = nvml {
            if let Ok(device) = n.device_by_index(0) {
                let name = device
                    .name()
                    .unwrap_or_else(|_| "NVIDIA GPU".to_string());
                return Self {
                    nvml,
                    backend: GpuBackend::Nvidia,
                    name,
                };
            }
        }

        // 2. Linux: AMD/Intel через sysfs gpu_busy_percent.
        #[cfg(target_os = "linux")]
        {
            if let Some((path, name)) = linux_find_gpu() {
                return Self {
                    nvml,
                    backend: GpuBackend::LinuxSysfs(path),
                    name,
                };
            }
        }

        // 3. Windows: AMD/Intel через счётчик производительности.
        #[cfg(target_os = "windows")]
        {
            let name = windows_gpu_name().unwrap_or_else(|| "GPU (Windows)".to_string());
            return Self {
                nvml,
                backend: GpuBackend::WindowsCounter,
                name,
            };
        }

        // 4. macOS / Apple Silicon: интегрированный GPU через ioreg.
        #[cfg(target_os = "macos")]
        {
            return Self {
                nvml,
                backend: GpuBackend::MacOsIoreg,
                name: macos_gpu_name(),
            };
        }

        // 5. Прочие случаи: метрика недоступна.
        #[allow(unreachable_code)]
        Self {
            nvml,
            backend: GpuBackend::Unavailable,
            name: "GPU не обнаружен".to_string(),
        }
    }

    /// Возвращает загрузку GPU в процентах (0..100).
    pub async fn get_gpu_usage(&self) -> f32 {
        match &self.backend {
            GpuBackend::Nvidia => {
                if let Some(ref nvml) = self.nvml {
                    if let Ok(device) = nvml.device_by_index(0) {
                        if let Ok(util) = device.utilization_rates() {
                            return util.gpu as f32;
                        }
                    }
                }
                0.0
            }

            GpuBackend::LinuxSysfs(path) => {
                // Синхронное чтение крошечного файла sysfs — мгновенно, блокировки нет.
                std::fs::read_to_string(path)
                    .ok()
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(0.0)
                    .clamp(0.0, 100.0)
            }

            GpuBackend::WindowsCounter => windows_gpu_usage().await,

            GpuBackend::MacOsIoreg => macos_gpu_usage().await,

            GpuBackend::Unavailable => 0.0,
        }
    }
}

/// Linux: ищем первую карту в /sys/class/drm с доступным gpu_busy_percent
/// и определяем её вендора по PCI-id.
#[cfg(target_os = "linux")]
fn linux_find_gpu() -> Option<(PathBuf, String)> {
    let drm = std::path::Path::new("/sys/class/drm");
    let entries = std::fs::read_dir(drm).ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Интересуют только cardN (не cardN-eDP-1 и т.п.)
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }

        let device_dir = entry.path().join("device");
        let busy = device_dir.join("gpu_busy_percent");
        if !busy.exists() {
            continue;
        }

        // Определяем вендора по PCI vendor id.
        let vendor = std::fs::read_to_string(device_dir.join("vendor"))
            .ok()
            .map(|s| s.trim().to_lowercase());
        let vendor_name = match vendor.as_deref() {
            Some("0x1002") => "AMD",
            Some("0x8086") => "Intel",
            Some("0x10de") => "NVIDIA",
            _ => "GPU",
        };

        return Some((busy, format!("{} ({})", vendor_name, name)));
    }
    None
}

/// Windows: имя видеоадаптера через CIM (выполняется один раз при инициализации).
#[cfg(target_os = "windows")]
fn windows_gpu_name() -> Option<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_VideoController | Select-Object -First 1 -ExpandProperty Name)",
        ])
        .output()
        .ok()?;

    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Windows: загрузка GPU через счётчик производительности.
/// Берём МАКСИМУМ по всем движкам, а не сумму: суммирование 3D+Copy+Video+Compute
/// сильно завышает значение и легко уходит за 100%. Максимум одного движка —
/// корректный показатель утилизации видеокарты.
#[cfg(target_os = "windows")]
async fn windows_gpu_usage() -> f32 {
    let output = tokio::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -ErrorAction SilentlyContinue).CounterSamples | Measure-Object -Property CookedValue -Maximum | Select-Object -ExpandProperty Maximum",
        ])
        .output()
        .await;

    if let Ok(out) = output {
        return String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<f32>()
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
    }
    0.0
}

/// Заглушка для не-Windows сборок, чтобы вызов компилировался.
#[cfg(not(target_os = "windows"))]
async fn windows_gpu_usage() -> f32 {
    0.0
}

/// macOS / Apple Silicon: имя чипа через sysctl (например "Apple M2 Pro").
/// На Apple Silicon GPU интегрирован в SoC, поэтому показываем имя чипа.
#[cfg(target_os = "macos")]
fn macos_gpu_name() -> String {
    let chip = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    match chip {
        Some(c) => format!("{} (Apple integrated GPU)", c),
        None => "Apple Silicon GPU".to_string(),
    }
}

/// macOS / Apple Silicon: утилизация GPU через ioreg без прав root.
/// Парсим "Device Utilization %" из словаря PerformanceStatistics акселератора.
#[cfg(target_os = "macos")]
async fn macos_gpu_usage() -> f32 {
    let output = tokio::process::Command::new("ioreg")
        .args(["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator", "-k", "PerformanceStatistics"])
        .output()
        .await;

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(v) = parse_ioreg_utilization(&text) {
            return v.clamp(0.0, 100.0);
        }
    }
    0.0
}

/// Извлекает значение `"Device Utilization %"=NN` из вывода ioreg.
#[cfg(target_os = "macos")]
fn parse_ioreg_utilization(text: &str) -> Option<f32> {
    let key = "\"Device Utilization %\"=";
    let idx = text.find(key)?;
    let rest = &text[idx + key.len()..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse::<f32>().ok()
}

/// Заглушка для не-macOS сборок, чтобы вызов компилировался.
#[cfg(not(target_os = "macos"))]
async fn macos_gpu_usage() -> f32 {
    0.0
}
