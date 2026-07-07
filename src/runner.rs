use std::process::{Child, Command};
use std::thread;
use std::time::Duration;
use enigo::{Enigo, Mouse, Settings, Button, Direction, Coordinate};

/// Запускает приложение по указанному пути (кросс-платформенно).
///
/// На Windows/Linux это прямой запуск бинарного файла.
/// На macOS, если указан бандл `.app`, запускаем вложенный исполняемый файл
/// напрямую (через `open` нельзя получить PID процесса для мониторинга).
pub fn run_app(path: &str) -> std::io::Result<Child> {
    let exec_path = resolve_executable(path);
    Command::new(&exec_path).spawn()
}

/// Преобразует путь к macOS-бандлу `Foo.app` в путь к реальному бинарю внутри него.
/// Для остальных платформ/путей возвращает исходный путь без изменений.
fn resolve_executable(path: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        use std::path::Path;
        let p = Path::new(path);
        if p.extension().and_then(|e| e.to_str()) == Some("app") {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                let inner = p.join("Contents/MacOS").join(stem);
                if inner.exists() {
                    return inner.to_string_lossy().into_owned();
                }
            }
        }
    }
    path.to_string()
}

/// Эмулирует пользовательский сценарий взаимодействия с GUI.
///
/// Координаты кликов берутся ОТНОСИТЕЛЬНО размера основного дисплея, а не
/// захардкоженными пикселями — так сценарий не зависит от разрешения экрана.
/// Ошибки ввода не роняют процесс (мониторинг важнее сценария): в худшем
/// случае клик пропускается, но замер метрик продолжается.
pub fn execute_ui_scenario() {
    // Даем приложению открыться
    thread::sleep(Duration::from_secs(2));

    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("⚠️  Не удалось инициализировать управление вводом: {e}. Сценарий UI пропущен.");
            return;
        }
    };

    // Размер основного дисплея (с разумным запасным вариантом).
    let (w, h) = enigo.main_display().unwrap_or((1920, 1080));
    let point = |fx: f32, fy: f32| ((w as f32 * fx) as i32, (h as f32 * fy) as i32);

    // Клик 1: Имитируем переход по меню (верхняя левая четверть).
    let (x1, y1) = point(0.30, 0.25);
    let _ = enigo.move_mouse(x1, y1, Coordinate::Abs);
    let _ = enigo.button(Button::Left, Direction::Click);
    thread::sleep(Duration::from_millis(500));

    // Клик 2: Имитируем запуск тяжелой задачи (центр экрана).
    let (x2, y2) = point(0.50, 0.50);
    let _ = enigo.move_mouse(x2, y2, Coordinate::Abs);
    let _ = enigo.button(Button::Left, Direction::Click);
    thread::sleep(Duration::from_secs(2));
}