//! Состояние стенда: запущен ли тестируемый терминал прямо сейчас.
//!
//! Терминал одноэкземплярный, и второй запуск НЕ становится вторым процессом:
//! он видит занятый замок, просит живой экземпляр показаться и выходит.
//! ЗАМЕРЕНО 06.09.2026 на этой машине: второй процесс умирает через 0,11 с
//! с кодом возврата 0. То есть для харнесса это выглядит удачным запуском —
//! `spawn` вернул PID, ошибки нет, — а дальше `collect_app_metrics` ловит по
//! совпадению имени УЖЕ РАБОТАВШИЙ экземпляр и меряет его: чужую сборку,
//! возможно прогретую час назад. В конце прогона `terminate_app` закрыл бы её
//! по тому же совпадению имени, вместе с несохранённым. Ни одной ошибки при
//! этом не будет, а отчёт выйдет правдоподобным — Правило 6 в чистом виде.
//!
//! Спрашиваем ровно то же и ровно тем же способом, что и сам терминал: пробуем
//! взять эксклюзивный замок на `instance.lock` рядом с базой. `File::try_lock`
//! из std на Windows разворачивается в тот же
//! `LockFileEx(EXCLUSIVE | FAIL_IMMEDIATELY, 0, u32::MAX, u32::MAX)`, на Unix —
//! в тот же `flock(LOCK_EX | LOCK_NB)`, которыми терминал и держит слот.
//! Поэтому здесь нет ни одной `#[cfg]`-ветки: одинаковый вопрос, одинаковый
//! ответ на всех трёх ОС.
//!
//! **Почему не pid из `instance.json`.** Он там есть, но признаком жизни не
//! является — так написано в исходниках терминала («nothing here trusts a file
//! to mean a process is alive») и так проверено опытом: харнесс убивает
//! приложение жёстко, и после этого `instance.lock` и `instance.json` остаются
//! лежать на диске с мёртвым pid. Проверка «файл есть — значит запущен»
//! запретила бы все прогоны подряд, а проверка «процесс с этим pid существует»
//! соврала бы на переиспользованном номере. У замка таких состояний нет вовсе:
//! ОС снимает его при смерти процесса любым способом, включая `TerminateProcess`
//! и отключение питания. `instance.json` поэтому читается ТОЛЬКО чтобы назвать
//! человеку, кто держит замок, и на решение не влияет.

use std::fs::{OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Чем представился живой экземпляр — содержимое `instance.json`.
///
/// Формат чужой, поэтому разбирается терпимо: неизвестные поля `serde`
/// пропускает сам, а отсутствующий `http_port` читается как `None` — это
/// поведение самого `Option`, и `#[serde(default)]` здесь НЕ нужен (проверено
/// мутацией: со снятым атрибутом тесты остаются зелёными). Требование Правила 2
/// помечать новые поля `default` этим не отменяется — оно про поля, у которых
/// нет такого поведения даром: `String`, числа, перечисления в `TestResult`.
///
/// Цена ошибки разбора тут маленькая и односторонняя: не разобравшаяся запись
/// превращает понятную диагностику в «экземпляр не назвался», но на решение
/// мерить или не мерить не влияет — его принимает замок.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InstanceInfo {
    /// PID живого экземпляра. Нужен только для сообщения человеку.
    pub pid: u32,
    /// Порт локального HTTP-сервера. `None`, пока сервер ещё поднимается.
    pub http_port: Option<u16>,
}

/// Что стенд ответил на вопрос «терминал уже запущен?».
pub enum Stand {
    /// Замка нет или он свободен — запускать можно.
    Free,
    /// Замок держит живой экземпляр. Внутри — то, чем он представился, если
    /// `instance.json` на месте и разобрался.
    Busy(Option<InstanceInfo>),
    /// Спросить не удалось (нет прав, каталог на сетевой шаре без блокировок).
    /// Это НЕ «свободно»: разница между «никого нет» и «не смогли посмотреть» —
    /// ровно то, что Правило 6 запрещает стирать.
    Unknown(String),
}

/// Путь к замку одноэкземплярности — рядом с базой.
///
/// Каталог тот же, что и у `exchanges.db`, и это не совпадение: замок защищает
/// именно базу, поэтому терминал кладёт его туда же, куда её. Значит и наш
/// `DB_PATH` из `.env` указывает на нужный каталог сам собой.
pub fn lock_path(db_path: &Path) -> PathBuf {
    beside(db_path, "instance.lock")
}

/// Путь к записи живого экземпляра — там же.
pub fn info_path(db_path: &Path) -> PathBuf {
    beside(db_path, "instance.json")
}

fn beside(db_path: &Path, name: &str) -> PathBuf {
    db_path.parent().unwrap_or_else(|| Path::new(".")).join(name)
}

/// Спрашивает у ОС, держит ли кто-нибудь замок одноэкземплярности.
pub fn probe(db_path: &Path) -> Stand {
    let path = lock_path(db_path);

    // Открываем БЕЗ `create`: харнесс меряет, а не готовит стенд. Отсутствие
    // файла и есть ответ — на этом каталоге терминал ещё не запускался.
    let file = match OpenOptions::new().read(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Stand::Free,
        Err(e) => return Stand::Unknown(format!("{}: {e}", path.display())),
    };

    match file.try_lock() {
        // Замок свободен — и мы немедленно его отпускаем. Держать его нельзя
        // ни секунды: пока он наш, терминал, который мы вот-вот запустим, сам
        // окажется «вторым экземпляром» и выйдет, — то есть проверка сломала бы
        // ровно тот замер, который защищает. Закрытие файла снимает замок и
        // само по себе, `unlock` здесь стоит ради явности намерения.
        Ok(()) => {
            let _ = file.unlock();
            Stand::Free
        }
        Err(TryLockError::WouldBlock) => Stand::Busy(read_info(&info_path(db_path))),
        Err(TryLockError::Error(e)) => Stand::Unknown(format!("{}: {e}", path.display())),
    }
}

/// Читает `instance.json`. `None` — файла нет или он не разобрался.
///
/// Отсутствие и порча одинаково означают «экземпляр не назвался» и НИЧЕГО не
/// говорят о том, запущен ли он: это решает замок, а не файл.
pub fn read_info(path: &Path) -> Option<InstanceInfo> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Отдельный каталог на каждый тест: замок — вещь общая для процесса,
    /// и тесты, поделившие один файл, мешали бы друг другу.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spectre-automation-stand-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("не удалось создать временный каталог");
        dir
    }

    /// Оба служебных файла ищутся В КАТАЛОГЕ БАЗЫ, а не рядом с бинарём и не
    /// в текущем каталоге. Промахнись путь — проверка молча отвечала бы
    /// «свободно» на любом прогоне, то есть выключилась бы целиком.
    #[test]
    fn service_files_are_looked_for_next_to_the_database() {
        let db = Path::new("/data/spectre-terminal/exchanges.db");
        assert_eq!(lock_path(db), Path::new("/data/spectre-terminal/instance.lock"));
        assert_eq!(info_path(db), Path::new("/data/spectre-terminal/instance.json"));
    }

    /// Путь без каталога не должен уводить проверку в корень или в панику:
    /// `DB_PATH` человек задаёт руками, и относительный путь там возможен.
    #[test]
    fn bare_database_name_stays_in_the_current_directory() {
        assert_eq!(lock_path(Path::new("exchanges.db")), Path::new("instance.lock"));
    }

    /// Ровно те байты, которые терминал написал на этой машине 06.09.2026.
    #[test]
    fn published_record_is_read_back() {
        let dir = temp_dir("info");
        let path = dir.join("instance.json");
        std::fs::write(&path, br#"{"pid":1404,"http_port":59428}"#).expect("фикстура");

        assert_eq!(
            read_info(&path),
            Some(InstanceInfo { pid: 1404, http_port: Some(59428) })
        );
    }

    /// Запись без порта — не порча, а штатное состояние: терминал пишет её
    /// сразу после взятия замка, до подъёма HTTP-сервера. Перестань она
    /// читаться — самое интересное окно (первые доли секунды чужого запуска)
    /// осталось бы без диагностики.
    ///
    /// Обе формы проверяются намеренно: `null` — то, что терминал пишет
    /// сегодня, отсутствующий ключ — то, чем это может стать. Обе держатся на
    /// одном лишь типе `Option`, безо всяких атрибутов; проверка нужна потому,
    /// что формат чужой и меняется без нашего участия.
    #[test]
    fn record_without_a_port_is_still_a_record() {
        let dir = temp_dir("info-no-port");
        let expected = Some(InstanceInfo { pid: 1404, http_port: None });

        let explicit = dir.join("null.json");
        std::fs::write(&explicit, br#"{"pid":1404,"http_port":null}"#).expect("фикстура");
        assert_eq!(read_info(&explicit), expected, "запись с явным null не прочиталась");

        let missing = dir.join("missing.json");
        std::fs::write(&missing, br#"{"pid":1404}"#).expect("фикстура");
        assert_eq!(read_info(&missing), expected, "запись без ключа http_port не прочиталась");
    }

    /// Обрывок файла и отсутствие файла означают одно и то же — «не назвался», —
    /// и ни то, ни другое не имеет права уронить прогон.
    #[test]
    fn broken_or_missing_record_reads_as_absent() {
        let dir = temp_dir("info-broken");
        let broken = dir.join("instance.json");
        std::fs::write(&broken, b"{\"pid\": ").expect("фикстура");

        assert_eq!(read_info(&broken), None);
        assert_eq!(read_info(&dir.join("does-not-exist.json")), None);
    }

    /// Главное, ради чего заведён замок вместо проверки pid: файл, оставшийся
    /// от жёстко убитого процесса (а харнесс убивает именно так), — это
    /// СВОБОДНЫЙ стенд. Прочитай проверка существование файла — она запретила бы
    /// все прогоны подряд начиная со следующего.
    #[test]
    fn leftover_lock_file_is_a_free_stand() {
        let dir = temp_dir("stale");
        std::fs::write(dir.join("instance.lock"), b"").expect("фикстура");
        std::fs::write(dir.join("instance.json"), br#"{"pid":1404,"http_port":59428}"#)
            .expect("фикстура");

        assert!(matches!(probe(&dir.join("exchanges.db")), Stand::Free));
    }

    /// Занятый замок обязан читаться как занятый, и человеку обязано достаться
    /// имя того, кто его держит. Замок берётся здесь же, в тестовом процессе:
    /// и `LockFileEx`, и `flock` разводят по разным ОТКРЫТЫМ ФАЙЛАМ, а не по
    /// процессам, поэтому проверка честная и не требует второго процесса.
    #[test]
    fn held_lock_is_seen_as_busy() {
        let dir = temp_dir("busy");
        let holder = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("instance.lock"))
            .expect("фикстура");
        holder.try_lock().expect("замок должен браться на свободном файле");
        std::fs::write(dir.join("instance.json"), br#"{"pid":1404,"http_port":59428}"#)
            .expect("фикстура");

        let Stand::Busy(info) = probe(&dir.join("exchanges.db")) else {
            panic!("занятый замок прочитан как свободный стенд");
        };
        assert_eq!(info, Some(InstanceInfo { pid: 1404, http_port: Some(59428) }));

        let _ = holder.unlock();
    }

    /// Занятый замок остаётся занятым, даже если сказать о себе некому:
    /// `instance.json` мог не успеть появиться или быть стёрт. Решает замок.
    #[test]
    fn busy_without_a_record_is_still_busy() {
        let dir = temp_dir("busy-mute");
        let holder = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("instance.lock"))
            .expect("фикстура");
        holder.try_lock().expect("замок должен браться на свободном файле");

        assert!(matches!(probe(&dir.join("exchanges.db")), Stand::Busy(None)));

        let _ = holder.unlock();
    }

    /// Проверка НИЧЕГО не создаёт: харнесс меряет, а не готовит стенд. Появись
    /// здесь `create(true)` — харнесс сам разложил бы по каталогу базы файлы
    /// приложения, которое ещё ни разу не запускалось.
    #[test]
    fn probing_creates_nothing() {
        let dir = temp_dir("empty");

        assert!(matches!(probe(&dir.join("exchanges.db")), Stand::Free));
        assert!(!dir.join("instance.lock").exists(), "проверка создала замок");
    }
}
