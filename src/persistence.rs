//! `janis.db` — the app's single SQLite store (user preferences + track
//! library). Opened once at boot in the per-user data directory (resolved by
//! `platform::data_directory`, cross-platform by construction) and shared
//! behind a mutex: every call is a short read or write, so one connection is
//! plenty and keeps SQLite's locking model trivial.

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use crate::audio::EQ_BAND_COUNT;

/// The file `init` opens in the data directory.
const DATABASE_FILE: &str = "janis.db";

pub struct DbState(Mutex<Connection>);

impl DbState {
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().expect("janis.db mutex poisoned")
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS user_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    volume REAL NOT NULL DEFAULT 0.8,
    eq_gains TEXT NOT NULL DEFAULT '[0,0,0,0,0,0,0,0,0,0]',
    eq_preset TEXT NOT NULL DEFAULT 'flat',
    gapless INTEGER NOT NULL DEFAULT 1,
    crossfade INTEGER NOT NULL DEFAULT 1,
    normalize INTEGER NOT NULL DEFAULT 1,
    exclusive INTEGER NOT NULL DEFAULT 0,
    language TEXT NOT NULL DEFAULT 'en',
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS watched_folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    added_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    -- NULL for ad-hoc single-file imports (Add music / drag-and-drop);
    -- otherwise the watched folder the scanner found the file under.
    folder_id INTEGER REFERENCES watched_folders(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    composer TEXT,
    duration_secs REAL NOT NULL DEFAULT 0,
    format TEXT NOT NULL,
    sample_rate INTEGER,
    bit_depth INTEGER,
    channels INTEGER,
    lossless INTEGER NOT NULL DEFAULT 0,
    added_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_tracks_folder ON tracks(folder_id);
CREATE INDEX IF NOT EXISTS idx_tracks_added ON tracks(added_at DESC);
";

/// `SCHEMA` above is frozen at version 1 — it only ever creates a fresh file.
/// Every change since is a migration, so a new database and an existing one
/// converge on the same shape.
const SCHEMA_VERSION: i64 = 6;

/// Index `i` upgrades to version `i + 2`. Append only: editing a shipped entry
/// would skip the change for anyone who already ran it.
const MIGRATIONS: &[&str] = &[
    // → 2: the tag fields the scanner reads beyond title/artist/album, so
    //      albums can be shown in their own running order.
    "ALTER TABLE tracks ADD COLUMN track_number INTEGER;
     ALTER TABLE tracks ADD COLUMN track_total INTEGER;
     ALTER TABLE tracks ADD COLUMN disc_number INTEGER;
     ALTER TABLE tracks ADD COLUMN disc_total INTEGER;
     ALTER TABLE tracks ADD COLUMN year INTEGER;
     ALTER TABLE tracks ADD COLUMN genre TEXT;
     ALTER TABLE tracks ADD COLUMN album_artist TEXT;",
    // → 3: what each track's playback gain is worked out from. ReplayGain
    //      columns come from the file's tags; the loudness pair is measured
    //      when a track without tags is played end to end. NULL throughout
    //      means "not known", which is what keeps an unmeasured track
    //      distinguishable from one that genuinely needs no adjustment.
    "ALTER TABLE tracks ADD COLUMN rg_track_gain_db REAL;
     ALTER TABLE tracks ADD COLUMN rg_track_peak REAL;
     ALTER TABLE tracks ADD COLUMN rg_album_gain_db REAL;
     ALTER TABLE tracks ADD COLUMN rg_album_peak REAL;
     ALTER TABLE tracks ADD COLUMN loudness_lufs REAL;
     ALTER TABLE tracks ADD COLUMN loudness_peak REAL;",
    // → 4: whether the equalizer runs in its linear-phase (FIR) mode. Off by
    //      default: the zero-latency realtime EQ is what a player should do
    //      out of the box, and the FIR mode trades ~43 ms of latency for it.
    "ALTER TABLE user_preferences ADD COLUMN eq_linear_phase INTEGER NOT NULL DEFAULT 0;",
    // → 5: the stations the listener starred, by their catalog id. A row is a
    //      favourite; no row is the default. The id is not checked against the
    //      catalog, so a station that leaves it and comes back keeps its star.
    "CREATE TABLE IF NOT EXISTS favourite_stations (
         station_id TEXT PRIMARY KEY,
         added_at INTEGER NOT NULL DEFAULT (unixepoch())
     );",
    // → 6: a Jellyfin server whose songs join the library. The server row
    //      keeps the access token, never the password. A server track points at
    //      its server, so signing out removes its songs with it, and names the
    //      item whose picture is its cover. The device identifier is what
    //      Jellyfin's authorization header asks a client to send.
    "CREATE TABLE IF NOT EXISTS media_servers (
         id INTEGER PRIMARY KEY,
         kind TEXT NOT NULL DEFAULT 'jellyfin',
         address TEXT NOT NULL,
         server_identifier TEXT NOT NULL,
         server_name TEXT NOT NULL DEFAULT '',
         user_identifier TEXT NOT NULL,
         user_name TEXT NOT NULL,
         access_token TEXT NOT NULL,
         last_sync_at INTEGER,
         added_at INTEGER NOT NULL DEFAULT (unixepoch())
     );
     ALTER TABLE tracks ADD COLUMN server_id INTEGER REFERENCES media_servers(id) ON DELETE CASCADE;
     ALTER TABLE tracks ADD COLUMN artwork_item TEXT;
     ALTER TABLE user_preferences ADD COLUMN device_identifier TEXT;
     CREATE INDEX IF NOT EXISTS idx_tracks_server ON tracks(server_id);",
];

/// Opens (creating if needed) `janis.db` under the app-data directory and
/// brings it up to date. Called once at boot.
pub fn init(app_data_dir: PathBuf) -> Result<DbState, String> {
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("create app data dir {}: {}", app_data_dir.display(), e))?;
    let db_path = app_data_dir.join(DATABASE_FILE);
    let conn =
        Connection::open(&db_path).map_err(|e| format!("open {}: {}", db_path.display(), e))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("enable foreign keys: {}", e))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("apply schema: {}", e))?;
    migrate(&conn)?;
    conn.execute("INSERT OR IGNORE INTO user_preferences (id) VALUES (1)", [])
        .map_err(|e| format!("seed preferences row: {}", e))?;
    Ok(DbState(Mutex::new(conn)))
}

/// Applies any migration the file has not seen, tracked in SQLite's own
/// `user_version`. The version stamp is written *inside* each migration's
/// transaction — `user_version` lives in the database header and is fully
/// transactional — so DDL and stamp commit or roll back together. Stamping
/// afterwards would leave a crash window where the `ALTER TABLE`s had
/// committed but the version had not; the non-idempotent re-run then fails
/// with "duplicate column name" on every launch and the app never boots.
fn migrate(conn: &Connection) -> Result<(), String> {
    let mut version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| format!("read schema version: {}", e))?;

    // Zero means the file predates migrations, which is exactly the shape
    // `SCHEMA` produces — the version it would have been stamped with.
    if version == 0 {
        version = 1;
    }

    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let target = index as i64 + 2;
        if version >= target {
            continue;
        }
        let result = conn.execute_batch(&format!(
            "BEGIN; {} PRAGMA user_version = {}; COMMIT;",
            sql, target
        ));
        if let Err(e) = result {
            // The failed batch never reached its COMMIT, so its transaction
            // is still open; close it before touching the file again.
            let _ = conn.execute_batch("ROLLBACK;");
            // A database whose columns are present with the version behind
            // (a migration applied but never stamped) fails exactly here.
            // The migrations are append-only ALTERs, so a duplicate column
            // proves this one already ran — adopt it rather than refuse to
            // boot forever.
            if e.to_string().contains("duplicate column name") {
                conn.pragma_update(None, "user_version", target)
                    .map_err(|e| format!("stamp schema version {}: {}", target, e))?;
            } else {
                return Err(format!("migration to v{}: {}", target, e));
            }
        }
        version = target;
    }

    debug_assert_eq!(
        version, SCHEMA_VERSION,
        "MIGRATIONS must reach SCHEMA_VERSION"
    );
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub volume: f64,
    pub eq_gains: Vec<f64>,
    pub eq_preset: String,
    pub eq_linear_phase: bool,
    pub gapless: bool,
    pub crossfade: bool,
    pub normalize: bool,
    pub exclusive: bool,
    pub language: String,
}

/// The four playback switches, as a closed enum so the column a write can
/// touch is an allowlist rather than an interpolated string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackOption {
    Gapless,
    Crossfade,
    Normalize,
    Exclusive,
}

impl PlaybackOption {
    fn column(self) -> &'static str {
        match self {
            PlaybackOption::Gapless => "gapless",
            PlaybackOption::Crossfade => "crossfade",
            PlaybackOption::Normalize => "normalize",
            PlaybackOption::Exclusive => "exclusive",
        }
    }
}

pub fn get_preferences(db: &DbState) -> Result<Preferences, String> {
    let conn = db.lock();
    conn.query_row(
        "SELECT volume, eq_gains, eq_preset, eq_linear_phase, gapless, crossfade, normalize,
                exclusive, language
         FROM user_preferences WHERE id = 1",
        [],
        |row| {
            let gains_json: String = row.get(1)?;
            Ok(Preferences {
                volume: row.get(0)?,
                eq_gains: serde_json::from_str(&gains_json)
                    .unwrap_or_else(|_| vec![0.0; EQ_BAND_COUNT]),
                eq_preset: row.get(2)?,
                eq_linear_phase: row.get::<_, i64>(3)? != 0,
                gapless: row.get::<_, i64>(4)? != 0,
                crossfade: row.get::<_, i64>(5)? != 0,
                normalize: row.get::<_, i64>(6)? != 0,
                exclusive: row.get::<_, i64>(7)? != 0,
                language: row.get(8)?,
            })
        },
    )
    .map_err(|e| format!("read preferences: {}", e))
}

pub fn set_volume(db: &DbState, volume: f64) -> Result<(), String> {
    let volume = volume.clamp(0.0, 1.0);
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET volume = ?1, updated_at = unixepoch() WHERE id = 1",
        [volume],
    )
    .map_err(|e| format!("persist volume: {}", e))?;
    Ok(())
}

pub fn set_eq(db: &DbState, gains: &[f64], preset: &str) -> Result<(), String> {
    if gains.len() != EQ_BAND_COUNT {
        return Err(format!(
            "expected {} EQ gains, got {}",
            EQ_BAND_COUNT,
            gains.len()
        ));
    }
    // The equalizer bounds the gains; they are stored as it hands them over.
    let gains_json = serde_json::to_string(gains).map_err(|e| e.to_string())?;
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET eq_gains = ?1, eq_preset = ?2, updated_at = unixepoch()
         WHERE id = 1",
        rusqlite::params![gains_json, preset],
    )
    .map_err(|e| format!("persist eq: {}", e))?;
    Ok(())
}

/// The equalizer's quality mode. Separate from `set_eq` because the gains are
/// persisted debounced (a drag emits dozens a second) while this is one
/// discrete switch.
pub fn set_eq_linear_phase(db: &DbState, enabled: bool) -> Result<(), String> {
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET eq_linear_phase = ?1, updated_at = unixepoch() WHERE id = 1",
        [enabled as i64],
    )
    .map_err(|e| format!("persist eq linear phase: {}", e))?;
    Ok(())
}

pub fn set_playback_option(
    db: &DbState,
    option: PlaybackOption,
    enabled: bool,
) -> Result<(), String> {
    let conn = db.lock();
    conn.execute(
        &format!(
            "UPDATE user_preferences SET {} = ?1, updated_at = unixepoch() WHERE id = 1",
            option.column()
        ),
        [enabled as i64],
    )
    .map_err(|e| format!("persist playback option: {}", e))?;
    Ok(())
}

/// `language` is a code `translations::Language` names; an unknown one reads
/// back as English.
pub fn set_language(db: &DbState, language: &str) -> Result<(), String> {
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET language = ?1, updated_at = unixepoch() WHERE id = 1",
        [language],
    )
    .map_err(|e| format!("persist language: {}", e))?;
    Ok(())
}

/// The ids of the stations the listener starred.
pub fn favourite_station_ids(database: &DbState) -> Result<Vec<String>, String> {
    let connection = database.lock();
    let mut statement = connection
        .prepare("SELECT station_id FROM favourite_stations")
        .map_err(|error| format!("read favourite stations: {error}"))?;
    let station_ids = statement
        .query_map([], |row| row.get(0))
        .and_then(|rows| rows.collect())
        .map_err(|error| format!("read favourite stations: {error}"))?;
    Ok(station_ids)
}

/// Stars a station or takes its star away. Saving the state it already has
/// changes nothing.
pub fn set_favourite_station(
    database: &DbState,
    station_id: &str,
    favourite: bool,
) -> Result<(), String> {
    let connection = database.lock();
    let statement = if favourite {
        "INSERT OR IGNORE INTO favourite_stations (station_id) VALUES (?1)"
    } else {
        "DELETE FROM favourite_stations WHERE station_id = ?1"
    };
    connection
        .execute(statement, [station_id])
        .map_err(|error| format!("persist favourite station: {error}"))?;
    Ok(())
}

/// A Jellyfin server the listener signed in to, as `janis.db` keeps it.
#[derive(Clone, PartialEq)]
pub struct MediaServer {
    pub id: i64,
    /// The address the listener typed, normalised (`http://host:8096`).
    pub address: String,
    /// The server's own identifier, stable across address changes.
    pub server_identifier: String,
    pub server_name: String,
    pub user_identifier: String,
    pub user_name: String,
    pub access_token: String,
    /// When the songs were last read from the server, in Unix seconds.
    pub last_sync_at: Option<i64>,
}

impl std::fmt::Debug for MediaServer {
    /// The access token stays out of any log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaServer")
            .field("id", &self.id)
            .field("address", &self.address)
            .field("server_name", &self.server_name)
            .field("user_name", &self.user_name)
            .field("last_sync_at", &self.last_sync_at)
            .finish_non_exhaustive()
    }
}

/// What a sign-in brings back, before it has a row.
#[derive(Clone, PartialEq)]
pub struct NewMediaServer {
    pub address: String,
    pub server_identifier: String,
    pub server_name: String,
    pub user_identifier: String,
    pub user_name: String,
    pub access_token: String,
}

impl std::fmt::Debug for NewMediaServer {
    /// The access token stays out of any log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewMediaServer")
            .field("address", &self.address)
            .field("server_name", &self.server_name)
            .field("user_name", &self.user_name)
            .finish_non_exhaustive()
    }
}

/// The server the listener is signed in to, if any.
pub fn media_server(database: &DbState) -> Result<Option<MediaServer>, String> {
    let connection = database.lock();
    connection
        .query_row(
            "SELECT id, address, server_identifier, server_name, user_identifier, user_name,
                    access_token, last_sync_at
             FROM media_servers ORDER BY id LIMIT 1",
            [],
            |row| {
                Ok(MediaServer {
                    id: row.get(0)?,
                    address: row.get(1)?,
                    server_identifier: row.get(2)?,
                    server_name: row.get(3)?,
                    user_identifier: row.get(4)?,
                    user_name: row.get(5)?,
                    access_token: row.get(6)?,
                    last_sync_at: row.get(7)?,
                })
            },
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(format!("read media server: {other}")),
        })
}

/// Keeps a sign-in. Signing in again to the same server as the same user only
/// renews the token, so its songs and their loudness measurements stay; any
/// other server or user replaces the one before, songs included — Janis keeps
/// one server at a time.
pub fn save_media_server(
    database: &DbState,
    server: &NewMediaServer,
) -> Result<MediaServer, String> {
    {
        let connection = database.lock();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("save media server: {error}"))?;
        let updated = transaction
            .execute(
                "UPDATE media_servers
                 SET address = ?1, server_name = ?2, user_name = ?3, access_token = ?4
                 WHERE server_identifier = ?5 AND user_identifier = ?6",
                rusqlite::params![
                    server.address,
                    server.server_name,
                    server.user_name,
                    server.access_token,
                    server.server_identifier,
                    server.user_identifier,
                ],
            )
            .map_err(|error| format!("save media server: {error}"))?;
        if updated == 0 {
            transaction
                .execute("DELETE FROM media_servers", [])
                .map_err(|error| format!("replace media server: {error}"))?;
            transaction
                .execute(
                    "INSERT INTO media_servers (address, server_identifier, server_name,
                                                user_identifier, user_name, access_token)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        server.address,
                        server.server_identifier,
                        server.server_name,
                        server.user_identifier,
                        server.user_name,
                        server.access_token,
                    ],
                )
                .map_err(|error| format!("save media server: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("save media server: {error}"))?;
    }
    media_server(database)?.ok_or_else(|| "the saved media server is missing".to_owned())
}

/// Forgets the server, and with it every song it brought.
pub fn remove_media_server(database: &DbState, server_id: i64) -> Result<(), String> {
    let connection = database.lock();
    connection
        .execute("DELETE FROM media_servers WHERE id = ?1", [server_id])
        .map_err(|error| format!("remove media server: {error}"))?;
    Ok(())
}

/// Records that the server's songs were just read.
pub fn set_last_sync(database: &DbState, server_id: i64) -> Result<(), String> {
    let connection = database.lock();
    connection
        .execute(
            "UPDATE media_servers SET last_sync_at = unixepoch() WHERE id = ?1",
            [server_id],
        )
        .map_err(|error| format!("record the sync: {error}"))?;
    Ok(())
}

/// The identifier this installation gives itself when it talks to a server:
/// random, made once, and the same on every launch after.
pub fn device_identifier(database: &DbState) -> Result<String, String> {
    let connection = database.lock();
    connection
        .execute(
            "UPDATE user_preferences SET device_identifier = lower(hex(randomblob(16)))
             WHERE id = 1 AND device_identifier IS NULL",
            [],
        )
        .map_err(|error| format!("make the device identifier: {error}"))?;
    connection
        .query_row(
            "SELECT device_identifier FROM user_preferences WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("read the device identifier: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .expect("table_info");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        rows
    }

    #[test]
    fn a_fresh_database_lands_on_the_current_version() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = init(directory.path().to_path_buf()).expect("init a new database");
        let conn = db.lock();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);

        let cols = columns(&conn, "tracks");
        for expected in [
            "track_number",
            "disc_number",
            "year",
            "genre",
            "album_artist",
        ] {
            assert!(cols.contains(&expected.to_string()), "missing {expected}");
        }
        assert!(columns(&conn, "user_preferences").contains(&"eq_linear_phase".to_string()));
        drop(conn);
    }

    #[test]
    fn an_existing_v1_database_gains_the_new_columns() {
        // The case that silently breaks without migrations: a library created
        // before these columns existed. `CREATE TABLE IF NOT EXISTS` would
        // leave it untouched and every read of a new column would fail.
        let directory = tempfile::tempdir().expect("a temporary directory");
        {
            let conn = Connection::open(directory.path().join(DATABASE_FILE)).expect("open");
            conn.execute_batch(SCHEMA).expect("v1 schema");
            conn.execute(
                "INSERT INTO tracks (path, title, format) VALUES ('/a.flac', 'A', 'FLAC')",
                [],
            )
            .expect("seed a track");
            // Version 0 is what a pre-migration file carries.
            conn.pragma_update(None, "user_version", 0).expect("stamp");
        }

        let db = init(directory.path().to_path_buf()).expect("init over an existing database");
        let conn = db.lock();

        let cols = columns(&conn, "tracks");
        assert!(cols.contains(&"track_number".to_string()));
        assert!(cols.contains(&"album_artist".to_string()));
        assert!(cols.contains(&"rg_track_gain_db".to_string()));
        assert!(cols.contains(&"loudness_lufs".to_string()));
        // The preferences row gains its column too, defaulted off — the
        // realtime EQ stays what an upgraded install boots with.
        assert!(columns(&conn, "user_preferences").contains(&"eq_linear_phase".to_string()));
        let linear_phase: i64 = conn
            .query_row(
                "SELECT eq_linear_phase FROM user_preferences WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("read the new preference");
        assert_eq!(linear_phase, 0);

        // The row that was already there survives, with the new column empty.
        let (title, track_number): (String, Option<u32>) = conn
            .query_row(
                "SELECT title, track_number FROM tracks WHERE path = '/a.flac'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("the existing row is still there");
        assert_eq!(title, "A");
        assert_eq!(track_number, None);

        drop(conn);
    }

    #[test]
    fn a_database_with_a_migration_applied_but_not_stamped_still_boots() {
        // A file can carry a migration's columns with the version behind:
        // the DDL committed, the stamp did not. Running that migration again
        // dies on "duplicate column name" at every launch. Such a file must
        // be adopted, not refused.
        let directory = tempfile::tempdir().expect("a temporary directory");
        {
            let conn = Connection::open(directory.path().join(DATABASE_FILE)).expect("open");
            conn.execute_batch(SCHEMA).expect("v1 schema");
            // Migration to v2 fully applied…
            conn.execute_batch(MIGRATIONS[0]).expect("v2 ddl");
            // …but the crash landed before the version stamp.
            conn.pragma_update(None, "user_version", 1).expect("stamp");
        }

        let db =
            init(directory.path().to_path_buf()).expect("init must adopt the applied migration");
        let conn = db.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);
        assert!(
            columns(&conn, "tracks").contains(&"loudness_lufs".to_string()),
            "the migrations after the adopted one still run"
        );
        drop(conn);
    }

    #[test]
    fn a_favourite_station_is_remembered_until_its_star_is_taken_away() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        assert_eq!(favourite_station_ids(&database), Ok(Vec::new()));

        set_favourite_station(&database, "fip", true).expect("star a station");
        // Starring twice keeps one row.
        set_favourite_station(&database, "fip", true).expect("star it again");
        assert_eq!(favourite_station_ids(&database), Ok(vec!["fip".to_owned()]));

        set_favourite_station(&database, "fip", false).expect("take the star away");
        assert_eq!(favourite_station_ids(&database), Ok(Vec::new()));
    }

    /// A database as an earlier Janis left it: the frozen schema, the
    /// migrations up to `version`, a preference changed and one track.
    fn database_at_version(directory: &std::path::Path, version: i64) {
        std::fs::create_dir_all(directory).expect("temp directory");
        let connection = Connection::open(directory.join(DATABASE_FILE)).expect("open");
        connection.execute_batch(SCHEMA).expect("v1 schema");
        for migration in &MIGRATIONS[..(version.max(1) - 1) as usize] {
            connection
                .execute_batch(migration)
                .expect("earlier migrations");
        }
        connection
            .execute(
                "INSERT INTO user_preferences (id, volume) VALUES (1, 0.3)",
                [],
            )
            .expect("seed the preferences");
        connection
            .execute(
                "INSERT INTO tracks (path, title, format) VALUES ('/a.flac', 'A', 'FLAC')",
                [],
            )
            .expect("seed a track");
        connection
            .pragma_update(None, "user_version", version)
            .expect("stamp");
    }

    #[test]
    fn a_database_from_every_earlier_version_upgrades_and_keeps_what_it_held() {
        // Version 0 is a file from before migrations; 1 to 5 are each release
        // since. Every one must land on the current shape with nothing lost.
        for version in 0..SCHEMA_VERSION {
            let directory = tempfile::tempdir().expect("a temporary directory");
            database_at_version(directory.path(), version);

            let database = init(directory.path().to_path_buf())
                .unwrap_or_else(|error| panic!("init over a v{version} database: {error}"));
            let preferences = get_preferences(&database).expect("the preferences read");
            assert_eq!(preferences.volume, 0.3, "v{version}: the volume survives");
            assert!(
                !preferences.eq_linear_phase,
                "v{version}: linear phase is off"
            );
            assert_eq!(favourite_station_ids(&database), Ok(Vec::new()));
            assert_eq!(media_server(&database), Ok(None));
            let connection = database.lock();
            let (title, server_id): (String, Option<i64>) = connection
                .query_row(
                    "SELECT title, server_id FROM tracks WHERE path = '/a.flac'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|error| panic!("v{version}: the track survives: {error}"));
            assert_eq!(title, "A");
            assert_eq!(
                server_id, None,
                "v{version}: an existing track is a local one"
            );
            let schema_version: i64 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("read version");
            assert_eq!(schema_version, SCHEMA_VERSION);
            drop(connection);
        }
    }

    fn new_server(server_identifier: &str, user_identifier: &str, token: &str) -> NewMediaServer {
        NewMediaServer {
            address: "http://jellyfin.local:8096".to_owned(),
            server_identifier: server_identifier.to_owned(),
            server_name: "Home".to_owned(),
            user_identifier: user_identifier.to_owned(),
            user_name: "listener".to_owned(),
            access_token: token.to_owned(),
        }
    }

    fn server_track(database: &DbState, server_id: i64, item: &str) {
        database
            .lock()
            .execute(
                "INSERT INTO tracks (server_id, path, title, format) VALUES (?1, ?2, 'T', 'FLAC')",
                rusqlite::params![server_id, format!("jellyfin://server/{item}")],
            )
            .expect("insert a server track");
    }

    fn track_count(database: &DbState) -> i64 {
        database
            .lock()
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .expect("count tracks")
    }

    #[test]
    fn signing_in_again_to_the_same_server_keeps_its_songs_and_renews_the_token() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        assert_eq!(media_server(&database), Ok(None));

        let first = save_media_server(&database, &new_server("server", "user", "one"))
            .expect("save a sign-in");
        server_track(&database, first.id, "a");
        let second = save_media_server(&database, &new_server("server", "user", "two"))
            .expect("sign in again");

        assert_eq!(second.id, first.id);
        assert_eq!(second.access_token, "two");
        assert_eq!(track_count(&database), 1, "the songs stay");
    }

    #[test]
    fn another_server_or_signing_out_takes_the_songs_away() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        let first = save_media_server(&database, &new_server("server", "user", "one"))
            .expect("save a sign-in");
        server_track(&database, first.id, "a");

        let other = save_media_server(&database, &new_server("other", "user", "token"))
            .expect("sign in elsewhere");
        assert_eq!(other.server_identifier, "other");
        assert_eq!(
            track_count(&database),
            0,
            "the first server's songs go with it"
        );

        server_track(&database, other.id, "b");
        remove_media_server(&database, other.id).expect("sign out");
        assert_eq!(media_server(&database), Ok(None));
        assert_eq!(track_count(&database), 0);
    }

    #[test]
    fn the_device_identifier_is_made_once() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        let first = device_identifier(&database).expect("made");
        let second = device_identifier(&database).expect("read back");
        assert_eq!(first.len(), 32);
        assert_eq!(first, second);
    }

    #[test]
    fn a_fresh_database_reads_the_documented_defaults() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        assert_eq!(
            get_preferences(&database),
            Ok(Preferences {
                volume: 0.8,
                eq_gains: vec![0.0; EQ_BAND_COUNT],
                eq_preset: "flat".to_owned(),
                eq_linear_phase: false,
                gapless: true,
                crossfade: true,
                normalize: true,
                exclusive: false,
                language: "en".to_owned(),
            })
        );
    }

    #[test]
    fn the_saved_playback_switches_start_where_the_player_does() {
        use crate::playback_options::PlaybackOptions;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        let saved = get_preferences(&database).expect("the preferences read");
        let defaults = PlaybackOptions::default();
        for (option, saved) in [
            (PlaybackOption::Gapless, saved.gapless),
            (PlaybackOption::Crossfade, saved.crossfade),
            (PlaybackOption::Normalize, saved.normalize),
            (PlaybackOption::Exclusive, saved.exclusive),
        ] {
            assert_eq!(defaults.enabled(option), saved, "{option:?}");
        }
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        // Every launch runs `init`; the second must not try to re-add columns.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let first = init(directory.path().to_path_buf()).expect("first open");
        drop(first);
        let second = init(directory.path().to_path_buf()).expect("second open must not fail");
        let conn = second.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);
        drop(conn);
    }
}
