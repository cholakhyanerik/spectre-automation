use std::process::{Child, Command};
use std::thread;
use std::time::Duration;
use enigo::{Enigo, Mouse, Settings, Button, Direction, Coordinate};

/// Запускает приложение по указанному пути (кросс-платформенно).
///
/// На Windows/Linux это прямой запуск бинарного файла.
/// На macOS, если указан бандл `.app`, запускаем вложенный исполняемый файл
/// напрямую (через `open` нельзя получить PID процесса для мониторинга).
pub fn run_app(path: &str) -> Child {
    let exec_path = resolve_executable(path);
    Command::new(&exec_path)
        .spawn()
        .unwrap_or_else(|e| panic!("Не удалось запустить приложение '{}': {}", exec_path, e))
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

/// Эмулирует пользовательский сценарий взаимодействия с GUI
pub fn execute_ui_scenario() {
    // Даем приложению открыться
    thread::sleep(Duration::from_secs(2));

    let mut enigo = Enigo::new(&Settings::default()).unwrap();

    // Клик 1: Имитируем переход по меню
    enigo.move_mouse(300, 200, Coordinate::Abs).unwrap();
    enigo.button(Button::Left, Direction::Click).unwrap();
    thread::sleep(Duration::from_millis(500));

    // Клик 2: Имитируем запуск тяжелой задачи для теста производительности
    enigo.move_mouse(500, 400, Coordinate::Abs).unwrap();
    enigo.button(Button::Left, Direction::Click).unwrap();
    thread::sleep(Duration::from_secs(2));
}