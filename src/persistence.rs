//! `janis.db` — the app's single SQLite store (user preferences + track
//! library). Opened once at boot in the per-user data directory (resolved by
//! `platform::data_directory`, cross-platform by construction) and shared
//! behind a mutex: every call is a short read or write, so one connection is
//! plenty and keeps SQLite's locking model trivial.

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::audio::EQ_BAND_COUNT;

/// The file `init` opens in the data directory.
const DATABASE_FILE: &str = "janis.db";

pub struct DbState {
    connection: Mutex<Connection>,
    /// Counts the changes to what a library snapshot reads — the songs, the
    /// folders, the server's libraries, the favourites, the playlists and
    /// their groups — so a snapshot can tell it was overtaken by one.
    library_revision: AtomicU64,
    /// The threads waiting for the connection, so a job writing in bursts
    /// can let them have it between two.
    waiting: AtomicUsize,
}

/// The longest a job writing in bursts waits for the others to have had the
/// connection, so a stream of waiters cannot hold it up for ever.
const GIVE_WAY_LIMIT: Duration = Duration::from_millis(20);

impl DbState {
    fn new(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
            library_revision: AtomicU64::new(0),
            waiting: AtomicUsize::new(0),
        }
    }

    /// The connection, for a read or for a write that leaves the library as
    /// it was (a preference, a loudness measurement).
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let connection = self.connection.lock().expect("janis.db mutex poisoned");
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        connection
    }

    /// Lets the threads waiting for the connection have it first. A job that
    /// writes in bursts calls it between two, without the lock: the mutex is
    /// not fair, and the thread that just released it would otherwise take
    /// it straight back, keeping the interface and the engine waiting for the
    /// whole job.
    pub fn give_way(&self) {
        let deadline = Instant::now() + GIVE_WAY_LIMIT;
        while self.waiting.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
    }

    /// The connection, for a write that changes what a library snapshot
    /// reads: the revision moves on when the guard goes, before the lock is
    /// released.
    pub fn lock_for_library_change(&self) -> LibraryChange<'_> {
        LibraryChange {
            connection: self.lock(),
            revision: &self.library_revision,
        }
    }

    /// The library's revision now. Read under the lock, it is the revision
    /// of what that lock's reads see.
    pub fn library_revision(&self) -> u64 {
        self.library_revision.load(Ordering::Acquire)
    }
}

/// The connection held for a change to the library; see
/// [`DbState::lock_for_library_change`].
pub struct LibraryChange<'database> {
    connection: MutexGuard<'database, Connection>,
    revision: &'database AtomicU64,
}

impl std::ops::Deref for LibraryChange<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl std::ops::DerefMut for LibraryChange<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

impl Drop for LibraryChange<'_> {
    fn drop(&mut self) {
        // The guard's fields, the lock among them, are dropped after this.
        self.revision.fetch_add(1, Ordering::AcqRel);
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
const SCHEMA_VERSION: i64 = 12;

// The migrations reach the version a fresh database is stamped with.
const _: () = assert!(MIGRATIONS.len() as i64 + 1 == SCHEMA_VERSION);

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
    // → 7: the libraries a media server shows its songs in, and which songs
    //      each holds. Every library is synced; `shown` is the listener's
    //      choice of the ones whose songs appear in Janis, on by default. A
    //      song may sit in more than one library when they share a folder,
    //      hence the join table. Both cascade from the server and the song.
    "CREATE TABLE IF NOT EXISTS server_libraries (
         id INTEGER PRIMARY KEY,
         server_id INTEGER NOT NULL REFERENCES media_servers(id) ON DELETE CASCADE,
         library_identifier TEXT NOT NULL,
         name TEXT NOT NULL DEFAULT '',
         position INTEGER NOT NULL DEFAULT 0,
         shown INTEGER NOT NULL DEFAULT 1,
         UNIQUE (server_id, library_identifier)
     );
     CREATE TABLE IF NOT EXISTS track_libraries (
         track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
         library_id INTEGER NOT NULL REFERENCES server_libraries(id) ON DELETE CASCADE,
         PRIMARY KEY (track_id, library_id)
     ) WITHOUT ROWID;
     CREATE INDEX IF NOT EXISTS idx_track_libraries_library ON track_libraries(library_id);",
    // → 8: the songs the listener starred. A row is a favourite; no row is the
    //      default. It cascades from the song, so a file pruned or a server
    //      song gone from its server takes its star with it. A server song's
    //      row follows the server's own favourite at every sync; `added_at`
    //      orders the Favourites tab, newest first.
    //      Its own row id breaks a tie between two stars of the same second.
    "CREATE TABLE IF NOT EXISTS favourite_tracks (
         id INTEGER PRIMARY KEY,
         track_id INTEGER NOT NULL UNIQUE REFERENCES tracks(id) ON DELETE CASCADE,
         added_at INTEGER NOT NULL DEFAULT (unixepoch())
     );",
    // → 9: the listener's playlists, the groups they sort them into, and the
    //      songs each holds, in order. A playlist lives in Janis alone, or on
    //      a media server (`server_id` and its `server_item`), whose sync
    //      keeps it, so it leaves with the server. Groups and the chosen
    //      picture are Janis's own and survive a sync. A song is in a
    //      playlist at most once; a removed song leaves its playlists.
    "CREATE TABLE IF NOT EXISTS playlist_groups (
         id INTEGER PRIMARY KEY,
         name TEXT NOT NULL,
         position INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE IF NOT EXISTS playlists (
         id INTEGER PRIMARY KEY,
         name TEXT NOT NULL,
         server_id INTEGER REFERENCES media_servers(id) ON DELETE CASCADE,
         server_item TEXT,
         server_picture INTEGER NOT NULL DEFAULT 0,
         group_id INTEGER REFERENCES playlist_groups(id) ON DELETE SET NULL,
         picture BLOB,
         created_at INTEGER NOT NULL DEFAULT (unixepoch()),
         UNIQUE (server_id, server_item)
     );
     CREATE TABLE IF NOT EXISTS playlist_entries (
         playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
         track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
         position INTEGER NOT NULL,
         PRIMARY KEY (playlist_id, track_id)
     ) WITHOUT ROWID;
     CREATE INDEX IF NOT EXISTS idx_playlist_entries_track ON playlist_entries(track_id);",
    // → 10: only the path of a user-owned cookie file is remembered. Google
    //       session cookies themselves never enter the database.
    "ALTER TABLE user_preferences ADD COLUMN youtube_music_cookie_file TEXT;",
    // → 11: provider ownership separates remote songs from local-file work.
    //       Existing Jellyfin rows keep their identity and associated state.
    "ALTER TABLE tracks ADD COLUMN source TEXT NOT NULL DEFAULT 'file';
     UPDATE tracks SET source = 'jellyfin' WHERE server_id IS NOT NULL;
     ALTER TABLE tracks ADD COLUMN artwork_address TEXT;
     CREATE INDEX IF NOT EXISTS idx_tracks_source ON tracks(source);
     ALTER TABLE user_preferences ADD COLUMN youtube_music_last_sync_at INTEGER;",
    // → 12: playlists have an explicit provider, with stable identities for
    //       read-only YouTube Music imports independent of Jellyfin rows.
    "ALTER TABLE playlists ADD COLUMN source TEXT NOT NULL DEFAULT 'local';
     UPDATE playlists SET source = 'jellyfin' WHERE server_id IS NOT NULL;
     ALTER TABLE playlists ADD COLUMN provider_identifier TEXT;
     CREATE UNIQUE INDEX idx_playlists_provider
         ON playlists(source, provider_identifier);",
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
    Ok(DbState::new(conn))
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

    // The YouTube Music feature's databases used versions 9 and 10 before
    // upstream introduced playlists at 9. Their shape identifies them
    // unambiguously: adopt the missing upstream tables without replaying
    // provider ALTERs over existing tracks.
    if matches!(version, 9 | 10) {
        let legacy_youtube_music: bool = conn
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM pragma_table_info('user_preferences')
                     WHERE name = 'youtube_music_cookie_file'
                 ) AND NOT EXISTS (
                     SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'playlists'
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|error| format!("recognize provider schema: {error}"))?;
        if legacy_youtube_music {
            let adopted_version = version + 1;
            let transaction = conn
                .unchecked_transaction()
                .map_err(|error| format!("adopt provider schema: {error}"))?;
            transaction
                .execute_batch(MIGRATIONS[7])
                .map_err(|error| format!("adopt upstream playlists: {error}"))?;
            transaction
                .pragma_update(None, "user_version", adopted_version)
                .map_err(|error| format!("stamp adopted provider schema: {error}"))?;
            transaction
                .commit()
                .map_err(|error| format!("commit adopted provider schema: {error}"))?;
            version = adopted_version;
        }
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

    if version > SCHEMA_VERSION {
        // Written by a later Janis: its additions are columns and tables this
        // one does not read, so the file opens as it is.
        log::warn!("janis.db is at version {version}, newer than this Janis ({SCHEMA_VERSION})");
    }
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
    pub const ALL: [PlaybackOption; 4] = [
        PlaybackOption::Gapless,
        PlaybackOption::Crossfade,
        PlaybackOption::Normalize,
        PlaybackOption::Exclusive,
    ];

    /// The switch the Settings screen names; its column's name, too.
    pub fn from_identifier(identifier: &str) -> Option<PlaybackOption> {
        Self::ALL
            .into_iter()
            .find(|option| option.column() == identifier)
    }

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

/// The user-owned cookie file to read on a worker when restoring the source.
pub fn youtube_music_cookie_file(database: &DbState) -> Result<Option<PathBuf>, String> {
    database
        .lock()
        .query_row(
            "SELECT youtube_music_cookie_file FROM user_preferences WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        )
        .map(|path| path.map(PathBuf::from))
        .map_err(|error| format!("read YouTube Music connection: {error}"))
}

/// The last complete YouTube Music library import, in Unix seconds.
pub fn youtube_music_last_sync(database: &DbState) -> Result<Option<i64>, String> {
    database
        .lock()
        .query_row(
            "SELECT youtube_music_last_sync_at FROM user_preferences WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("read YouTube Music sync time: {error}"))
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

/// The ids of the songs the listener starred, the newest favourite first.
pub fn favourite_track_ids(database: &DbState) -> Result<Vec<i64>, String> {
    favourite_track_ids_in(&database.lock())
}

/// [`favourite_track_ids`], on a connection the caller holds.
pub(crate) fn favourite_track_ids_in(
    connection: &rusqlite::Connection,
) -> Result<Vec<i64>, String> {
    connection
        .prepare("SELECT track_id FROM favourite_tracks ORDER BY added_at DESC, id DESC")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|error| format!("read favourite songs: {error}"))
}

/// Stars a song or takes its star away. Saving the state it already has
/// changes nothing, and starring a song no longer in the library — removed
/// while its star was on its way to a server — does nothing either.
pub fn set_favourite_track(
    database: &DbState,
    track_id: i64,
    favourite: bool,
) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    let statement = if favourite {
        "INSERT OR IGNORE INTO favourite_tracks (track_id) SELECT id FROM tracks WHERE id = ?1"
    } else {
        "DELETE FROM favourite_tracks WHERE track_id = ?1"
    };
    connection
        .execute(statement, [track_id])
        .map_err(|error| format!("persist favourite song: {error}"))?;
    Ok(())
}

/// The owner of a playlist, independent of provider-specific columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistSource {
    Local,
    Jellyfin,
    YouTubeMusic,
}

impl rusqlite::types::FromSql for PlaylistSource {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "local" => Ok(Self::Local),
            "jellyfin" => Ok(Self::Jellyfin),
            "youtube-music" => Ok(Self::YouTubeMusic),
            source => Err(rusqlite::types::FromSqlError::Other(
                format!("unknown playlist source: {source}").into(),
            )),
        }
    }
}

/// A playlist as `janis.db` keeps it, with its songs in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedPlaylist {
    pub id: i64,
    pub name: String,
    pub source: PlaylistSource,
    /// The Jellyfin server it lives on, and its item there; none for other
    /// providers.
    pub server: Option<(i64, String)>,
    /// The server has a picture of it.
    pub server_picture: bool,
    pub group_id: Option<i64>,
    /// The listener chose a picture for it.
    pub picture: bool,
    pub track_ids: Vec<i64>,
}

/// A group the listener sorts playlists into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistGroup {
    pub id: i64,
    pub name: String,
}

/// Every playlist, the oldest first, each with its songs in order.
pub fn playlists(database: &DbState) -> Result<Vec<SavedPlaylist>, String> {
    playlists_in(&database.lock())
}

/// [`playlists`], on a connection the caller holds.
pub(crate) fn playlists_in(
    connection: &rusqlite::Connection,
) -> Result<Vec<SavedPlaylist>, String> {
    let mut playlists: Vec<SavedPlaylist> = connection
        .prepare(
            "SELECT id, name, server_id, server_item, server_picture, group_id,
                    picture IS NOT NULL, source
             FROM playlists ORDER BY created_at, id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    let server_id: Option<i64> = row.get(2)?;
                    let server_item: Option<String> = row.get(3)?;
                    Ok(SavedPlaylist {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        source: row.get(7)?,
                        server: server_id.zip(server_item),
                        server_picture: row.get::<_, i64>(4)? != 0,
                        group_id: row.get(5)?,
                        picture: row.get(6)?,
                        track_ids: Vec::new(),
                    })
                })?
                .collect()
        })
        .map_err(|error| format!("read the playlists: {error}"))?;
    let entries: Vec<(i64, i64)> = connection
        .prepare(
            "SELECT playlist_id, track_id FROM playlist_entries ORDER BY playlist_id, position",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|error| format!("read the playlists' songs: {error}"))?;
    let places: std::collections::HashMap<i64, usize> = playlists
        .iter()
        .enumerate()
        .map(|(place, playlist)| (playlist.id, place))
        .collect();
    for (playlist_id, track_id) in entries {
        if let Some(place) = places.get(&playlist_id) {
            playlists[*place].track_ids.push(track_id);
        }
    }
    Ok(playlists)
}

/// The groups, in the order the listener gave them.
pub fn playlist_groups(database: &DbState) -> Result<Vec<PlaylistGroup>, String> {
    playlist_groups_in(&database.lock())
}

/// [`playlist_groups`], on a connection the caller holds.
pub(crate) fn playlist_groups_in(
    connection: &rusqlite::Connection,
) -> Result<Vec<PlaylistGroup>, String> {
    connection
        .prepare("SELECT id, name FROM playlist_groups ORDER BY position, id")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok(PlaylistGroup {
                        id: row.get(0)?,
                        name: row.get(1)?,
                    })
                })?
                .collect()
        })
        .map_err(|error| format!("read the playlist groups: {error}"))
}

/// Saves a new playlist holding `track_ids`, in that order, and gives its
/// row. A server's playlist the server's sync saved already is that row,
/// renamed; one whose server is gone is not saved, and gives none.
pub fn create_playlist(
    database: &DbState,
    name: &str,
    server: Option<(i64, &str)>,
    track_ids: &[i64],
) -> Result<Option<i64>, String> {
    let mut connection = database.lock_for_library_change();
    let transaction = connection
        .transaction()
        .map_err(|error| format!("create a playlist: {error}"))?;
    let playlist_id: Option<i64> = match server {
        None => transaction
            .query_row(
                "INSERT INTO playlists (name) VALUES (?1) RETURNING id",
                [name],
                |row| row.get(0),
            )
            .map(Some),
        Some((server_id, item)) => transaction
            .query_row(
                "INSERT INTO playlists (name, server_id, server_item, source)
                 SELECT ?1, id, ?3, 'jellyfin' FROM media_servers WHERE id = ?2
                 ON CONFLICT (server_id, server_item) DO UPDATE SET name = excluded.name
                 RETURNING id",
                rusqlite::params![name, server_id, item],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                error => Err(error),
            }),
    }
    .map_err(|error| format!("create a playlist: {error}"))?;
    if let Some(playlist_id) = playlist_id {
        for track_id in track_ids {
            add_entry(&transaction, playlist_id, *track_id)
                .map_err(|error| format!("fill a new playlist: {error}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("create a playlist: {error}"))?;
    Ok(playlist_id)
}

/// Puts an owned song at the end of an editable playlist, unless it is there
/// already or no longer in the library.
fn add_entry(connection: &Connection, playlist_id: i64, track_id: i64) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT OR IGNORE INTO playlist_entries (playlist_id, track_id, position)
         SELECT ?1, tracks.id,
                (SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_entries WHERE playlist_id = ?1)
         FROM tracks JOIN playlists ON playlists.id = ?1
         WHERE tracks.id = ?2 AND tracks.id > 0
           AND ((playlists.source = 'local' AND tracks.source IN ('file', 'youtube-music'))
                OR (playlists.source = 'jellyfin' AND tracks.source = 'jellyfin'
                    AND playlists.server_id = tracks.server_id))",
        [playlist_id, track_id],
    )
}

/// Puts a song at the end of a playlist, or takes it out.
pub fn set_playlist_entry(
    database: &DbState,
    playlist_id: i64,
    track_id: i64,
    present: bool,
) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    let result = if present {
        add_entry(&connection, playlist_id, track_id)
    } else {
        connection.execute(
            "DELETE FROM playlist_entries WHERE playlist_id = ?1 AND track_id = ?2
             AND EXISTS (SELECT 1 FROM playlists
                         WHERE id = ?1 AND source != 'youtube-music')",
            [playlist_id, track_id],
        )
    };
    result
        .map(|_| ())
        .map_err(|error| format!("change a playlist's songs: {error}"))
}

pub fn rename_playlist(database: &DbState, playlist_id: i64, name: &str) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "UPDATE playlists SET name = ?1 WHERE id = ?2 AND source != 'youtube-music'",
            rusqlite::params![name, playlist_id],
        )
        .map(|_| ())
        .map_err(|error| format!("rename a playlist: {error}"))
}

pub fn remove_playlist(database: &DbState, playlist_id: i64) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "DELETE FROM playlists WHERE id = ?1 AND source != 'youtube-music'",
            [playlist_id],
        )
        .map(|_| ())
        .map_err(|error| format!("remove a playlist: {error}"))
}

/// Files a playlist in a group, or in none.
pub fn set_playlist_group(
    database: &DbState,
    playlist_id: i64,
    group_id: Option<i64>,
) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "UPDATE playlists SET group_id = (SELECT id FROM playlist_groups WHERE id = ?1)
             WHERE id = ?2",
            rusqlite::params![group_id, playlist_id],
        )
        .map(|_| ())
        .map_err(|error| format!("file a playlist in a group: {error}"))
}

/// Keeps the picture the listener chose for a playlist, or forgets it.
pub fn set_playlist_picture(
    database: &DbState,
    playlist_id: i64,
    picture: Option<&[u8]>,
) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "UPDATE playlists SET picture = ?1 WHERE id = ?2",
            rusqlite::params![picture, playlist_id],
        )
        .map(|_| ())
        .map_err(|error| format!("keep a playlist's picture: {error}"))
}

/// The picture the listener chose for a playlist, as it was saved.
pub fn playlist_picture(database: &DbState, playlist_id: i64) -> Result<Option<Vec<u8>>, String> {
    let connection = database.lock();
    connection
        .query_row(
            "SELECT picture FROM playlists WHERE id = ?1",
            [playlist_id],
            |row| row.get(0),
        )
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            error => Err(error),
        })
        .map_err(|error| format!("read a playlist's picture: {error}"))
}

/// Saves a new group after the others and gives its row.
pub fn create_playlist_group(database: &DbState, name: &str) -> Result<i64, String> {
    let connection = database.lock_for_library_change();
    connection
        .query_row(
            "INSERT INTO playlist_groups (name, position)
             SELECT ?1, COALESCE(MAX(position) + 1, 0) FROM playlist_groups
             RETURNING id",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| format!("create a playlist group: {error}"))
}

pub fn rename_playlist_group(database: &DbState, group_id: i64, name: &str) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "UPDATE playlist_groups SET name = ?1 WHERE id = ?2",
            rusqlite::params![name, group_id],
        )
        .map(|_| ())
        .map_err(|error| format!("rename a playlist group: {error}"))
}

/// Removes a group; its playlists are then in none.
pub fn remove_playlist_group(database: &DbState, group_id: i64) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute("DELETE FROM playlist_groups WHERE id = ?1", [group_id])
        .map(|_| ())
        .map_err(|error| format!("remove a playlist group: {error}"))
}

/// Puts the groups in the order given.
pub fn set_playlist_group_order(database: &DbState, group_ids: &[i64]) -> Result<(), String> {
    let mut connection = database.lock_for_library_change();
    let transaction = connection
        .transaction()
        .map_err(|error| format!("order the playlist groups: {error}"))?;
    for (position, group_id) in group_ids.iter().enumerate() {
        transaction
            .execute(
                "UPDATE playlist_groups SET position = ?1 WHERE id = ?2",
                rusqlite::params![position as i64, group_id],
            )
            .map_err(|error| format!("order the playlist groups: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("order the playlist groups: {error}"))
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
        let connection = database.lock_for_library_change();
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
            // The replacement goes in before the account it replaces goes
            // out, so it never takes that row's id: what the controller
            // still holds of the old account cannot pass for the new one's.
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
            transaction
                .execute(
                    "DELETE FROM media_servers WHERE id != ?1",
                    [transaction.last_insert_rowid()],
                )
                .map_err(|error| format!("replace media server: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("save media server: {error}"))?;
    }
    media_server(database)?.ok_or_else(|| "the saved media server is missing".to_owned())
}

/// Forgets the server, and with it every song it brought.
pub fn remove_media_server(database: &DbState, server_id: i64) -> Result<(), String> {
    let connection = database.lock_for_library_change();
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

/// One library of the signed-in server, as `janis.db` keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerLibrary {
    pub id: i64,
    pub name: String,
    /// Whether its songs appear in Janis. Every library is synced either way.
    pub shown: bool,
}

/// The libraries of the kept servers, in the order the server lists them.
pub fn server_libraries(database: &DbState) -> Result<Vec<ServerLibrary>, String> {
    server_libraries_in(&database.lock())
}

/// [`server_libraries`], on a connection the caller holds.
pub(crate) fn server_libraries_in(
    connection: &rusqlite::Connection,
) -> Result<Vec<ServerLibrary>, String> {
    connection
        .prepare(
            "SELECT id, name, shown FROM server_libraries
             ORDER BY server_id, position, id",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok(ServerLibrary {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        shown: row.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect()
        })
        .map_err(|error| format!("read the server's libraries: {error}"))
}

/// Remembers whether a library's songs appear in Janis.
pub fn set_server_library_shown(
    database: &DbState,
    library_id: i64,
    shown: bool,
) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    connection
        .execute(
            "UPDATE server_libraries SET shown = ?1 WHERE id = ?2",
            rusqlite::params![shown as i64, library_id],
        )
        .map_err(|error| format!("show or hide a library: {error}"))?;
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

    #[test]
    fn a_favourite_song_is_remembered_newest_first_and_leaves_with_its_song() {
        let (_directory, database) = fresh_database();
        {
            let connection = database.lock();
            connection
                .execute_batch(
                    "INSERT INTO tracks (id, path, title, format) VALUES
                         (1, '/a.flac', 'A', 'FLAC'), (2, '/b.flac', 'B', 'FLAC');",
                )
                .expect("seed two songs");
        }
        assert_eq!(favourite_track_ids(&database), Ok(Vec::new()));

        // Starred within the same second: the later star still comes first.
        set_favourite_track(&database, 2, true).expect("star the second");
        set_favourite_track(&database, 1, true).expect("star the first");
        // Starring twice keeps one row, in its place.
        set_favourite_track(&database, 2, true).expect("star the second again");
        assert_eq!(favourite_track_ids(&database), Ok(vec![1, 2]));

        set_favourite_track(&database, 2, false).expect("take a star away");
        assert_eq!(favourite_track_ids(&database), Ok(vec![1]));

        // A song that is not in the library is not starred.
        set_favourite_track(&database, 9, true).expect("star a song that is gone");
        assert_eq!(favourite_track_ids(&database), Ok(vec![1]));

        database
            .lock()
            .execute("DELETE FROM tracks WHERE id = 1", [])
            .expect("remove the song");
        assert_eq!(favourite_track_ids(&database), Ok(Vec::new()));
    }

    fn seed_songs(database: &DbState) {
        database
            .lock()
            .execute_batch(
                "INSERT INTO tracks (id, path, title, format) VALUES
                     (1, '/a.flac', 'A', 'FLAC'), (2, '/b.flac', 'B', 'FLAC'),
                     (3, '/c.flac', 'C', 'FLAC');",
            )
            .expect("seed three songs");
    }

    #[test]
    fn owned_playlists_mix_saved_sources_without_editing_provider_playlists() {
        let (_directory, database) = fresh_database();
        seed_songs(&database);
        database
            .lock()
            .execute_batch(
                "INSERT INTO media_servers
                     (id, address, server_identifier, user_identifier, user_name, access_token)
                 VALUES (7, 'http://server', 'server', 'user', 'Listener', 'token'),
                        (8, 'http://other', 'other', 'user', 'Listener', 'token');
                 INSERT INTO tracks (id, path, title, format, source, server_id) VALUES
                     (4, 'youtube-music://saved', 'Saved', 'AAC', 'youtube-music', NULL),
                     (-1, 'youtube-music://anonymous', 'Anonymous', 'AAC', 'youtube-music', NULL),
                     (5, 'jellyfin://server/song', 'Server', 'FLAC', 'jellyfin', 7),
                     (6, 'jellyfin://other/song', 'Other', 'FLAC', 'jellyfin', 8);
                 INSERT INTO playlists (id, name, source) VALUES (50, 'Imported', 'youtube-music');
                 INSERT INTO playlist_entries (playlist_id, track_id, position) VALUES (50, 4, 0);",
            )
            .expect("seed provider songs and imported playlist");
        let mixed = create_playlist(&database, "Mixed", None, &[4, 1, -1, 5])
            .expect("create mixed playlist")
            .expect("saved playlist");
        set_playlist_entry(&database, mixed, 2, true).expect("add file");
        set_playlist_entry(&database, mixed, 4, true).expect("deduplicate saved song");
        set_playlist_entry(&database, mixed, -1, true).expect("reject anonymous song");
        set_playlist_entry(&database, mixed, 6, true).expect("reject server song");
        let server = create_playlist(&database, "Server", Some((7, "playlist")), &[5, 6, 4, 1])
            .expect("create server playlist")
            .expect("saved server playlist");
        set_playlist_entry(&database, 50, 1, true).expect("reject imported addition");
        set_playlist_entry(&database, 50, 4, false).expect("reject imported removal");
        rename_playlist(&database, 50, "Changed").expect("reject imported rename");
        remove_playlist(&database, 50).expect("reject imported deletion");
        let saved = playlists(&database).expect("read playlists");
        assert_eq!(
            saved
                .iter()
                .find(|playlist| playlist.id == mixed)
                .expect("mixed")
                .track_ids,
            [4, 1, 2]
        );
        assert_eq!(
            saved
                .iter()
                .find(|playlist| playlist.id == server)
                .expect("server")
                .track_ids,
            [5]
        );
        let imported = saved
            .iter()
            .find(|playlist| playlist.id == 50)
            .expect("imported");
        assert_eq!(imported.name, "Imported");
        assert_eq!(imported.track_ids, [4]);
        set_playlist_entry(&database, mixed, 4, false).expect("remove saved song");
        assert_eq!(
            playlists(&database)
                .expect("read playlists")
                .iter()
                .find(|playlist| playlist.id == mixed)
                .expect("mixed")
                .track_ids,
            [1, 2]
        );
    }

    #[test]
    fn a_playlist_keeps_its_songs_in_the_order_given_each_once() {
        let (_directory, database) = fresh_database();
        seed_songs(&database);
        let playlist_id = create_playlist(&database, "Road", None, &[2, 1])
            .expect("create")
            .expect("a local playlist is always saved");
        set_playlist_entry(&database, playlist_id, 3, true).expect("add a third");
        set_playlist_entry(&database, playlist_id, 2, true).expect("add the second again");
        // A song no longer in the library is not added.
        set_playlist_entry(&database, playlist_id, 9, true).expect("add a song that is gone");
        assert_eq!(
            playlists(&database),
            Ok(vec![SavedPlaylist {
                id: playlist_id,
                name: "Road".to_owned(),
                source: PlaylistSource::Local,
                server: None,
                server_picture: false,
                group_id: None,
                picture: false,
                track_ids: vec![2, 1, 3],
            }])
        );

        set_playlist_entry(&database, playlist_id, 1, false).expect("take one out");
        set_playlist_entry(&database, playlist_id, 1, true).expect("put it back");
        let saved = playlists(&database).expect("read");
        assert_eq!(saved[0].track_ids, [2, 3, 1], "a song put back goes last");

        database
            .lock()
            .execute("DELETE FROM tracks WHERE id = 3", [])
            .expect("remove a song");
        let saved = playlists(&database).expect("read");
        assert_eq!(
            saved[0].track_ids,
            [2, 1],
            "a removed song leaves its playlists"
        );
    }

    #[test]
    fn a_playlist_is_renamed_given_a_picture_and_removed_with_its_songs() {
        let (_directory, database) = fresh_database();
        seed_songs(&database);
        let playlist_id = create_playlist(&database, "Road", None, &[1])
            .expect("create")
            .expect("saved");
        rename_playlist(&database, playlist_id, "Night drive").expect("rename");
        set_playlist_picture(&database, playlist_id, Some(&[1, 2, 3])).expect("picture");
        let saved = playlists(&database).expect("read");
        assert_eq!(saved[0].name, "Night drive");
        assert!(saved[0].picture);
        assert_eq!(
            playlist_picture(&database, playlist_id),
            Ok(Some(vec![1, 2, 3]))
        );
        set_playlist_picture(&database, playlist_id, None).expect("forget the picture");
        assert_eq!(playlist_picture(&database, playlist_id), Ok(None));

        remove_playlist(&database, playlist_id).expect("remove");
        assert_eq!(playlists(&database), Ok(Vec::new()));
        let entries: i64 = database
            .lock()
            .query_row("SELECT COUNT(*) FROM playlist_entries", [], |row| {
                row.get(0)
            })
            .expect("count entries");
        assert_eq!(entries, 0);
        assert_eq!(playlist_picture(&database, playlist_id), Ok(None));
    }

    #[test]
    fn groups_keep_their_order_and_removing_one_leaves_its_playlists_in_none() {
        let (_directory, database) = fresh_database();
        let first = create_playlist_group(&database, "Moods").expect("first group");
        let second = create_playlist_group(&database, "Sport").expect("second group");
        let names = |database: &DbState| -> Vec<String> {
            playlist_groups(database)
                .expect("read groups")
                .into_iter()
                .map(|group| group.name)
                .collect()
        };
        assert_eq!(names(&database), ["Moods", "Sport"]);
        set_playlist_group_order(&database, &[second, first]).expect("reorder");
        rename_playlist_group(&database, first, "Feelings").expect("rename");
        assert_eq!(names(&database), ["Sport", "Feelings"]);

        let playlist_id = create_playlist(&database, "Run", None, &[])
            .expect("create")
            .expect("saved");
        set_playlist_group(&database, playlist_id, Some(second)).expect("file it");
        assert_eq!(
            playlists(&database).expect("read")[0].group_id,
            Some(second)
        );
        // A group that does not exist files it in none.
        set_playlist_group(&database, playlist_id, Some(99)).expect("file it nowhere");
        assert_eq!(playlists(&database).expect("read")[0].group_id, None);

        set_playlist_group(&database, playlist_id, Some(second)).expect("file it again");
        remove_playlist_group(&database, second).expect("remove the group");
        assert_eq!(names(&database), ["Feelings"]);
        assert_eq!(playlists(&database).expect("read")[0].group_id, None);
    }

    #[test]
    fn a_server_playlist_is_saved_once_and_leaves_with_its_server() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init");
        let server = save_media_server(&database, &new_server("server", "user", "token"))
            .expect("save the server");
        server_track(&database, server.id, "song");

        let first = create_playlist(&database, "Mine", Some((server.id, "list")), &[1])
            .expect("create")
            .expect("its server is kept");
        // Saved again — the sync got there first: the same row.
        let again = create_playlist(&database, "Mine too", Some((server.id, "list")), &[])
            .expect("create again")
            .expect("its server is kept");
        assert_eq!(first, again);
        let saved = playlists(&database).expect("read");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].server, Some((server.id, "list".to_owned())));
        assert_eq!(saved[0].name, "Mine too");

        // A playlist whose server has gone is not saved.
        assert_eq!(
            create_playlist(&database, "Lost", Some((server.id + 1, "other")), &[]),
            Ok(None)
        );

        remove_media_server(&database, server.id).expect("sign out");
        assert_eq!(playlists(&database), Ok(Vec::new()));
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
    fn legacy_youtube_music_versions_adopt_upstream_playlists_without_losing_provider_rows() {
        for legacy_version in [9, 10] {
            let directory = tempfile::tempdir().expect("temporary directory");
            database_at_version(directory.path(), 8);
            {
                let connection = Connection::open(directory.path().join(DATABASE_FILE))
                    .expect("legacy database");
                connection
                    .execute_batch(MIGRATIONS[8])
                    .expect("legacy cookie path");
                connection
                    .execute(
                        "UPDATE user_preferences SET youtube_music_cookie_file = 'cookies.txt'",
                        [],
                    )
                    .expect("saved connection");
                if legacy_version == 10 {
                    connection
                        .execute_batch(MIGRATIONS[9])
                        .expect("legacy providers");
                    connection
                        .execute_batch(
                            "INSERT INTO tracks
                                 (id, path, title, format, source, artwork_address, loudness_lufs)
                             VALUES (42, 'youtube-music://abcdefghijk', 'Saved', 'AAC',
                                     'youtube-music', 'https://images.example/cover.jpg', -14);
                             INSERT INTO favourite_tracks (track_id, added_at) VALUES (42, 123);
                             UPDATE user_preferences SET youtube_music_last_sync_at = 456;",
                        )
                        .expect("legacy saved library");
                }
                connection
                    .pragma_update(None, "user_version", legacy_version)
                    .expect("legacy version");
            }
            let database = init(directory.path().to_path_buf()).expect("adopt legacy schema");
            assert_eq!(
                youtube_music_cookie_file(&database),
                Ok(Some(PathBuf::from("cookies.txt")))
            );
            assert_eq!(playlists(&database), Ok(Vec::new()));
            assert_eq!(playlist_groups(&database), Ok(Vec::new()));
            let local = create_playlist(&database, "Local", None, &[1])
                .expect("create after migration")
                .expect("saved");
            assert_eq!(playlists(&database).expect("playlists")[0].id, local);
            if legacy_version == 10 {
                let tracks = crate::library::list_tracks(&database).expect("retained tracks");
                let song = tracks
                    .iter()
                    .find(|song| song.id == 42)
                    .expect("provider song");
                assert_eq!(song.source, crate::library::TrackSource::YouTubeMusic);
                assert_eq!(
                    song.artwork_address.as_deref(),
                    Some("https://images.example/cover.jpg")
                );
                assert_eq!(favourite_track_ids(&database), Ok(vec![42]));
                assert_eq!(youtube_music_last_sync(&database), Ok(Some(456)));
                let loudness: f64 = database
                    .lock()
                    .query_row(
                        "SELECT loudness_lufs FROM tracks WHERE id = 42",
                        [],
                        |row| row.get(0),
                    )
                    .expect("retained loudness");
                assert_eq!(loudness, -14.0);
            }
            assert_eq!(
                database
                    .lock()
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .expect("migrated version"),
                SCHEMA_VERSION
            );
            drop(database);
            init(directory.path().to_path_buf()).expect("reopening remains idempotent");
        }
    }

    #[test]
    fn playlist_provider_migration_preserves_local_and_server_customization() {
        let directory = tempfile::tempdir().expect("temporary directory");
        database_at_version(directory.path(), 9);
        {
            let connection =
                Connection::open(directory.path().join(DATABASE_FILE)).expect("upstream database");
            connection
                .execute_batch(
                    "INSERT INTO media_servers
                         (id, address, server_identifier, user_identifier, user_name, access_token)
                     VALUES (7, 'http://server', 'server', 'user', 'Listener', 'token');
                     INSERT INTO playlist_groups (id, name) VALUES (3, 'Moods');
                     INSERT INTO playlists (id, name, group_id, picture, created_at)
                         VALUES (4, 'Local', 3, X'0102', 123);
                     INSERT INTO playlists
                         (id, name, server_id, server_item, server_picture, group_id, created_at)
                         VALUES (5, 'Remote', 7, 'playlist', 1, 3, 456);
                     INSERT INTO playlist_entries (playlist_id, track_id, position) VALUES (4, 1, 0);",
                )
                .expect("upstream playlist state");
        }
        let database = init(directory.path().to_path_buf()).expect("migrate playlist sources");
        let saved = playlists(&database).expect("saved playlists");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].source, PlaylistSource::Local);
        assert_eq!(saved[0].id, 4);
        assert_eq!(saved[0].group_id, Some(3));
        assert_eq!(saved[0].track_ids, [1]);
        assert_eq!(playlist_picture(&database, 4), Ok(Some(vec![1, 2])));
        assert_eq!(saved[1].source, PlaylistSource::Jellyfin);
        assert_eq!(saved[1].server, Some((7, "playlist".to_owned())));
        assert_eq!(saved[1].group_id, Some(3));
        assert!(saved[1].server_picture);
    }

    #[test]
    fn provider_migration_tags_existing_server_rows_without_replacing_their_state() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        database_at_version(directory.path(), 9);
        {
            let connection =
                Connection::open(directory.path().join(DATABASE_FILE)).expect("open old database");
            connection
                .execute_batch(
                    "INSERT INTO media_servers
                         (id, address, server_identifier, user_identifier, user_name, access_token)
                     VALUES (7, 'http://server', 'server', 'user', 'Listener', 'token');
                     INSERT INTO tracks
                         (id, server_id, path, title, format, artwork_item, added_at, loudness_lufs)
                     VALUES (42, 7, 'jellyfin://server/song', 'Server Song', 'FLAC', 'picture', 123, -14);
                     INSERT INTO favourite_tracks (track_id, added_at) VALUES (42, 456);
                     INSERT INTO server_libraries
                         (id, server_id, library_identifier, name, shown)
                     VALUES (3, 7, 'music', 'Music', 0);
                     INSERT INTO track_libraries (track_id, library_id) VALUES (42, 3);",
                )
                .expect("seed old server state");
        }

        let database = init(directory.path().to_path_buf()).expect("migrate");
        let tracks = crate::library::list_tracks(&database).expect("mapped tracks");
        let file = tracks
            .iter()
            .find(|track| track.path == "/a.flac")
            .expect("file");
        assert_eq!(file.source, crate::library::TrackSource::File);
        assert_eq!(file.artwork_address, None);
        let server = tracks
            .iter()
            .find(|track| track.id == 42)
            .expect("server song");
        assert_eq!(server.source, crate::library::TrackSource::Jellyfin);
        assert_eq!(server.server_id, Some(7));
        assert_eq!(server.artwork_item.as_deref(), Some("picture"));
        assert_eq!(server.artwork_address, None);
        assert_eq!(server.server_libraries, [3]);
        assert_eq!(server.added_at, 123);
        assert_eq!(favourite_track_ids(&database), Ok(vec![42]));
        assert!(!server_libraries(&database).expect("libraries")[0].shown);
        assert_eq!(youtube_music_last_sync(&database), Ok(None));
        let connection = database.lock();
        let state: (f64, i64) = connection
            .query_row(
                "SELECT loudness_lufs, favourite_tracks.added_at
                 FROM tracks JOIN favourite_tracks ON track_id = tracks.id WHERE tracks.id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("retained state");
        assert_eq!(state, (-14.0, 456));
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
            assert_eq!(server_libraries(&database), Ok(Vec::new()));
            assert_eq!(favourite_track_ids(&database), Ok(Vec::new()));
            assert_eq!(playlists(&database), Ok(Vec::new()));
            assert_eq!(playlist_groups(&database), Ok(Vec::new()));
            assert_eq!(youtube_music_cookie_file(&database), Ok(None));
            assert_eq!(youtube_music_last_sync(&database), Ok(None));
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
                "INSERT INTO tracks (server_id, source, path, title, format) VALUES (?1, 'jellyfin', ?2, 'T', 'FLAC')",
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
        assert_ne!(other.id, first.id, "the replacement has a row of its own");
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
    fn a_database_from_a_newer_janis_opens_as_it_is() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        database_at_version(directory.path(), SCHEMA_VERSION);
        Connection::open(directory.path().join(DATABASE_FILE))
            .and_then(|connection| {
                connection.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            })
            .expect("stamp a later version");

        let database = init(directory.path().to_path_buf()).expect("a newer database opens");
        assert_eq!(
            get_preferences(&database).map(|preferences| preferences.volume),
            Ok(0.3)
        );
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

    #[test]
    fn a_playback_switch_is_found_by_the_name_the_settings_screen_gives_it() {
        for option in PlaybackOption::ALL {
            assert_eq!(
                PlaybackOption::from_identifier(option.column()),
                Some(option)
            );
        }
        assert_eq!(PlaybackOption::from_identifier("loudness"), None);
    }

    fn fresh_database() -> (tempfile::TempDir, DbState) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = init(directory.path().to_path_buf()).expect("init a new database");
        (directory, database)
    }

    #[test]
    fn every_preference_reads_back_what_was_written() {
        let (_directory, database) = fresh_database();
        let mut gains = vec![0.0; EQ_BAND_COUNT];
        gains[0] = 4.5;
        gains[9] = -2.0;

        set_volume(&database, 0.42).expect("the volume is saved");
        set_eq(&database, &gains, "custom").expect("the equalizer is saved");
        set_eq_linear_phase(&database, true).expect("the mode is saved");
        set_language(&database, "fr").expect("the language is saved");
        for option in PlaybackOption::ALL {
            set_playback_option(&database, option, option != PlaybackOption::Gapless)
                .expect("the switch is saved");
        }

        assert_eq!(
            get_preferences(&database),
            Ok(Preferences {
                volume: 0.42,
                eq_gains: gains,
                eq_preset: "custom".to_owned(),
                eq_linear_phase: true,
                gapless: false,
                crossfade: true,
                normalize: true,
                exclusive: true,
                language: "fr".to_owned(),
            })
        );
    }

    #[test]
    fn a_volume_outside_the_range_is_saved_at_its_edge() {
        let (_directory, database) = fresh_database();
        set_volume(&database, 1.7).expect("saved");
        assert_eq!(
            get_preferences(&database).map(|saved| saved.volume),
            Ok(1.0)
        );
        set_volume(&database, -1.0).expect("saved");
        assert_eq!(
            get_preferences(&database).map(|saved| saved.volume),
            Ok(0.0)
        );
    }

    #[test]
    fn equalizer_gains_of_the_wrong_count_are_refused_and_change_nothing() {
        let (_directory, database) = fresh_database();
        assert!(set_eq(&database, &[1.0, 2.0], "custom").is_err());
        assert_eq!(
            get_preferences(&database).map(|saved| (saved.eq_gains, saved.eq_preset)),
            Ok((vec![0.0; EQ_BAND_COUNT], "flat".to_owned()))
        );
    }

    #[test]
    fn unreadable_saved_gains_read_back_flat() {
        let (_directory, database) = fresh_database();
        database
            .lock()
            .execute("UPDATE user_preferences SET eq_gains = 'not json'", [])
            .expect("corrupt the gains");
        assert_eq!(
            get_preferences(&database).map(|saved| saved.eq_gains),
            Ok(vec![0.0; EQ_BAND_COUNT])
        );
    }

    #[test]
    fn a_sync_records_when_it_happened() {
        let (_directory, database) = fresh_database();
        let saved =
            save_media_server(&database, &new_server("server", "user", "token")).expect("saved");
        assert_eq!(saved.last_sync_at, None);

        set_last_sync(&database, saved.id).expect("the sync is recorded");

        let synced = media_server(&database)
            .expect("read")
            .expect("the server is kept");
        assert!(synced.last_sync_at.is_some_and(|time| time > 0));
    }

    #[test]
    fn a_kept_server_never_shows_its_token_when_debugged() {
        let new = new_server("server", "user", "secret-token");
        assert!(!format!("{new:?}").contains("secret-token"));
        let (_directory, database) = fresh_database();
        let saved = save_media_server(&database, &new).expect("saved");
        assert!(!format!("{saved:?}").contains("secret-token"));
    }
}
