use std::env;
use std::path::PathBuf;
use dotenvy::dotenv;

/// Порог по умолчанию, ниже которого замер считается слишком коротким.
///
/// Это не «время до полки», а длительность с запасом: на контрольном прогоне
/// (180 сек, TD-1055-text-rendering.exe, Quadro P2200, Windows, 25.08.2026)
/// RAM выходила на 95 % полки к 12-й секунде, всплеск CPU держался до 13-й, —
/// но после разгона должно остаться ещё достаточно точек, чтобы медиана и p95
/// описывали установившийся режим, а не хвост переходного процесса. Замер чужой
/// программы стареет быстрее кода: на другой сборке терминала или другой машине
/// величину надо перемерить, а не унаследовать отсюда.
pub const DEFAULT_SETTLE_SECS: u64 = 60;

pub struct Config {
    pub app_path_new: String,
    pub app_path_old: Option<String>,
    pub test_duration_secs: u64,
    /// Сколько секунд закладывается на выход приложения на режим. Прогон короче
    /// этого укладывается в разгон целиком и меряет запуск, а не потребление, —
    /// об этом говорится вслух. Задаётся через SETTLE_SECS в .env, 0 отключает
    /// предупреждение.
    pub settle_secs: u64,
    /// Путь к файлу состояния (exchanges.db). Может быть задан в .env через DB_PATH,
    /// иначе вычисляется автоматически под текущую ОС.
    pub db_path: PathBuf,
    /// Подстроки имён процессов (в нижнем регистре), которые считаются частью
    /// тестируемого приложения: доп. окна (мультиоконный режим) и модалки,
    /// запускаемые как отдельные приложения (например "Spectre Settings").
    /// Задаётся через MATCH_PROCESSES в .env, иначе выводится из имени бинаря.
    pub match_processes: Vec<String>,
}

impl Config {
    pub fn load() -> Self {
        // Ошибку разбора .env нельзя глотать: при ней dotenvy не загружает НИ ОДНОЙ
        // переменной, и падение выглядит как «переменная не найдена», хотя на деле
        // файл просто не распарсился (например, строка без `#` и без `=`).
        // Отсутствие самого файла — не ошибка: переменные могут прийти из окружения.
        if let Err(e) = dotenv()
            && !e.not_found()
        {
            eprintln!("⚠️  ОШИБКА РАЗБОРА .env: {e}");
            eprintln!("    Переменные из .env НЕ загружены. Проверьте синтаксис:");
            eprintln!("    каждая строка — либо `КЛЮЧ=значение`, либо комментарий с `#`.");
        }

        let app_path_new = env::var("APP_PATH_NEW")
            .expect("КРИТИЧЕСКАЯ ОШИБКА: APP_PATH_NEW не найден в .env");

        let app_path_old = env::var("APP_PATH_OLD").ok().filter(|s| !s.is_empty());

        // Читаем длительность теста, если ошибка или её нет — ставим дефолт 10 секунд
        let test_duration_secs = env::var("TEST_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10); // По дефолту 10 секунд

        // Порог, ниже которого замер меряет запуск, а не потребление.
        // Маленький дефолт TEST_DURATION_SECS оставлен намеренно (быстрая
        // проверка самого харнесса), поэтому защита здесь — не запрет, а голос.
        let settle_secs = env::var("SETTLE_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SETTLE_SECS);

        // Путь к БД: либо явный из .env, либо кросс-платформенный дефолт
        let db_path = env::var("DB_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(default_db_path);

        // Паттерны имён процессов приложения: явные из .env или выведенные из бинаря
        let match_processes = env::var("MATCH_PROCESSES")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_lowercase())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| default_match_patterns(&app_path_new));

        Self {
            app_path_new,
            app_path_old,
            test_duration_secs,
            settle_secs,
            db_path,
            match_processes,
        }
    }
}

/// true, если прогон длительностью `test_duration_secs` короче времени выхода
/// приложения на режим и потому меряет ЗАПУСК, а не потребление.
///
/// Вынесено в отдельную функцию, а не написано сравнением по месту, потому что
/// ошибиться здесь нечем и незаметно: перепутанная строгость сравнения или знак
/// выключили бы предупреждение молча, и дефект вернулся бы ровно в том виде,
/// в каком его нашли, — прогон дойдёт до конца, таблица нарисуется, числа будут
/// выглядеть нормально.
///
/// `SETTLE_SECS=0` — способ отключить предупреждение совсем, и работает он сам
/// собой: длительность у `u64` не бывает меньше нуля. Отдельной ветки `if
/// settle_secs == 0` здесь нет намеренно — она была бы мёртвой, а мёртвое
/// условие читается как несущее и переживает не одну правку.
pub fn is_short_run(test_duration_secs: u64, settle_secs: u64) -> bool {
    test_duration_secs < settle_secs
}

/// Выводит паттерны имён процессов из пути к бинарю.
/// Например "spectre-terminal.exe" -> ["spectre-terminal"].
///
/// ВАЖНО: берём только ПОЛНОЕ имя файла без расширения, а не короткий префикс
/// семейства. Сопоставление в мониторе идёт по подстроке (`contains`) и в конце
/// прогона по этим же паттернам процессы УБИВАЮТСЯ. Короткий общий префикс
/// (например "dev" из "dev-latest") совпал бы с посторонними процессами и
/// прибил бы их. Полное имя ("dev-latest") при этом всё ещё ловит мультиоконный
/// режим ("dev-latest.exe (2)"). Модалки-приложения (например "Spectre Settings")
/// нужно задавать явно через MATCH_PROCESSES в .env.
fn default_match_patterns(app_path: &str) -> Vec<String> {
    std::path::Path::new(app_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect()
}

/// Человекочитаемое имя текущей платформы.
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    }
}

/// Каталог пользовательских данных приложения, в зависимости от ОС.
///   Windows: %LOCALAPPDATA%\spectre-terminal
///   macOS:   ~/Library/Application Support/spectre-terminal
///   Linux:   $XDG_DATA_HOME/spectre-terminal или ~/.local/share/spectre-terminal
fn default_app_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("spectre-terminal"))
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join("Library/Application Support/spectre-terminal"))
    } else {
        // Linux и прочие Unix
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|p| p.join("spectre-terminal"))
    }
}

/// Кросс-платформенный путь по умолчанию к файлу состояния exchanges.db.
fn default_db_path() -> PathBuf {
    default_app_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("exchanges.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Паттерн выводится из полного имени бинаря без расширения и приводится
    /// к нижнему регистру: сопоставление в мониторе регистрозависимо
    /// (`monitor::name_matches`). Пути с прямым слэшем разбираются одинаково
    /// на всех трёх ОС, поэтому проверка общая.
    #[test]
    fn pattern_is_lowercased_file_stem() {
        assert_eq!(default_match_patterns("/opt/spectre/spectre-terminal"), ["spectre-terminal"]);
        assert_eq!(default_match_patterns("/opt/builds/Spectre-Terminal.exe"), ["spectre-terminal"]);
    }

    /// Windows-путь с обратными слэшами — основной случай для этого проекта,
    /// но разбирается он так только на Windows: на Linux и macOS обратный слэш
    /// не разделитель, и весь путь стал бы одним именем файла.
    #[cfg(windows)]
    #[test]
    fn pattern_from_windows_path() {
        assert_eq!(
            default_match_patterns(r"C:\Builds\TD-1055\Spectre-Terminal.exe"),
            ["spectre-terminal"],
        );
    }

    /// Точки в имени сборки принадлежат имени, а не расширению: `file_stem`
    /// отрезает только последнее. Укоротись паттерн до «dev-latest» — под него
    /// попали бы соседние сборки того же семейства, и в конце прогона их убили бы.
    #[test]
    fn dots_in_name_stay_in_pattern() {
        assert_eq!(default_match_patterns("/opt/dev-latest.2026.01.exe"), ["dev-latest.2026.01"]);
        assert_eq!(default_match_patterns("/opt/build.v2/spectre-terminal"), ["spectre-terminal"]);
    }

    /// Дефолтная длительность (10 сек) обязана считаться короткой при дефолтном
    /// пороге — ради этого случая предупреждение и заводилось: на контрольном
    /// прогоне первые 10 секунд дают медиану CPU 19,54 % вместо 11,23 % и
    /// медиану RAM 1 119 MB вместо 1 930 MB, и обе цифры выглядят обычно.
    #[test]
    fn default_duration_is_short_for_default_threshold() {
        assert!(is_short_run(10, DEFAULT_SETTLE_SECS));
    }

    /// Граница: ровно порог — уже НЕ короткий прогон. Сравнение здесь строгое,
    /// иначе человек, выставивший TEST_DURATION_SECS ровно в SETTLE_SECS,
    /// получал бы предупреждение на каждом прогоне и перестал бы его читать.
    #[test]
    fn threshold_itself_is_not_short() {
        assert!(is_short_run(59, 60));
        assert!(!is_short_run(60, 60));
        assert!(!is_short_run(180, 60));
    }

    /// Ноль в пороге — задокументированный способ выключить предупреждение
    /// совсем, и держится он на строгости сравнения, а не на отдельной проверке.
    /// Ослабь её кто-нибудь до `<=` — нулевой порог начал бы предупреждать на
    /// прогоне нулевой длительности, то есть перестал бы означать «выключено».
    #[test]
    fn zero_threshold_turns_warning_off() {
        assert!(!is_short_run(1, 0));
        assert!(!is_short_run(0, 0));
    }

    /// Самый дорогой случай: путь, из которого имени не выводится. Вернуть
    /// отсюда пустую строку нельзя ни при каких обстоятельствах — по этим
    /// паттернам процессы не только считаются, но и УБИВАЮТСЯ, а `contains("")`
    /// истинно для любого имени в системе.
    #[test]
    fn unusable_path_gives_no_patterns() {
        for path in ["", "/", "..", "."] {
            let got = default_match_patterns(path);
            assert!(got.is_empty(), "путь {path:?} дал паттерны {got:?}");
        }
    }
}