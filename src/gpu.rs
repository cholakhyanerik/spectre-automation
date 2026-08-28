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
    /// (покрывает AMD и Intel, для которых нет простого API). Несёт в себе открытый
    /// ОДИН РАЗ запрос PDH — так же, как `LinuxSysfs` несёт свой путь: в цикле замера
    /// остаётся пара вызовов библиотеки вместо запуска powershell.exe на каждую секунду.
    ///
    /// Хэндлы хранятся как `isize`, а не как указатели, и это не косметика:
    /// `GpuMonitor` живёт внутри задачи, отданной `tokio::spawn` (monitor.rs), то есть
    /// обязан оставаться `Send`. Сырой указатель отнял бы это молча — ошибкой сборки
    /// в чужом файле, а не здесь.
    WindowsCounter { query: isize, counter: isize },
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
        if let Some(ref n) = nvml
            && let Ok(device) = n.device_by_index(0) {
                let name = device
                    .name()
                    .unwrap_or_else(|_| "NVIDIA GPU".to_string());
                return Self {
                    nvml,
                    backend: GpuBackend::Nvidia,
                    name,
                };
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
            if let Some((query, counter)) = windows_gpu_open() {
                return Self {
                    nvml,
                    backend: GpuBackend::WindowsCounter { query, counter },
                    name,
                };
            }

            // Счётчик не открылся — значит метрики GPU не будет ВЕСЬ прогон.
            // Сказать об этом вслух обязательно: ровная линия по нулю в отчёте
            // выглядит как «видеокарта не нагружалась», а не как отказ измерения
            // (Правило 6). Молча оставить бэкенд выбранным — худший вариант.
            eprintln!(
                "⚠️  Счётчик загрузки GPU недоступен: PDH не отдал \
                 \\GPU Engine(*)\\Utilization Percentage. Метрика GPU будет нулевой \
                 весь прогон — сравнивать по ней сборки нельзя."
            );
            return Self {
                nvml,
                backend: GpuBackend::Unavailable,
                name: format!("{} (счётчик недоступен)", name),
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
                if let Some(ref nvml) = self.nvml
                    && let Ok(device) = nvml.device_by_index(0)
                        && let Ok(util) = device.utilization_rates() {
                            return util.gpu as f32;
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

            // Синхронный вызов внутри async — намеренно, как и чтение sysfs выше:
            // это пара обращений к уже открытому запросу PDH, около миллисекунды.
            // Раньше здесь стоял `.await` на запуске powershell.exe, и стоил он
            // 3,2 секунды при шаге семплинга в секунду (замерено 28.08.2026).
            GpuBackend::WindowsCounter { query, counter } => windows_gpu_usage(*query, *counter),

            GpuBackend::MacOsIoreg => macos_gpu_usage().await,

            GpuBackend::Unavailable => 0.0,
        }
    }
}

/// Закрываем запрос PDH за собой. На прогоне-сравнении мониторов создаётся два,
/// и каждый держит запрос с сотнями экземпляров счётчика (на этой машине — 506).
#[cfg(target_os = "windows")]
impl Drop for GpuMonitor {
    fn drop(&mut self) {
        if let GpuBackend::WindowsCounter { query, .. } = &self.backend {
            unsafe { PdhCloseQuery(*query) };
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

// PDH (Performance Data Helper) — системная библиотека Windows, та самая, через
// которую работает и `Get-Counter`. Пять объявлений мы пишем сами: обёрточный
// крейт ради пяти строк — лишняя зависимость (раздел «Общее» в CLAUDE.md), а
// `raw-dylib` снимает и требование Windows SDK — импорт строится по имени DLL,
// линковщику не нужен `pdh.lib`.
//
// Хэндлы объявлены как `isize`, а не `*mut c_void`: для непрозрачного значения
// размером с указатель это тот же ABI, зато `GpuBackend` остаётся `Send`.
#[cfg(target_os = "windows")]
#[link(name = "pdh.dll", kind = "raw-dylib")]
unsafe extern "system" {
    fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut isize) -> u32;
    fn PdhAddEnglishCounterW(
        query: isize,
        path: *const u16,
        user_data: usize,
        counter: *mut isize,
    ) -> u32;
    fn PdhCollectQueryData(query: isize) -> u32;
    fn PdhGetFormattedCounterArrayW(
        counter: isize,
        format: u32,
        buffer_size: *mut u32,
        item_count: *mut u32,
        buffer: *mut PdhCounterItem,
    ) -> u32;
    fn PdhCloseQuery(query: isize) -> u32;
}

/// Просим у PDH значения типа `double`.
#[cfg(target_os = "windows")]
const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
/// «Буфера не хватило, вот нужный размер» — штатный первый ответ, а не ошибка.
#[cfg(target_os = "windows")]
const PDH_MORE_DATA: u32 = 0x8000_07D2;
/// Значение экземпляра снято корректно.
#[cfg(target_os = "windows")]
const PDH_CSTATUS_VALID_DATA: u32 = 0x0000_0000;
/// То же самое, но значение обновилось с прошлого сбора. Тоже ВАЛИДНОЕ: пропустить
/// его — значит недосчитать как раз те движки, которые сейчас и работают.
#[cfg(target_os = "windows")]
const PDH_CSTATUS_NEW_DATA: u32 = 0x0000_0001;

/// Элемент массива, который отдаёт `PdhGetFormattedCounterArrayW`
/// (`PDH_FMT_COUNTERVALUE_ITEM_W`). На x86-64 занимает 24 байта: указатель на имя
/// экземпляра (8), код состояния (4), выравнивание (4), само значение (8).
/// Раскладка сверена с замером: шаг между элементами реального буфера — 24 байта.
///
/// Имя экземпляра нам не нужно (мы берём максимум по всем), но выкинуть поле
/// нельзя — от него зависит смещение остальных.
#[cfg(target_os = "windows")]
#[repr(C)]
struct PdhCounterItem {
    name: *mut u16,
    status: u32,
    value: f64,
}

/// Открывает запрос PDH к счётчику загрузки GPU и делает первый, базовый сбор.
///
/// `None` означает «счётчика нет» — например, на системе, где счётчики
/// производительности отключены или повреждены (`unlodctr`). Это НЕ то же самое,
/// что «GPU не нагружен»: вызывающая сторона обязана развести эти случаи, иначе
/// получится ровная нулевая линия, неотличимая от измеренной (Правило 6).
#[cfg(target_os = "windows")]
fn windows_gpu_open() -> Option<(isize, isize)> {
    let mut query: isize = 0;
    if unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != PDH_CSTATUS_VALID_DATA {
        return None;
    }

    // Путь берётся АНГЛИЙСКИЙ, и это не недосмотр: `PdhAddEnglishCounterW` сам
    // переводит его в имена текущего языка системы. Имена счётчиков локализованы,
    // и раньше эту работу делал скрипт, лазивший в реестр Perflib, — без него
    // Get-Counter не находил путь на русской Windows и метрика молча уходила
    // в ноль (Правило 6). Теперь перевод делает сама библиотека, то есть целого
    // класса отказов «путь зашит по-английски» здесь больше нет.
    //
    // Звёздочка обязана остаться звёздочкой: набор движков GPU меняется на ходу
    // (запустилось приложение, подключили внешнюю карту). Проверено 28.08.2026 на
    // этой машине — при запуске нового окна экземпляров стало 544 вместо 520, и
    // однажды открытый запрос увидел новые сам. Зафиксировать конкретный
    // экземпляр — значит перестать видеть как раз то, что меряем.
    let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut counter: isize = 0;
    if unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) }
        != PDH_CSTATUS_VALID_DATA
    {
        unsafe { PdhCloseQuery(query) };
        return None;
    }

    // Счётчик скоростной: первый сбор только ставит базу, значения появляются
    // со второго (проверено — первый отдаёт PDH_CSTATUS_INVALID_DATA). Делаем
    // базовый сбор здесь, до начала цикла, чтобы ПЕРВАЯ точка графика была
    // измеренной. Иначе разгон приложения — самое интересное место прогона —
    // начинался бы с нуля-заглушки.
    unsafe { PdhCollectQueryData(query) };

    Some((query, counter))
}

/// Windows: загрузка GPU через счётчик производительности, снятый напрямую из PDH.
///
/// Берём МАКСИМУМ по всем движкам, а не сумму: суммирование 3D+Copy+Video+Compute
/// сильно завышает значение и легко уходит за 100%. Максимум одного движка —
/// корректный показатель утилизации видеокарты.
///
/// Окно усреднения — интервал между соседними сборами, то есть шаг семплинга
/// харнесса. Это и есть отличие от прежнего `Get-Counter`, который брал свой
/// собственный интервал.
#[cfg(target_os = "windows")]
fn windows_gpu_usage(query: isize, counter: isize) -> f32 {
    if unsafe { PdhCollectQueryData(query) } != PDH_CSTATUS_VALID_DATA {
        return 0.0;
    }

    // Протокол у PDH двухшаговый: сначала спрашиваем размер буфера, потом читаем.
    // Между двумя шагами набор экземпляров может измениться (см. выше — он живой),
    // и тогда второй вызов снова ответит PDH_MORE_DATA. Поэтому пробуем несколько
    // раз, а не считаем один промах отказом: молча вернуть 0.0 здесь — как раз
    // тот отказ, который не отличить от измерения.
    for _ in 0..3 {
        let mut size: u32 = 0;
        let mut items: u32 = 0;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut items,
                std::ptr::null_mut(),
            )
        };
        if status != PDH_MORE_DATA {
            return 0.0;
        }

        // Буфер заводим как массив самих структур, а не как Vec<u8>: PDH пишет
        // сюда PDH_FMT_COUNTERVALUE_ITEM_W, а Vec<u8> выровнен по одному байту,
        // и чтение структур из него было бы неопределённым поведением.
        // Хвостом в том же буфере лежат строки имён — отсюда округление вверх.
        //
        // Одна аллокация на замер (около 13 КБ на 544 экземпляра) — это то, что
        // Правило 1 просит не делать в цикле. Осознанно: она заменила запуск
        // powershell.exe, стоивший 3,2 секунды. Убрать её совсем можно только
        // переиспользуемым буфером, а он требует мутабельности через `&self`,
        // то есть Mutex, — цена больше выигрыша.
        let capacity = (size as usize).div_ceil(size_of::<PdhCounterItem>());
        let mut buffer: Vec<PdhCounterItem> = Vec::with_capacity(capacity);

        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut items,
                buffer.as_mut_ptr(),
            )
        };
        if status == PDH_MORE_DATA {
            continue; // набор экземпляров успел вырасти — спрашиваем размер заново
        }
        if status != PDH_CSTATUS_VALID_DATA {
            return 0.0;
        }

        // PDH заполнил `items` элементов; больше, чем влезло в буфер, он не пишет.
        let filled = (items as usize).min(capacity);
        let values = unsafe { std::slice::from_raw_parts(buffer.as_ptr(), filled) };
        return max_engine_usage(values.iter().map(|item| (item.status, item.value)))
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
    }

    0.0
}

/// Максимум по движкам GPU среди значений, которые PDH пометил валидными.
///
/// `None` означает «ни одного валидного значения» и НЕ равно нулю: ноль сказал бы,
/// что видеокарта простаивала, а здесь мы просто ничего не измерили. В `0.0` это
/// превращает вызывающая сторона — сознательно и в одном месте, как это уже
/// сделано для macOS в `parse_ioreg_utilization` (Правило 6).
#[cfg(target_os = "windows")]
fn max_engine_usage<I: IntoIterator<Item = (u32, f64)>>(samples: I) -> Option<f32> {
    samples
        .into_iter()
        .filter(|(status, _)| {
            *status == PDH_CSTATUS_VALID_DATA || *status == PDH_CSTATUS_NEW_DATA
        })
        .map(|(_, value)| value as f32)
        .fold(None, |best: Option<f32>, value| {
            Some(match best {
                Some(b) if b >= value => b,
                _ => value,
            })
        })
}

/// Заглушка для не-Windows сборок, чтобы вызов компилировался.
#[cfg(not(target_os = "windows"))]
fn windows_gpu_usage(_query: isize, _counter: isize) -> f32 {
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

/// Этот модуль, в отличие от соседнего macOS-ного, на Windows ВЫПОЛНЯЕТСЯ —
/// `max_engine_usage` живёт под `#[cfg(target_os = "windows")]`. На Linux и macOS
/// его в выводе `cargo test` не будет вовсе.
#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    /// Максимум, а не сумма. Суммирование 3D+Copy+Video+Compute завышает
    /// утилизацию и уходит за 100 %, а `clamp` превращает завышение в ровную
    /// полку на сотне — то есть в отказ, неотличимый от измерения.
    /// Красным виден так: заменить `fold` на суммирование — здесь станет 90.
    #[test]
    fn takes_the_busiest_engine_not_their_sum() {
        let samples = [
            (PDH_CSTATUS_VALID_DATA, 40.0),
            (PDH_CSTATUS_VALID_DATA, 30.0),
            (PDH_CSTATUS_VALID_DATA, 20.0),
        ];
        assert_eq!(max_engine_usage(samples), Some(40.0));
    }

    /// PDH помечает часть экземпляров как невалидные (движок исчез между сбором
    /// и чтением). Их значения — мусор, и попасть в отчёт они не должны.
    #[test]
    fn skips_instances_pdh_marked_invalid() {
        const PDH_CSTATUS_INVALID_DATA: u32 = 0xC000_0BBA;
        let samples = [
            (PDH_CSTATUS_VALID_DATA, 7.0),
            (PDH_CSTATUS_INVALID_DATA, 99.0),
        ];
        assert_eq!(max_engine_usage(samples), Some(7.0));
    }

    /// А вот PDH_CSTATUS_NEW_DATA (1) — валидное значение, а не отказ: так PDH
    /// помечает экземпляры, обновившиеся с прошлого сбора, то есть РАБОТАЮЩИЕ
    /// прямо сейчас. Отфильтровать их по «status != 0» — значит выбросить именно
    /// нагруженные движки и получить правдоподобно заниженную метрику.
    /// Красным виден так: сузить фильтр до одного PDH_CSTATUS_VALID_DATA.
    #[test]
    fn counts_freshly_updated_instances_too() {
        let samples = [
            (PDH_CSTATUS_VALID_DATA, 3.0),
            (PDH_CSTATUS_NEW_DATA, 61.0),
        ];
        assert_eq!(max_engine_usage(samples), Some(61.0));
    }

    /// Ни одного валидного значения — это ОТКАЗ измерения, и на этом уровне он
    /// обязан быть отличим от честного нуля. В `0.0` его превращает вызывающая
    /// `windows_gpu_usage` — сознательно и в одном месте (Правило 6).
    #[test]
    fn nothing_valid_is_none_not_zero() {
        const PDH_CSTATUS_INVALID_DATA: u32 = 0xC000_0BBA;
        assert_eq!(max_engine_usage([(PDH_CSTATUS_INVALID_DATA, 50.0)]), None);
        assert_eq!(max_engine_usage([]), None);
    }

    /// Простаивающая видеокарта — это Some(0.0), а не None: ноль здесь измерен.
    /// Разница видна только на этом уровне, и ради неё функция и отдаёт Option.
    #[test]
    fn measured_idle_is_zero_not_absence() {
        assert_eq!(max_engine_usage([(PDH_CSTATUS_VALID_DATA, 0.0)]), Some(0.0));
    }

    /// Раскладка PDH_FMT_COUNTERVALUE_ITEM_W задана Windows, а не нами: имя (8) +
    /// код состояния (4) + выравнивание (4) + значение (8). Разъедется она молча —
    /// PDH пишет в буфер по своей раскладке, а читаем мы по своей, и в отчёт
    /// поедут числа, собранные из чужих байтов. Шаг 24 сверен замером на этой
    /// машине 28.08.2026.
    #[test]
    fn counter_item_layout_matches_what_pdh_writes() {
        assert_eq!(size_of::<PdhCounterItem>(), 24);
        assert_eq!(align_of::<PdhCounterItem>(), 8);
    }
}

/// ВНИМАНИЕ: этот модуль собирается и выполняется ТОЛЬКО на macOS — сама
/// `parse_ioreg_utilization` живёт под `#[cfg(target_os = "macos")]`. На Windows
/// и Linux `cargo test` пройдёт зелёным, НЕ выполнив отсюда ни одной проверки.
/// Зелёный вывод здесь не означает, что ветка macOS проверена; CI в проекте нет,
/// так что проверить её можно только на самой macOS.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Фрагмент вывода `ioreg -r -d 1 -w 0 -c IOAccelerator -k
    /// PerformanceStatistics`: словарь в одну строку, значения через запятую.
    const IOREG: &str = concat!(
        "  | {\n",
        "  |   \"PerformanceStatistics\" = {\"Alloc system memory\"=123456,",
        "\"Device Utilization %\"=37,\"Renderer Utilization %\"=12,",
        "\"In use system memory\"=654321}\n",
        "  | }\n",
    );

    /// Значение берётся из середины словаря и обрывается на запятой, а не
    /// заглатывает соседний ключ.
    #[test]
    fn reads_utilization_from_ioreg_dump() {
        assert_eq!(parse_ioreg_utilization(IOREG), Some(37.0));
    }

    /// Дробная часть не должна теряться: округление до целого здесь никем
    /// не заказано, а на слабой нагрузке съедало бы всю разницу между сборками.
    #[test]
    fn keeps_fractional_value() {
        assert_eq!(parse_ioreg_utilization("\"Device Utilization %\"=37.5,"), Some(37.5));
    }

    /// Ключа нет — это ОТКАЗ измерения, и на этом уровне он обязан быть
    /// отличим от честного нуля. В `0.0` его превращает вызывающая
    /// `macos_gpu_usage` — сознательно и в одном месте (Правило 6).
    #[test]
    fn missing_key_is_none_not_zero() {
        assert_eq!(parse_ioreg_utilization("{\"Renderer Utilization %\"=12}"), None);
        assert_eq!(parse_ioreg_utilization(""), None);
    }

    /// Ключ есть, а числа за ним нет (обрезанный или изменившийся вывод) —
    /// тоже отказ, а не ноль.
    #[test]
    fn key_without_number_is_none() {
        assert_eq!(parse_ioreg_utilization("\"Device Utilization %\"=,\"Free memory\"=1"), None);
    }
}
