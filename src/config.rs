use std::env;
use dotenvy::dotenv;

pub struct Config {
    pub app_path_new: String,
    pub app_path_old: Option<String>, // Option означает, что переменной может не быть
}

impl Config {
    pub fn load() -> Self {
        dotenv().ok();
        
        let app_path_new = env::var("APP_PATH_NEW")
            .expect("КРИТИЧЕСКАЯ ОШИБКА: APP_PATH_NEW не найден в .env");
            
        // Если переменной нет в .env, или она пустая, запишем None
        let app_path_old = env::var("APP_PATH_OLD").ok().filter(|s| !s.is_empty());

        Self {
            app_path_new,
            app_path_old,
        }
    }
}