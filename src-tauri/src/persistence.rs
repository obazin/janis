//! `janis.db` — the app's single SQLite store (user preferences + track
//! library). Opened once at setup in the OS app-data directory (resolved via
//! Tauri's path API, cross-platform by construction) and shared behind a
//! mutex: every command is a short read or write, so one connection is
//! plenty and keeps SQLite's locking model trivial.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

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
const SCHEMA_VERSION: i64 = 4;

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
];

/// Opens (creating if needed) `janis.db` under the app-data directory and
/// brings it up to date. Called once from `main.rs`'s setup hook.
pub fn init(app_data_dir: PathBuf) -> Result<DbState, String> {
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("create app data dir {}: {}", app_data_dir.display(), e))?;
    let db_path = app_data_dir.join("janis.db");
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
            // A database wounded by the old split write (columns present,
            // version behind) fails exactly here. The migrations are
            // append-only ALTERs, so a duplicate column proves this one
            // already ran — adopt it rather than refuse to boot forever.
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
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

/// The four playback switches, as a closed enum so the column an invoke can
/// touch is an allowlist rather than an interpolated string.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
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

#[tauri::command]
pub fn get_preferences(db: tauri::State<'_, DbState>) -> Result<Preferences, String> {
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
                eq_gains: serde_json::from_str(&gains_json).unwrap_or_else(|_| vec![0.0; 10]),
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

#[tauri::command]
pub fn set_volume(db: tauri::State<'_, DbState>, volume: f64) -> Result<(), String> {
    let volume = volume.clamp(0.0, 1.0);
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET volume = ?1, updated_at = unixepoch() WHERE id = 1",
        [volume],
    )
    .map_err(|e| format!("persist volume: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn set_eq(
    db: tauri::State<'_, DbState>,
    gains: Vec<f64>,
    preset: String,
) -> Result<(), String> {
    if gains.len() != 10 {
        return Err(format!("expected 10 EQ gains, got {}", gains.len()));
    }
    let gains: Vec<f64> = gains.iter().map(|g| g.clamp(-12.0, 12.0)).collect();
    let gains_json = serde_json::to_string(&gains).map_err(|e| e.to_string())?;
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
#[tauri::command]
pub fn set_eq_linear_phase(db: tauri::State<'_, DbState>, enabled: bool) -> Result<(), String> {
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET eq_linear_phase = ?1, updated_at = unixepoch() WHERE id = 1",
        [enabled as i64],
    )
    .map_err(|e| format!("persist eq linear phase: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn set_playback_option(
    db: tauri::State<'_, DbState>,
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

#[tauri::command]
pub fn set_language(db: tauri::State<'_, DbState>, language: String) -> Result<(), String> {
    if !matches!(language.as_str(), "en" | "fr") {
        return Err(format!("unsupported language: {}", language));
    }
    let conn = db.lock();
    conn.execute(
        "UPDATE user_preferences SET language = ?1, updated_at = unixepoch() WHERE id = 1",
        [language],
    )
    .map_err(|e| format!("persist language: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("janis-db-{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

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
        let dir = temp_dir("fresh");
        let db = init(dir.clone()).expect("init a new database");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_v1_database_gains_the_new_columns() {
        // The case that silently breaks without migrations: a library created
        // before these columns existed. `CREATE TABLE IF NOT EXISTS` would
        // leave it untouched and every read of a new column would fail.
        let dir = temp_dir("upgrade");
        std::fs::create_dir_all(&dir).expect("temp dir");
        {
            let conn = Connection::open(dir.join("janis.db")).expect("open");
            conn.execute_batch(SCHEMA).expect("v1 schema");
            conn.execute(
                "INSERT INTO tracks (path, title, format) VALUES ('/a.flac', 'A', 'FLAC')",
                [],
            )
            .expect("seed a track");
            // Version 0 is what a pre-migration file carries.
            conn.pragma_update(None, "user_version", 0).expect("stamp");
        }

        let db = init(dir.clone()).expect("init over an existing database");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_wounded_by_the_old_split_stamp_still_boots() {
        // The old code committed a migration's DDL and stamped user_version
        // as a separate write; a crash between the two left the columns
        // present with the version behind, and every later launch then died
        // on "duplicate column name" — permanently unbootable. Such a file
        // must be adopted, not refused.
        let dir = temp_dir("wounded");
        std::fs::create_dir_all(&dir).expect("temp dir");
        {
            let conn = Connection::open(dir.join("janis.db")).expect("open");
            conn.execute_batch(SCHEMA).expect("v1 schema");
            // Migration to v2 fully applied…
            conn.execute_batch(MIGRATIONS[0]).expect("v2 ddl");
            // …but the crash landed before the version stamp.
            conn.pragma_update(None, "user_version", 1).expect("stamp");
        }

        let db = init(dir.clone()).expect("init must adopt the applied migration");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        // Every launch runs `init`; the second must not try to re-add columns.
        let dir = temp_dir("idempotent");
        let first = init(dir.clone()).expect("first open");
        drop(first);
        let second = init(dir.clone()).expect("second open must not fail");
        let conn = second.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("read version");
        assert_eq!(version, SCHEMA_VERSION);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
