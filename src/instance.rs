//! Состояние стенда: запущен ли тестируемый терминал прямо сейчас и на какой
//! схеме базы пойдёт замер.
//!
//! Оба вопроса задаются служебным файлам ЧУЖОГО формата — замку
//! `instance.lock`, записи `instance.json` и таблице `_migrations` внутри
//! `exchanges.db`, — и меняется этот формат без нашего участия. Решение
//! «мерить или не мерить» здесь не принимается: модуль отдаёт состояние, а
//! выбор делает `main.rs`.
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

/// На какой схеме базы стоит стенд — то, что записано в `_migrations`.
///
/// Терминал накатывает миграции ПРИ СТАРТЕ и обратных не хранит. Значит первая
/// сборка прогона-сравнения доводит общую базу до своей версии, а вторая
/// стартует на схеме, которой её код не знает: она может переинициализировать
/// базу с нуля, а может проигнорировать незнакомые таблицы. Прогон при этом
/// дойдёт до конца, таблица нарисуется, числа выйдут правдоподобными — и будут
/// про два РАЗНЫХ стенда, а выглядеть будут как разница сборок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    /// Миграции прочитаны: сколько их всего и какая накатилась последней.
    Applied { count: i64, version: String, name: String },
    /// База есть, а миграций в ней нет: таблицы `_migrations` нет или она
    /// пуста. Так выглядит база, которую терминал ещё ни разу не открывал.
    Fresh,
    /// Файла базы нет вовсе — замер пойдёт с чистого стейта.
    Absent,
    /// Спросить не удалось. Это НЕ «схема не менялась»: разницу между «одинаково»
    /// и «не смогли посмотреть» Правило 6 запрещает стирать.
    Unknown(String),
}

impl Schema {
    /// Одна строка для человека — то, что печатается перед замером.
    ///
    /// Число миграций стоит без согласования по падежу («миграций: 42»)
    /// намеренно: русский счёт требовал бы трёх форм слова, а формулировка
    /// нужна ровно затем, чтобы её сравнили глазами с такой же строкой от
    /// второй сборки.
    pub fn describe(&self) -> String {
        match self {
            Schema::Applied { count, version, name } => {
                format!("миграций: {count}, последняя {version} ({name})")
            }
            Schema::Fresh => "миграций нет — терминал эту базу ещё не открывал".to_string(),
            Schema::Absent => "базы нет — замер пойдёт с чистого стейта".to_string(),
            Schema::Unknown(why) => format!("прочитать не удалось ({why})"),
        }
    }

    /// Разные ли это два состояния стенда.
    ///
    /// Ответ троичный намеренно. `None` — «сравнить нечем»: хотя бы одно
    /// состояние не прочиталось, и выдать это за «ничего не изменилось» значило
    /// бы придумать факт. Ровно та ошибка, ради которой заведено Правило 6:
    /// молчание проверки неотличимо от её успеха.
    pub fn differs_from(&self, other: &Schema) -> Option<bool> {
        match (self, other) {
            (Schema::Unknown(_), _) | (_, Schema::Unknown(_)) => None,
            _ => Some(self != other),
        }
    }
}

/// Спрашивает у базы, какие миграции на ней накатаны.
///
/// **Открывается строго на чтение.** Права записи здесь не нужны и вредны:
/// SQLite, открывший базу в режиме WAL на запись, сам сделает восстановление и
/// checkpoint, то есть харнесс доделает за убитый терминал его работу и отдаст
/// следующей сборке подготовленный кем-то стенд вместо того, который мерил
/// (Правило 2, README: базу мы не создаём и не меняем).
///
/// Совсем «не трогать каталог» при этом не выходит, и полагаться на это нельзя:
/// служебный индекс `exchanges.db-shm` SQLite заводит рядом с базой и для
/// ЧИТАТЕЛЯ — проверено, файл появляется. Данных в нём нет, терминал создаёт
/// его сам при каждом старте, и на замер это не влияет; неизменными остаются
/// сама база и её WAL, что и проверяется тестом.
///
/// А вот убрать и `-shm`, открыв базу как `immutable=1`, — ловушка: этот режим
/// велит SQLite считать файл неизменным и ИГНОРИРОВАТЬ WAL. Проверено: на базе,
/// где таблица миграций живёт в WAL, ответ становится «миграций нет» — вид
/// чистого стенда там, где стенд накатан. Ошибки при этом нет никакой.
///
/// **Читать только до старта и после убийства.** Запрос дешёвый (открытие плюс
/// два `SELECT`, миллисекунды) и стоит вне цикла семплинга, но конкурировать
/// с работающим терминалом за те же страницы во время замера всё равно нельзя.
///
/// **Последняя миграция берётся по `rowid`, а не по `max(version)`.** Версия
/// объявлена как TEXT, и написаны версии в двух разных стилях сразу — `m031` и
/// `m20260819_204500`. Лексикографический максимум по ним отдал бы дату 2026
/// года как «последнюю» на любой базе, где такая миграция вообще есть, и число
/// перестало бы меняться от прогона к прогону — молчаливый отказ проверки,
/// выглядящий как её успех. Порядок вставки (`rowid`) отвечает на нужный
/// вопрос: какая миграция накатилась последней.
pub fn schema(db_path: &Path) -> Schema {
    // Отсутствие базы — ответ, а не ошибка: это другой режим замера, о котором
    // харнесс говорит отдельно. Открывать (и тем более создавать) тут нечего.
    if !db_path.exists() {
        return Schema::Absent;
    }

    let conn = match rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(conn) => conn,
        Err(e) => return Schema::Unknown(format!("{}: {e}", db_path.display())),
    };

    // Таблицу спрашиваем отдельно: без неё запрос ниже вернул бы обычную ошибку
    // SQLite, и отличить «терминал эту базу не открывал» от «база испорчена»
    // пришлось бы по тексту сообщения — то есть ненадёжно.
    let has_table: i64 = match conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_migrations'",
        [],
        |row| row.get(0),
    ) {
        Ok(n) => n,
        Err(e) => return Schema::Unknown(format!("{}: {e}", db_path.display())),
    };
    if has_table == 0 {
        return Schema::Fresh;
    }

    match conn.query_row(
        "SELECT (SELECT COUNT(*) FROM _migrations), version, name \
         FROM _migrations ORDER BY rowid DESC LIMIT 1",
        [],
        |row| {
            Ok(Schema::Applied {
                count: row.get(0)?,
                version: row.get(1)?,
                name: row.get(2)?,
            })
        },
    ) {
        Ok(schema) => schema,
        // Таблица есть, а записей нет: терминал её завёл и упал на первой же
        // миграции. Для нашего вопроса это то же самое, что и пустая база.
        Err(rusqlite::Error::QueryReturnedNoRows) => Schema::Fresh,
        Err(e) => Schema::Unknown(format!("{}: {e}", db_path.display())),
    }
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

    /// Такой же `_migrations`, какой заводит терминал, с записями в порядке
    /// накатывания. Соединение закрывается сразу: база должна читаться как
    /// оставшаяся от завершившегося приложения.
    fn make_db(dir: &Path, rows: &[(&str, &str)]) -> PathBuf {
        let path = dir.join("exchanges.db");
        let conn = rusqlite::Connection::open(&path).expect("фикстура: база не создалась");
        // Режим тот же, в котором терминал держит настоящую базу: он меняет и
        // то, как база читается, и то, какие файлы появляются рядом с ней.
        conn.pragma_update(None, "journal_mode", "wal").expect("фикстура: WAL не включился");
        conn.execute_batch(
            "CREATE TABLE _migrations (
                 version TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 applied_at DATETIME DEFAULT CURRENT_TIMESTAMP
             )",
        )
        .expect("фикстура: таблица не создалась");
        for (version, name) in rows {
            conn.execute("INSERT INTO _migrations (version, name) VALUES (?1, ?2)", [version, name])
                .expect("фикстура: запись не вставилась");
        }
        path
    }

    /// Последней считается ПОСЛЕДНЯЯ НАКАТАННАЯ, а не наибольшая по алфавиту.
    ///
    /// Версии объявлены как TEXT и написаны в двух стилях сразу — так они лежат
    /// в настоящей базе. Возьми проверка `max(version)`, как просилось на
    /// первый взгляд, она вечно показывала бы `m20260819_204500` и не менялась
    /// бы от прогона к прогону: отказ, неотличимый от «схема не тронута».
    #[test]
    fn last_migration_is_the_last_applied_not_the_alphabetical_max() {
        let dir = temp_dir("schema-order");
        let db = make_db(
            &dir,
            &[
                ("m031", "create_screenings_table"),
                ("m20260819_204500", "drop_legacy_credential_columns"),
                ("m040", "seed_signal_level_hotkey"),
            ],
        );

        assert_eq!(
            schema(&db),
            Schema::Applied {
                count: 3,
                version: "m040".to_string(),
                name: "seed_signal_level_hotkey".to_string(),
            }
        );
    }

    /// Главный случай ради которого всё это и читается: накатанная миграция
    /// обязана быть ВИДНА как другое состояние стенда.
    #[test]
    fn one_more_migration_is_a_different_stand() {
        let before = Schema::Applied {
            count: 42,
            version: "m040".to_string(),
            name: "seed_signal_level_hotkey".to_string(),
        };
        let after = Schema::Applied {
            count: 43,
            version: "m041".to_string(),
            name: "add_alerts_table".to_string(),
        };

        assert_eq!(before.differs_from(&after), Some(true));
        assert_eq!(before.differs_from(&before.clone()), Some(false));
    }

    /// Непрочитанное состояние не сравнивается ни с чем и НЕ выдаётся за
    /// «одинаково»: иначе сломавшаяся проверка выглядела бы как успешная —
    /// прогон молча сообщал бы, что схему никто не трогал.
    #[test]
    fn unreadable_state_is_never_called_equal() {
        let unknown = Schema::Unknown("нет прав".to_string());
        assert_eq!(unknown.differs_from(&Schema::Fresh), None);
        assert_eq!(Schema::Fresh.differs_from(&unknown), None);
        assert_eq!(unknown.differs_from(&unknown.clone()), None);
        // А вот эти три состояния между собой различимы и сравниваются молча.
        assert_eq!(Schema::Fresh.differs_from(&Schema::Absent), Some(true));
    }

    /// Стенд в том виде, в каком его оставляет ЖЁСТКО УБИТЫЙ терминал: база в
    /// режиме WAL, рядом несведённый `exchanges.db-wal` с последними записями и
    /// ни одного живого соединения.
    ///
    /// Собрать такое состояние прямо нельзя: SQLite сводит WAL в базу и убирает
    /// его, когда закрывается последнее соединение. Поэтому каталог копируется,
    /// ПОКА писатель жив, — копия и оказывается брошенной на полуслове.
    fn stand_left_after_a_kill(tag: &str) -> PathBuf {
        let live = temp_dir(&format!("{tag}-live"));
        let path = live.join("exchanges.db");
        let writer = rusqlite::Connection::open(&path).expect("фикстура: база не создалась");
        writer.pragma_update(None, "journal_mode", "wal").expect("фикстура: WAL не включился");
        writer
            .execute_batch(
                "CREATE TABLE _migrations (version TEXT PRIMARY KEY, name TEXT NOT NULL);
                 INSERT INTO _migrations (version, name) VALUES ('m001', 'init');",
            )
            .expect("фикстура: миграция не записалась");

        let dir = temp_dir(tag);
        for entry in std::fs::read_dir(&live).expect("фикстура: каталог не читается") {
            let entry = entry.expect("фикстура: запись каталога");
            std::fs::copy(entry.path(), dir.join(entry.file_name())).expect("фикстура: копия");
        }
        drop(writer);

        let wal = std::fs::metadata(dir.join("exchanges.db-wal")).expect("фикстура: WAL не скопирован");
        assert!(wal.len() > 0, "фикстура: WAL пуст, состояние после убийства не воспроизведено");
        dir
    }

    /// База в режиме WAL: свежие записи лежат в `exchanges.db-wal`, а не в самом
    /// файле — на настоящем стенде это 4 МБ данных мимо базы. Читай проверка
    /// файл базы сама, мимо SQLite, она показала бы состояние ДО последних
    /// миграций: число есть, выглядит обычно и не меняется от прогона к прогону.
    #[test]
    fn migrations_living_in_the_wal_are_still_read() {
        let dir = stand_left_after_a_kill("schema-wal");

        assert_eq!(
            schema(&dir.join("exchanges.db")),
            Schema::Applied { count: 1, version: "m001".to_string(), name: "init".to_string() }
        );
    }

    /// База, которую терминал ещё не открывал, и база без единой накатанной
    /// миграции — для нашего вопроса одно и то же состояние.
    #[test]
    fn database_without_applied_migrations_is_fresh() {
        let no_table = temp_dir("schema-no-table");
        let path = no_table.join("exchanges.db");
        rusqlite::Connection::open(&path)
            .expect("фикстура: база не создалась")
            .execute_batch("CREATE TABLE settings (k TEXT)")
            .expect("фикстура: таблица не создалась");
        assert_eq!(schema(&path), Schema::Fresh, "база без таблицы _migrations");

        let empty = temp_dir("schema-empty");
        assert_eq!(schema(&make_db(&empty, &[])), Schema::Fresh, "пустая таблица _migrations");
    }

    /// Нет файла — нет и вопроса: это штатный режим «чистый стейт», а не отказ.
    #[test]
    fn missing_database_is_absent() {
        let dir = temp_dir("schema-none");
        assert_eq!(schema(&dir.join("exchanges.db")), Schema::Absent);
    }

    /// А вот испорченный файл — именно отказ, и притвориться чистым стейтом он
    /// не имеет права: «базы нет» и «базу не прочитать» ведут к разным выводам
    /// о том, что показал замер.
    #[test]
    fn unreadable_database_is_not_a_clean_stand() {
        let dir = temp_dir("schema-garbage");
        let path = dir.join("exchanges.db");
        std::fs::write(&path, b"not a database at all").expect("фикстура");

        assert!(matches!(schema(&path), Schema::Unknown(_)), "мусорный файл прочитан как состояние");
    }

    /// Опрос не сводит WAL в базу и не переписывает её — держится это на одном
    /// флаге открытия, и проверяется именно он.
    ///
    /// Стенд взят в состоянии после жёсткого убийства: несведённый WAL и ни
    /// одного живого соединения. Соединение С ПРАВОМ ЗАПИСИ на таком стенде
    /// восстановит журнал и при закрытии сделает checkpoint — то есть харнесс
    /// доделает за терминал работу, которую тот не успел, и следующая сборка
    /// стартует на подготовленной кем-то базе. Ошибки при этом не будет ни
    /// одной, а разница уедет в отчёт как разница сборок.
    ///
    /// Служебный индекс `-shm` из проверки исключён намеренно: SQLite заводит
    /// его и для читателя (проверено — файл появляется), данных в нём нет, и
    /// требовать его отсутствия значило бы требовать невозможного.
    #[test]
    fn reading_the_schema_changes_nothing() {
        fn state(path: &Path) -> (u64, std::time::SystemTime) {
            let meta = std::fs::metadata(path).expect("файл стенда пропал");
            (meta.len(), meta.modified().expect("время правки"))
        }

        let dir = stand_left_after_a_kill("schema-readonly");
        let db = dir.join("exchanges.db");
        let wal = dir.join("exchanges.db-wal");
        let (db_before, wal_before) = (state(&db), state(&wal));

        let _ = schema(&db);

        assert_eq!(state(&db), db_before, "опрос переписал саму базу");
        assert_eq!(state(&wal), wal_before, "опрос свёл WAL — стенд достался следующей сборке другим");
    }
}
