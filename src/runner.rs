use std::process::{Child, Command};
use std::thread;
use std::time::Duration;
use enigo::{Enigo, Mouse, Settings, Button, Direction, Coordinate};

/// Запускает приложение по указанному пути
pub fn run_app(path: &str) -> Child {
    Command::new(path)
        .spawn()
        .expect("Не удалось запустить приложение")
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