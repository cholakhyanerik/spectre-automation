use std::env;
use dotenvy::dotenv;

pub struct Config {
    pub app_path_new: String,
    pub app_path_old: Option<String>,
    pub test_duration_secs: u64,
}

impl Config {
    pub fn load() -> Self {
        dotenv().ok();
        
        let app_path_new = env::var("APP_PATH_NEW")
            .expect("КРИТИЧЕСКАЯ ОШИБКА: APP_PATH_NEW не найден в .env");
            
        let app_path_old = env::var("APP_PATH_OLD").ok().filter(|s| !s.is_empty());

        // Читаем длительность теста, если ошибка или её нет — ставим дефолт 10 секунд
        let test_duration_secs = env::var("TEST_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10); // По дефолту 10 секунд

        Self {
            app_path_new,
            app_path_old,
            test_duration_secs,
        }
    }
}