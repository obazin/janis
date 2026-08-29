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
    crossfade INTEGER NOT NULL DEFAULT 0,
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

/// Opens (creating if needed) `janis.db` under the app-data directory and
/// applies the schema. Called once from `main.rs`'s setup hook.
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
    conn.execute("INSERT OR IGNORE INTO user_preferences (id) VALUES (1)", [])
        .map_err(|e| format!("seed preferences row: {}", e))?;
    Ok(DbState(Mutex::new(conn)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub volume: f64,
    pub eq_gains: Vec<f64>,
    pub eq_preset: String,
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
        "SELECT volume, eq_gains, eq_preset, gapless, crossfade, normalize, exclusive, language
         FROM user_preferences WHERE id = 1",
        [],
        |row| {
            let gains_json: String = row.get(1)?;
            Ok(Preferences {
                volume: row.get(0)?,
                eq_gains: serde_json::from_str(&gains_json).unwrap_or_else(|_| vec![0.0; 10]),
                eq_preset: row.get(2)?,
                gapless: row.get::<_, i64>(3)? != 0,
                crossfade: row.get::<_, i64>(4)? != 0,
                normalize: row.get::<_, i64>(5)? != 0,
                exclusive: row.get::<_, i64>(6)? != 0,
                language: row.get(7)?,
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
