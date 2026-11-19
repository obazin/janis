//! The track library: watched folders, the scanner, and cover art — the
//! database side of Janis's music collection.
//!
//! Per-file tag/property/cover reading lives in the `audio-stack-rs` facade
//! (`crate::audio::{read_metadata, read_cover, audio_extension}`); this module
//! walks folders, upserts the results into `tracks` keyed by absolute path (so
//! a rescan updates in place and never duplicates), and resolves playback gain.
//! Remote songs are rows too, keyed by a provider address instead of a path.
//! Complete provider listings update their own rows; the file scanner, pruner,
//! cover reader and background loudness reader only visit file-owned rows.
//! Every scan here blocks for as long as the walk takes, so callers run them
//! off the user-interface thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::audio::{
    audio_extension, gain_db, read_cover, read_metadata, CoverArt, Measured, Metadata,
};
use crate::persistence::DbState;

/// The owner of a persistent track, independent of provider-specific columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackSource {
    File,
    Jellyfin,
    YouTubeMusic,
}

impl TrackSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Jellyfin => "jellyfin",
            Self::YouTubeMusic => "youtube-music",
        }
    }
}

impl rusqlite::types::FromSql for TrackSource {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        match value.as_str()? {
            "file" => Ok(Self::File),
            "jellyfin" => Ok(Self::Jellyfin),
            "youtube-music" => Ok(Self::YouTubeMusic),
            source => Err(rusqlite::types::FromSqlError::Other(
                format!("unknown track source: {source}").into(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: i64,
    pub source: TrackSource,
    /// A provider's stable cover address, never a signed media URL.
    pub artwork_address: Option<String>,
    pub folder_id: Option<i64>,
    /// The Jellyfin server the song lives on; `None` for other sources.
    pub server_id: Option<i64>,
    /// The server item whose picture is the song's cover — its own, or its
    /// album's. `None` for other sources, and for a server song with no picture.
    pub artwork_item: Option<String>,
    /// The server libraries the song is listed in, by their row in
    /// `server_libraries`. Empty for other sources.
    pub server_libraries: Vec<i64>,
    pub path: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub composer: Option<String>,
    pub duration_secs: f64,
    pub format: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub lossless: bool,
    pub added_at: i64,
    /// Position within the album. From the tags, or recovered from the
    /// filename when the tags are silent.
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    /// The album's own artist, which on a compilation is not the track's.
    pub album_artist: Option<String>,
    /// Playback gain in dB, already resolved from the ReplayGain tags or the
    /// measured loudness. Zero when neither is known. Resolved here so
    /// nothing downstream has to re-derive it.
    pub gain_db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WatchedFolder {
    pub id: i64,
    pub path: String,
    pub track_count: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
}

/// Where an upserted row comes from, with only its owner's associated fields.
#[derive(Clone, Copy, Debug)]
enum Origin<'a> {
    File {
        folder_id: Option<i64>,
    },
    Server {
        server_id: i64,
        artwork_item: Option<&'a str>,
    },
    YouTubeMusic {
        artwork_address: Option<&'a str>,
    },
}

/// The row an upsert wrote.
#[derive(Clone, Copy, Debug)]
struct Upserted {
    track_id: i64,
    /// The row was newly inserted rather than updated.
    added: bool,
}

/// Upserts metadata without replacing the track's identity, stars or loudness.
fn upsert_track(
    conn: &rusqlite::Connection,
    origin: Origin<'_>,
    path: &str,
    meta: &Metadata,
) -> Result<Upserted, String> {
    let (source, folder_id, server_id, artwork_item, artwork_address) = match origin {
        Origin::File { folder_id } => (TrackSource::File, folder_id, None, None, None),
        Origin::Server {
            server_id,
            artwork_item,
        } => (
            TrackSource::Jellyfin,
            None,
            Some(server_id),
            artwork_item,
            None,
        ),
        Origin::YouTubeMusic { artwork_address } => {
            (TrackSource::YouTubeMusic, None, None, None, artwork_address)
        }
    };
    let existed: bool = conn
        .query_row("SELECT 1 FROM tracks WHERE path = ?1", [path], |_| Ok(()))
        .map(|_| true)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })
        .map_err(|e| format!("probe track row: {}", e))?;

    let track_id: i64 = conn
        .query_row(
            "INSERT INTO tracks (folder_id, path, title, artist, album, composer, duration_secs,
                             format, sample_rate, bit_depth, channels, lossless,
                             track_number, track_total, disc_number, disc_total,
                             year, genre, album_artist,
                             rg_track_gain_db, rg_track_peak,
                             rg_album_gain_db, rg_album_peak,
                             server_id, artwork_item, source, artwork_address)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                 ?26, ?27)
         ON CONFLICT(path) DO UPDATE SET
             -- An ad-hoc import of a file a watched folder already holds
             -- leaves it in that folder.
             folder_id = COALESCE(excluded.folder_id, tracks.folder_id),
             title = excluded.title,
             artist = excluded.artist,
             album = excluded.album,
             composer = excluded.composer,
             duration_secs = excluded.duration_secs,
             format = excluded.format,
             sample_rate = excluded.sample_rate,
             bit_depth = excluded.bit_depth,
             channels = excluded.channels,
             lossless = excluded.lossless,
             track_number = excluded.track_number,
             track_total = excluded.track_total,
             disc_number = excluded.disc_number,
             disc_total = excluded.disc_total,
             year = excluded.year,
             genre = excluded.genre,
             album_artist = excluded.album_artist,
             rg_track_gain_db = excluded.rg_track_gain_db,
             rg_track_peak = excluded.rg_track_peak,
             rg_album_gain_db = excluded.rg_album_gain_db,
             rg_album_peak = excluded.rg_album_peak,
             server_id = excluded.server_id,
             artwork_item = excluded.artwork_item,
             artwork_address = excluded.artwork_address
         WHERE tracks.source = excluded.source
         RETURNING id",
            rusqlite::params![
                folder_id,
                path,
                meta.title,
                meta.artist,
                meta.album,
                meta.composer,
                meta.duration_secs,
                meta.format,
                meta.sample_rate,
                meta.bit_depth,
                meta.channels,
                meta.lossless as i64,
                meta.track_number,
                meta.track_total,
                meta.disc_number,
                meta.disc_total,
                meta.year,
                meta.genre,
                meta.album_artist,
                meta.rg_track_gain_db,
                meta.rg_track_peak,
                meta.rg_album_gain_db,
                meta.rg_album_peak,
                server_id,
                artwork_item,
                source.as_str(),
                artwork_address,
            ],
            |row| row.get(0),
        )
        .map_err(|e| format!("upsert track {}: {}", path, e))?;
    Ok(Upserted {
        track_id,
        added: !existed,
    })
}

/// Files read per database visit during a scan. The walk and the tag probes
/// are the slow part and run without the lock; each flush then holds it only
/// for a burst of upserts.
const SCAN_BATCH: usize = 64;

/// Walks `folder` and upserts every audio file found under it.
///
/// Deliberately does *not* hold the database mutex across the walk: the audio
/// engine thread takes the same mutex for its short loudness reads and writes,
/// and a multi-minute walk holding it would starve the engine — silence and a
/// frozen transport for the duration of the scan.
fn scan_folder(db: &DbState, folder_id: i64, folder: &Path) -> ScanReport {
    let mut report = ScanReport::default();
    let mut batch: Vec<(String, Metadata)> = Vec::with_capacity(SCAN_BATCH);
    // Links are followed, to a file or a folder on another disk alike; the
    // walk notices a link back up the folder and reports it as an error.
    for entry in WalkDir::new(folder).follow_links(true) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // A folder that cannot be read, a link back up the tree or a
                // link to nothing: what lies beyond it is not scanned, and the
                // report counts it.
                log::warn!("skipped while scanning: {error}");
                report.skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if audio_extension(path).is_none() {
            continue;
        }
        // Paths are stored as text. One that is not valid Unicode could only
        // be stored altered, as the path of a file that does not exist.
        let Some(path_text) = path.to_str() else {
            log::warn!("skipped, its path is not valid Unicode: {}", path.display());
            report.skipped += 1;
            continue;
        };
        match read_metadata(path) {
            Ok(meta) => {
                batch.push((path_text.to_owned(), meta));
                if batch.len() >= SCAN_BATCH {
                    flush_batch(db, Some(folder_id), &mut batch, &mut report);
                }
            }
            Err(e) => {
                log::warn!("{}", e);
                report.skipped += 1;
            }
        }
    }
    flush_batch(db, Some(folder_id), &mut batch, &mut report);
    report
}

/// Writes one batch of scanned files, taking the lock only for the writes —
/// and in one transaction, so the burst costs one commit rather than one per
/// row.
fn flush_batch(
    db: &DbState,
    folder_id: Option<i64>,
    batch: &mut Vec<(String, Metadata)>,
    report: &mut ScanReport,
) {
    if batch.is_empty() {
        return;
    }
    let conn = db.lock_for_library_change();
    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(e) => {
            log::warn!("scan batch transaction: {}", e);
            report.skipped += batch.drain(..).count();
            return;
        }
    };
    let mut written = ScanReport::default();
    for (path, meta) in batch.drain(..) {
        match upsert_track(&tx, Origin::File { folder_id }, &path, &meta) {
            Ok(Upserted { added: true, .. }) => written.added += 1,
            Ok(Upserted { added: false, .. }) => written.updated += 1,
            Err(e) => {
                log::warn!("{}", e);
                written.skipped += 1;
            }
        }
    }
    match tx.commit() {
        Ok(()) => {
            report.added += written.added;
            report.updated += written.updated;
            report.skipped += written.skipped;
        }
        // Nothing of the batch was kept.
        Err(e) => {
            log::warn!("scan batch commit: {}", e);
            report.skipped += written.added + written.updated + written.skipped;
        }
    }
}

/// Deletes rows whose file vanished from disk. Returns the number removed.
///
/// "Vanished" requires the place the file lived to still be reachable: a track
/// is only pruned when its watched folder's root (or, for an ad-hoc import, its
/// parent directory) is a directory right now and the file is confirmed absent
/// from it — a file beneath a folder that cannot be read is kept.
/// The guard is what keeps a rescan with an external drive ejected from
/// reading every row as missing and deleting the whole library — along with
/// the loudness measurements a rescan can never rebuild.
fn prune_missing(db: &DbState) -> Result<usize, String> {
    let (tracks, folders) = {
        let conn = db.lock();
        let tracks: Vec<(i64, Option<i64>, String)> = conn
            // Remote sources are never inspected as files.
            .prepare("SELECT id, folder_id, path FROM tracks WHERE source = 'file'")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect()
            })
            .map_err(|e| format!("list track paths: {}", e))?;
        let folders: Vec<(i64, String)> = conn
            .prepare("SELECT id, path FROM watched_folders")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|e| format!("list folders: {}", e))?;
        (tracks, folders)
    };

    // The filesystem checks run without the lock — stat over a big library can
    // be slow, and the engine thread shares the mutex.
    let reachable: std::collections::HashMap<i64, bool> = folders
        .into_iter()
        .map(|(id, path)| (id, Path::new(&path).is_dir()))
        .collect();
    let doomed: Vec<i64> = tracks
        .into_iter()
        .filter(|(_, folder_id, path)| {
            let path = Path::new(path);
            let root_reachable = match folder_id {
                Some(folder_id) => reachable.get(folder_id).copied().unwrap_or(false),
                None => path.parent().is_some_and(Path::is_dir),
            };
            // Only a stat that answers "not found" is absence: a directory
            // denying access, or any other error, says nothing about the file.
            root_reachable && matches!(path.try_exists(), Ok(false))
        })
        .map(|(id, _, _)| id)
        .collect();

    let conn = db.lock_for_library_change();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("prune transaction: {}", e))?;
    for id in &doomed {
        tx.execute("DELETE FROM tracks WHERE id = ?1 AND source = 'file'", [id])
            .map_err(|e| format!("prune track {}: {}", id, e))?;
    }
    tx.commit().map_err(|e| format!("prune commit: {}", e))?;
    Ok(doomed.len())
}

/// A read of the whole library — songs, folders, the server's libraries,
/// favourites, playlists and groups — as one state of the database, with
/// the revision it was read at. Taken off the interface thread.
#[derive(Debug)]
pub struct LibrarySnapshot {
    pub revision: u64,
    pub tracks: Vec<Track>,
    pub folders: Vec<WatchedFolder>,
    pub libraries: Vec<crate::persistence::ServerLibrary>,
    pub favourite_track_ids: Vec<i64>,
    pub playlists: Vec<crate::persistence::SavedPlaylist>,
    pub playlist_groups: Vec<crate::persistence::PlaylistGroup>,
}

impl LibrarySnapshot {
    /// Reads every part under one lock, so no change lands between two of
    /// them, and the revision with them.
    pub fn read(database: &DbState) -> Result<Self, String> {
        let connection = database.lock();
        Ok(Self {
            revision: database.library_revision(),
            tracks: list_tracks_in(&connection)?,
            folders: list_watched_folders_in(&connection)?,
            libraries: crate::persistence::server_libraries_in(&connection)?,
            favourite_track_ids: crate::persistence::favourite_track_ids_in(&connection)?,
            playlists: crate::persistence::playlists_in(&connection)?,
            playlist_groups: crate::persistence::playlist_groups_in(&connection)?,
        })
    }

    /// True while nothing has changed the library since this was read.
    pub fn is_current(&self, database: &DbState) -> bool {
        self.revision == database.library_revision()
    }
}

pub fn list_tracks(db: &DbState) -> Result<Vec<Track>, String> {
    list_tracks_in(&db.lock())
}

/// [`list_tracks`], on a connection the caller holds.
pub(crate) fn list_tracks_in(conn: &rusqlite::Connection) -> Result<Vec<Track>, String> {
    let mut libraries_of_track: std::collections::HashMap<i64, Vec<i64>> =
        std::collections::HashMap::new();
    conn.prepare("SELECT track_id, library_id FROM track_libraries ORDER BY library_id")
        .and_then(|mut statement| {
            for membership in
                statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
            {
                let (track_id, library_id) = membership?;
                libraries_of_track
                    .entry(track_id)
                    .or_default()
                    .push(library_id);
            }
            Ok(())
        })
        .map_err(|error| format!("read the songs' libraries: {error}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, folder_id, path, title, artist, album, composer, duration_secs,
                    format, sample_rate, bit_depth, channels, lossless, added_at,
                    track_number, track_total, disc_number, disc_total,
                    year, genre, album_artist,
                    rg_track_gain_db, rg_track_peak, loudness_lufs, loudness_peak,
                    server_id, artwork_item, rg_album_gain_db, rg_album_peak,
                    source, artwork_address
             FROM tracks ORDER BY added_at DESC, id DESC",
        )
        .map_err(|e| format!("prepare list_tracks: {}", e))?;
    let tracks = stmt
        .query_map([], |row| {
            Ok(Track {
                id: row.get(0)?,
                source: row.get(29)?,
                artwork_address: row.get(30)?,
                folder_id: row.get(1)?,
                server_id: row.get(25)?,
                artwork_item: row.get(26)?,
                server_libraries: Vec::new(),
                path: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                album: row.get(5)?,
                composer: row.get(6)?,
                duration_secs: row.get(7)?,
                format: row.get(8)?,
                sample_rate: row.get(9)?,
                bit_depth: row.get(10)?,
                channels: row.get(11)?,
                lossless: row.get::<_, i64>(12)? != 0,
                added_at: row.get(13)?,
                track_number: row.get(14)?,
                track_total: row.get(15)?,
                disc_number: row.get(16)?,
                disc_total: row.get(17)?,
                year: row.get(18)?,
                genre: row.get(19)?,
                album_artist: row.get(20)?,
                gain_db: resolved_gain(
                    Gains {
                        album: row.get(27)?,
                        album_peak: row.get(28)?,
                        track: row.get(21)?,
                        track_peak: row.get(22)?,
                    },
                    row.get(23)?,
                    row.get(24)?,
                ),
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("read tracks: {}", e))?;
    let tracks = tracks
        .into_iter()
        .map(|mut track| {
            if let Some(libraries) = libraries_of_track.remove(&track.id) {
                track.server_libraries = libraries;
            }
            track
        })
        .collect();
    Ok(tracks)
}

pub fn list_watched_folders(db: &DbState) -> Result<Vec<WatchedFolder>, String> {
    list_watched_folders_in(&db.lock())
}

/// [`list_watched_folders`], on a connection the caller holds.
pub(crate) fn list_watched_folders_in(
    conn: &rusqlite::Connection,
) -> Result<Vec<WatchedFolder>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT f.id, f.path, COUNT(t.id) FROM watched_folders f
             LEFT JOIN tracks t ON t.folder_id = f.id AND t.source = 'file'
             GROUP BY f.id ORDER BY f.added_at",
        )
        .map_err(|e| format!("prepare list_watched_folders: {}", e))?;
    let folders = stmt
        .query_map([], |row| {
            Ok(WatchedFolder {
                id: row.get(0)?,
                path: row.get(1)?,
                track_count: row.get(2)?,
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("read watched folders: {}", e))?;
    Ok(folders)
}

/// Why a change to the library did not happen.
#[derive(Debug, PartialEq)]
pub enum ScanError {
    /// The folder lies inside one already watched, whose path this is.
    InsideWatched(String),
    /// Anything else, with its detail for the log.
    Failed(String),
}

impl From<String> for ScanError {
    fn from(detail: String) -> Self {
        ScanError::Failed(detail)
    }
}

/// Registers a folder and scans it. The scan is a long walk + tag reads.
pub fn add_watched_folder(db: &DbState, path: &str) -> Result<ScanReport, ScanError> {
    let folder = PathBuf::from(path);
    if !folder.is_dir() {
        return Err(ScanError::Failed(format!("not a directory: {}", path)));
    }
    // The lock covers only the registration; the walk itself runs unlocked
    // (see `scan_folder`).
    let folder_id: i64 = {
        let connection = db.lock_for_library_change();
        let watched: Vec<(i64, String)> = connection
            .prepare("SELECT id, path FROM watched_folders")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|error| format!("list folders: {error}"))?;
        // A track belongs to one folder: one inside a watched folder would
        // take its tracks from it, and removing it would remove them.
        if let Some((_, parent)) = watched
            .iter()
            .find(|(_, watched)| strictly_inside(Path::new(watched), &folder))
        {
            return Err(ScanError::InsideWatched(parent.clone()));
        }
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("register folder: {error}"))?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO watched_folders (path) VALUES (?1)",
                [path],
            )
            .map_err(|error| format!("register folder: {error}"))?;
        let folder_id: i64 = transaction
            .query_row(
                "SELECT id FROM watched_folders WHERE path = ?1",
                [path],
                |row| row.get(0),
            )
            .map_err(|error| format!("resolve folder id: {error}"))?;
        // The watched folders inside this one are taken over: their tracks
        // move to it, and their rows go.
        for (inner_id, _) in watched
            .iter()
            .filter(|(_, watched)| strictly_inside(&folder, Path::new(watched)))
        {
            transaction
                .execute(
                    "UPDATE tracks SET folder_id = ?1 WHERE folder_id = ?2 AND source = 'file'",
                    [folder_id, *inner_id],
                )
                .and_then(|_| {
                    transaction.execute("DELETE FROM watched_folders WHERE id = ?1", [inner_id])
                })
                .map_err(|error| format!("take over a watched folder: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("register folder: {error}"))?;
        folder_id
    };
    Ok(scan_folder(db, folder_id, &folder))
}

/// Whether `inner` lies inside `outer` and is not `outer` itself, compared
/// whole component by whole component on the paths with links resolved, so
/// `/music/a` holds `/music/a/b` but not `/music/ab`. A folder that cannot be
/// resolved (an ejected drive) is compared as written.
fn strictly_inside(outer: &Path, inner: &Path) -> bool {
    let resolved = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let (outer, inner) = (resolved(outer), resolved(inner));
    inner != outer && inner.starts_with(&outer)
}

pub fn remove_watched_folder(db: &DbState, folder_id: i64) -> Result<(), String> {
    let conn = db.lock_for_library_change();
    conn.execute("DELETE FROM watched_folders WHERE id = ?1", [folder_id])
        .map_err(|e| format!("remove folder: {}", e))?;
    Ok(())
}

/// Ad-hoc single-file import (the Add-music dialog and OS drag-and-drop). Files
/// land with no watched folder; a rescan keeps them as long as the file still
/// exists.
pub fn import_files(db: &DbState, paths: Vec<String>) -> ScanReport {
    let mut report = ScanReport::default();
    // Same lock discipline as a folder scan: probe files unlocked, write in
    // bursts.
    let mut batch: Vec<(String, Metadata)> = Vec::with_capacity(SCAN_BATCH);
    for path in paths {
        match read_metadata(Path::new(&path)) {
            Ok(meta) => {
                batch.push((path, meta));
                if batch.len() >= SCAN_BATCH {
                    flush_batch(db, None, &mut batch, &mut report);
                }
            }
            Err(e) => {
                log::warn!("{}", e);
                report.skipped += 1;
            }
        }
    }
    flush_batch(db, None, &mut batch, &mut report);
    report
}

/// One song as a media server lists it.
#[derive(Clone, Debug)]
pub struct ServerSong {
    /// Its `jellyfin://<server>/<item>` address, the row's unique key.
    pub address: String,
    pub metadata: Metadata,
    /// The item whose picture is the song's cover, when the server has one.
    pub artwork_item: Option<String>,
    /// The server's identifiers of the libraries the song is listed in.
    pub libraries: Vec<String>,
    /// The listener starred it on the server.
    pub favourite: bool,
}

/// One library of a media server, as its listing names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedLibrary {
    /// The server's own identifier of the library.
    pub identifier: String,
    pub name: String,
}

/// One playlist of a media server, as its listing gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPlaylist {
    /// The server's own identifier of the playlist.
    pub item: String,
    pub name: String,
    /// The server has a picture of it.
    pub picture: bool,
    /// The `jellyfin://<server>/<item>` addresses of its songs, in order.
    pub songs: Vec<String>,
}

/// Brings the server's playlists in line with a complete listing of them,
/// once its songs are: each listed playlist is upserted by its item, which
/// keeps the group and the picture the listener gave it, and holds the
/// listed songs Janis has, in order, each once at its first place. A
/// playlist of this server missing from the listing is removed.
///
/// Same lock discipline as a scan: the writes go in bursts of
/// `PLAYLIST_BURST` rows, one transaction each, so a server with many long
/// playlists never holds the lock the interface's preference writes and the
/// engine's loudness reads wait on.
pub fn sync_server_playlists(
    database: &DbState,
    server_id: i64,
    playlists: &[ListedPlaylist],
) -> Result<(), String> {
    let tracks: std::collections::HashMap<String, i64> = database
        .lock()
        .prepare("SELECT path, id FROM tracks WHERE server_id = ?1")
        .and_then(|mut statement| {
            statement
                .query_map([server_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|error| format!("list the server's songs: {error}"))?;
    let writes: Vec<PlaylistWrite> = playlists
        .iter()
        .flat_map(|playlist| {
            std::iter::once(PlaylistWrite::Keep(playlist)).chain(
                playlist
                    .songs
                    .iter()
                    .filter_map(|address| tracks.get(address).copied())
                    .map(|track_id| PlaylistWrite::Entry { track_id }),
            )
        })
        .collect();

    let mut kept = std::collections::HashSet::new();
    // The playlist the entries that follow belong to, and the place of its
    // next song.
    let mut filling: Option<(i64, &str, i64)> = None;
    for burst in writes.chunks(PLAYLIST_BURST) {
        let connection = database.lock_for_library_change();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("playlist sync transaction: {error}"))?;
        for write in burst {
            match write {
                PlaylistWrite::Keep(playlist) => {
                    let playlist_id: i64 = transaction
                        .query_row(
                            "INSERT INTO playlists
                                 (name, server_id, server_item, server_picture, source)
                             VALUES (?1, ?2, ?3, ?4, 'jellyfin')
                             ON CONFLICT(server_id, server_item) DO UPDATE SET
                                 name = excluded.name,
                                 server_picture = excluded.server_picture
                             RETURNING id",
                            rusqlite::params![
                                playlist.name,
                                server_id,
                                playlist.item,
                                playlist.picture
                            ],
                            |row| row.get(0),
                        )
                        .map_err(|error| format!("keep playlist {}: {error}", playlist.name))?;
                    kept.insert(playlist_id);
                    transaction
                        .execute(
                            "DELETE FROM playlist_entries WHERE playlist_id = ?1",
                            [playlist_id],
                        )
                        .map_err(|error| format!("empty playlist {}: {error}", playlist.name))?;
                    filling = Some((playlist_id, playlist.name.as_str(), 0));
                }
                PlaylistWrite::Entry { track_id } => {
                    let Some((playlist_id, name, position)) = filling.as_mut() else {
                        continue;
                    };
                    let added = transaction
                        .execute(
                            "INSERT OR IGNORE INTO playlist_entries
                                 (playlist_id, track_id, position)
                             VALUES (?1, ?2, ?3)",
                            rusqlite::params![*playlist_id, track_id, *position],
                        )
                        .map_err(|error| format!("fill playlist {name}: {error}"))?;
                    *position += added as i64;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("playlist sync commit: {error}"))?;
        drop(connection);
        database.give_way();
    }

    let connection = database.lock_for_library_change();
    let stored: Vec<i64> = connection
        .prepare("SELECT id FROM playlists WHERE server_id = ?1")
        .and_then(|mut statement| {
            statement
                .query_map([server_id], |row| row.get(0))?
                .collect()
        })
        .map_err(|error| format!("list the server's playlists: {error}"))?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("playlist prune transaction: {error}"))?;
    for playlist_id in stored {
        if !kept.contains(&playlist_id) {
            transaction
                .execute("DELETE FROM playlists WHERE id = ?1", [playlist_id])
                .map_err(|error| format!("remove playlist {playlist_id}: {error}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("playlist prune commit: {error}"))
}

/// How many rows a burst of the playlist sync writes in one transaction.
const PLAYLIST_BURST: usize = 512;

/// One row the playlist sync writes: a listed playlist, kept and emptied, or
/// one of its songs, in order, after it.
enum PlaylistWrite<'listing> {
    Keep(&'listing ListedPlaylist),
    Entry { track_id: i64 },
}

/// Brings the server's libraries and songs in the library in line with a
/// complete listing of them: every listed library and song is upserted, and
/// every library or song of this server missing from the listing is removed
/// — the listing is complete, so missing means gone from the server. A
/// listing that could not be read whole must not reach here: it would read as
/// songs deleted. A library keeps whether the listener shows it.
///
/// Same lock discipline as a folder scan: bursts of `SCAN_BATCH` upserts, one
/// transaction each, so the engine's loudness reads are never starved.
pub fn sync_server_tracks(
    database: &DbState,
    server_id: i64,
    libraries: &[ListedLibrary],
    songs: Vec<ServerSong>,
) -> Result<ScanReport, String> {
    let library_rows = sync_server_libraries(database, server_id, libraries)?;
    let mut report = ScanReport::default();
    let listed: std::collections::HashSet<String> =
        songs.iter().map(|song| song.address.clone()).collect();
    for burst in songs.chunks(SCAN_BATCH) {
        let connection = database.lock_for_library_change();
        let mut transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("server sync transaction: {error}"))?;
        let mut written = ScanReport::default();
        for song in burst {
            match write_server_song(&mut transaction, server_id, song, &library_rows) {
                Ok(Upserted { added: true, .. }) => written.added += 1,
                Ok(Upserted { added: false, .. }) => written.updated += 1,
                Err(error) => {
                    log::warn!("{error}");
                    written.skipped += 1;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("server sync commit: {error}"))?;
        drop(connection);
        database.give_way();
        report.added += written.added;
        report.updated += written.updated;
        report.skipped += written.skipped;
    }

    let connection = database.lock_for_library_change();
    let stored: Vec<(i64, String)> = connection
        .prepare("SELECT id, path FROM tracks WHERE server_id = ?1")
        .and_then(|mut statement| {
            statement
                .query_map([server_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|error| format!("list the server's songs: {error}"))?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("server prune transaction: {error}"))?;
    for (track_id, address) in stored {
        if !listed.contains(&address) {
            transaction
                .execute("DELETE FROM tracks WHERE id = ?1", [track_id])
                .map_err(|error| format!("remove song {track_id}: {error}"))?;
            report.removed += 1;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("server prune commit: {error}"))?;
    Ok(report)
}

/// Upserts the server's listed libraries, in the order listed, and removes
/// those it no longer lists. Returns each listed library's row, by the
/// server's identifier.
fn sync_server_libraries(
    database: &DbState,
    server_id: i64,
    libraries: &[ListedLibrary],
) -> Result<std::collections::HashMap<String, i64>, String> {
    let connection = database.lock_for_library_change();
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("library sync transaction: {error}"))?;
    let mut rows = std::collections::HashMap::new();
    for (position, library) in libraries.iter().enumerate() {
        let row: i64 = transaction
            .query_row(
                "INSERT INTO server_libraries (server_id, library_identifier, name, position)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(server_id, library_identifier) DO UPDATE SET
                     name = excluded.name,
                     position = excluded.position
                 RETURNING id",
                rusqlite::params![server_id, library.identifier, library.name, position as i64],
                |row| row.get(0),
            )
            .map_err(|error| format!("keep library {}: {error}", library.name))?;
        rows.insert(library.identifier.clone(), row);
    }
    let stored: Vec<i64> = transaction
        .prepare("SELECT id FROM server_libraries WHERE server_id = ?1")
        .and_then(|mut statement| {
            statement
                .query_map([server_id], |row| row.get(0))?
                .collect()
        })
        .map_err(|error| format!("list the server's libraries: {error}"))?;
    let kept: std::collections::HashSet<i64> = rows.values().copied().collect();
    for library_id in stored {
        if !kept.contains(&library_id) {
            transaction
                .execute("DELETE FROM server_libraries WHERE id = ?1", [library_id])
                .map_err(|error| format!("remove library {library_id}: {error}"))?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("library sync commit: {error}"))?;
    Ok(rows)
}

/// Upserts one server song, files it in the libraries that list it and
/// gives it the server's star, as one step: should filing fail, the song is
/// left as it was, so it never ends up in no library — which would show it
/// whatever the switches say.
fn write_server_song(
    transaction: &mut rusqlite::Transaction<'_>,
    server_id: i64,
    song: &ServerSong,
    library_rows: &std::collections::HashMap<String, i64>,
) -> Result<Upserted, String> {
    // Dropped without a commit, the savepoint rolls this song's writes back.
    let savepoint = transaction
        .savepoint()
        .map_err(|error| format!("start writing {}: {error}", song.address))?;
    let origin = Origin::Server {
        server_id,
        artwork_item: song.artwork_item.as_deref(),
    };
    let upserted = upsert_track(&savepoint, origin, &song.address, &song.metadata)?;
    set_track_libraries(
        &savepoint,
        upserted.track_id,
        &song.address,
        &song.libraries,
        library_rows,
    )?;
    set_server_favourite(&savepoint, upserted.track_id, song)?;
    savepoint
        .commit()
        .map_err(|error| format!("finish writing {}: {error}", song.address))?;
    Ok(upserted)
}

/// Stars a server song as the server does, or takes its star away. A star
/// the song already had keeps its date, and so its place among the
/// favourites.
fn set_server_favourite(
    connection: &rusqlite::Connection,
    track_id: i64,
    song: &ServerSong,
) -> Result<(), String> {
    let statement = if song.favourite {
        "INSERT OR IGNORE INTO favourite_tracks (track_id) VALUES (?1)"
    } else {
        "DELETE FROM favourite_tracks WHERE track_id = ?1"
    };
    connection
        .execute(statement, [track_id])
        .map_err(|error| format!("star {} as the server does: {error}", song.address))?;
    Ok(())
}

/// Records the libraries a server song is listed in, in place of those it was
/// listed in before.
fn set_track_libraries(
    connection: &rusqlite::Connection,
    track_id: i64,
    address: &str,
    libraries: &[String],
    library_rows: &std::collections::HashMap<String, i64>,
) -> Result<(), String> {
    connection
        .execute(
            "DELETE FROM track_libraries WHERE track_id = ?1",
            [track_id],
        )
        .map_err(|error| format!("clear the libraries of {address}: {error}"))?;
    for library in libraries {
        let Some(library_id) = library_rows.get(library) else {
            continue;
        };
        connection
            .execute(
                "INSERT OR IGNORE INTO track_libraries (track_id, library_id) VALUES (?1, ?2)",
                [track_id, *library_id],
            )
            .map_err(|error| format!("file {address} in a library: {error}"))?;
    }
    Ok(())
}

/// One saved YouTube Music song, with a stable video identifier and cover.
#[derive(Clone, Debug)]
pub struct YouTubeMusicTrack {
    pub identifier: String,
    pub metadata: Metadata,
    pub artwork_address: Option<String>,
}

/// A complete account library, including songs only present in playlists.
#[derive(Clone, Debug, Default)]
pub struct YouTubeMusicLibrary {
    pub tracks: Vec<YouTubeMusicTrack>,
    pub playlists: Vec<YouTubeMusicPlaylist>,
}

/// One saved playlist, with the available songs in their provider order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeMusicPlaylist {
    pub identifier: String,
    pub name: String,
    pub track_identifiers: Vec<String>,
}

/// Imports a complete account listing. Every network page, including playlist
/// contents, must be read before calling: absent songs and playlists are
/// removed from this source only. Validation precedes all writes; retained
/// rows keep their identity, stars, grouping, pictures and measured loudness.
/// Writes release the mutex between bursts. The kept cookie-file path changes
/// with the completed listing, never before.
pub fn sync_youtube_music_library(
    database: &DbState,
    library: &YouTubeMusicLibrary,
    cookie_file: &Path,
) -> Result<ScanReport, String> {
    let cookie_file = cookie_file
        .to_str()
        .ok_or("cookie file path is not Unicode")?;
    let songs = &library.tracks;
    let mut listed = std::collections::HashSet::with_capacity(songs.len());
    for song in songs {
        if song.identifier.len() != 11
            || !song.identifier.bytes().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, b'_' | b'-')
            })
        {
            return Err("invalid YouTube Music video identifier".to_owned());
        }
        if !listed.insert(song.identifier.as_str()) {
            return Err("duplicate YouTube Music video identifier".to_owned());
        }
        let metadata = &song.metadata;
        if !metadata.duration_secs.is_finite()
            || metadata.duration_secs < 0.0
            || [
                metadata.rg_track_gain_db,
                metadata.rg_track_peak,
                metadata.rg_album_gain_db,
                metadata.rg_album_peak,
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err("invalid YouTube Music numeric metadata".to_owned());
        }
    }
    let mut playlist_identifiers =
        std::collections::HashSet::with_capacity(library.playlists.len());
    for playlist in &library.playlists {
        if playlist.identifier.is_empty()
            || !playlist_identifiers.insert(playlist.identifier.as_str())
        {
            return Err("invalid or duplicate YouTube Music playlist identifier".to_owned());
        }
        if playlist
            .track_identifiers
            .iter()
            .any(|identifier| !listed.contains(identifier.as_str()))
        {
            return Err("YouTube Music playlist references an unlisted song".to_owned());
        }
    }

    let mut report = ScanReport::default();
    for burst in songs.chunks(SCAN_BATCH) {
        let connection = database.lock_for_library_change();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("YouTube Music sync transaction: {error}"))?;
        let mut written = ScanReport::default();
        for song in burst {
            let address = format!("youtube-music://{}", song.identifier);
            let origin = Origin::YouTubeMusic {
                artwork_address: song.artwork_address.as_deref(),
            };
            let upserted = upsert_track(&transaction, origin, &address, &song.metadata)?;
            if upserted.added {
                written.added += 1;
            } else {
                written.updated += 1;
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("YouTube Music sync commit: {error}"))?;
        drop(connection);
        database.give_way();
        report.added += written.added;
        report.updated += written.updated;
    }

    let kept_playlists = sync_youtube_music_playlists(database, &library.playlists)?;

    let connection = database.lock_for_library_change();
    let stored: Vec<(i64, String)> = connection
        .prepare("SELECT id, path FROM tracks WHERE source = 'youtube-music'")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|error| format!("list YouTube Music songs: {error}"))?;
    let stored_playlists: Vec<i64> = connection
        .prepare("SELECT id FROM playlists WHERE source = 'youtube-music'")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|error| format!("list YouTube Music playlists: {error}"))?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("YouTube Music prune transaction: {error}"))?;
    for (track_id, address) in stored {
        let retained = address
            .strip_prefix("youtube-music://")
            .is_some_and(|identifier| listed.contains(identifier));
        if !retained {
            report.removed += transaction
                .execute(
                    "DELETE FROM tracks WHERE id = ?1 AND source = 'youtube-music'",
                    [track_id],
                )
                .map_err(|error| format!("remove YouTube Music song {track_id}: {error}"))?;
        }
    }
    for playlist_id in stored_playlists {
        if !kept_playlists.contains(&playlist_id) {
            transaction
                .execute("DELETE FROM playlists WHERE id = ?1", [playlist_id])
                .map_err(|error| format!("prune YouTube Music playlist: {error}"))?;
        }
    }
    transaction
        .execute(
            "UPDATE user_preferences SET youtube_music_last_sync_at = unixepoch(),
                 youtube_music_cookie_file = ?1, updated_at = unixepoch() WHERE id = 1",
            [cookie_file],
        )
        .map_err(|error| format!("record YouTube Music sync time: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("YouTube Music prune commit: {error}"))?;
    Ok(report)
}

/// Writes imported playlists in bounded bursts, preserving Janis-local
/// customization while replacing provider-owned names and membership.
fn sync_youtube_music_playlists(
    database: &DbState,
    playlists: &[YouTubeMusicPlaylist],
) -> Result<std::collections::HashSet<i64>, String> {
    if playlists.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let tracks: std::collections::HashMap<String, i64> = database
        .lock()
        .prepare(
            "SELECT substr(path, length('youtube-music://') + 1), id
             FROM tracks WHERE source = 'youtube-music'",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|error| format!("list YouTube Music playlist songs: {error}"))?;
    let mut writes = playlists
        .iter()
        .flat_map(|playlist| {
            std::iter::once(YouTubeMusicPlaylistWrite::Keep(playlist)).chain(
                playlist
                    .track_identifiers
                    .iter()
                    .map(|identifier| YouTubeMusicPlaylistWrite::Entry(identifier)),
            )
        })
        .peekable();
    let mut kept = std::collections::HashSet::with_capacity(playlists.len());
    let mut filling: Option<(i64, i64)> = None;
    while writes.peek().is_some() {
        let connection = database.lock_for_library_change();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("YouTube Music playlist transaction: {error}"))?;
        for write in writes.by_ref().take(PLAYLIST_BURST) {
            match write {
                YouTubeMusicPlaylistWrite::Keep(playlist) => {
                    let playlist_id: i64 = transaction
                        .query_row(
                            "INSERT INTO playlists (name, source, provider_identifier)
                             VALUES (?1, 'youtube-music', ?2)
                             ON CONFLICT(source, provider_identifier)
                             DO UPDATE SET name = excluded.name
                             RETURNING id",
                            rusqlite::params![playlist.name, playlist.identifier],
                            |row| row.get(0),
                        )
                        .map_err(|error| format!("keep YouTube Music playlist: {error}"))?;
                    transaction
                        .execute(
                            "DELETE FROM playlist_entries WHERE playlist_id = ?1",
                            [playlist_id],
                        )
                        .map_err(|error| format!("empty YouTube Music playlist: {error}"))?;
                    kept.insert(playlist_id);
                    filling = Some((playlist_id, 0));
                }
                YouTubeMusicPlaylistWrite::Entry(identifier) => {
                    let track_id = tracks
                        .get(identifier)
                        .ok_or("YouTube Music playlist song is missing from the database")?;
                    let (playlist_id, position) = filling
                        .as_mut()
                        .expect("playlist entries follow their playlist");
                    let added = transaction
                        .execute(
                            "INSERT OR IGNORE INTO playlist_entries
                                 (playlist_id, track_id, position)
                             VALUES (?1, ?2, ?3)",
                            rusqlite::params![*playlist_id, track_id, *position],
                        )
                        .map_err(|error| format!("fill YouTube Music playlist: {error}"))?;
                    *position += added as i64;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("YouTube Music playlist commit: {error}"))?;
        drop(connection);
        database.give_way();
    }
    Ok(kept)
}

enum YouTubeMusicPlaylistWrite<'listing> {
    Keep(&'listing YouTubeMusicPlaylist),
    Entry(&'listing str),
}

/// Removes this source's songs, playlists, stars and connection, not other music.
pub fn remove_youtube_music_tracks(database: &DbState) -> Result<(), String> {
    let connection = database.lock_for_library_change();
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| format!("YouTube Music removal transaction: {error}"))?;
    transaction
        .execute("DELETE FROM playlists WHERE source = 'youtube-music'", [])
        .map_err(|error| format!("remove YouTube Music playlists: {error}"))?;
    transaction
        .execute("DELETE FROM tracks WHERE source = 'youtube-music'", [])
        .map_err(|error| format!("remove YouTube Music songs: {error}"))?;
    transaction
        .execute(
            "UPDATE user_preferences SET youtube_music_last_sync_at = NULL,
                 youtube_music_cookie_file = NULL, updated_at = unixepoch() WHERE id = 1",
            [],
        )
        .map_err(|error| format!("clear YouTube Music sync time: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("YouTube Music removal commit: {error}"))
}

pub fn count_youtube_music_tracks(database: &DbState) -> Result<usize, String> {
    database
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM tracks WHERE source = 'youtube-music'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("count YouTube Music songs: {error}"))
}

/// Where a track's cover comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverSource {
    /// The file, or its album's files (`track_cover`).
    File,
    /// The server's picture of this item; `None` when the server has none.
    Server { artwork_item: Option<String> },
    /// A stable remote picture address; `None` when the provider has none.
    Address { artwork_address: Option<String> },
}

/// Where a track's cover comes from. An unknown track reads as a file, whose
/// lookup then finds nothing.
pub fn cover_source(database: &DbState, track_id: i64) -> CoverSource {
    let connection = database.lock();
    connection
        .query_row(
            "SELECT source, artwork_item, artwork_address FROM tracks WHERE id = ?1",
            [track_id],
            |row| {
                Ok(match row.get::<_, TrackSource>(0)? {
                    TrackSource::File => CoverSource::File,
                    TrackSource::Jellyfin => CoverSource::Server {
                        artwork_item: row.get(1)?,
                    },
                    TrackSource::YouTubeMusic => CoverSource::Address {
                        artwork_address: row.get(2)?,
                    },
                })
            },
        )
        .unwrap_or(CoverSource::File)
}

/// Re-walks every watched folder (picking up new + changed files) and prunes
/// rows whose file is gone.
pub fn rescan_library(db: &DbState) -> Result<ScanReport, String> {
    let folders: Vec<(i64, String)> = {
        let conn = db.lock();
        conn.prepare("SELECT id, path FROM watched_folders")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|e| format!("list folders: {}", e))?
    };
    let mut report = ScanReport::default();
    for (id, path) in folders {
        let root = Path::new(&path);
        if !root.is_dir() {
            // An ejected drive or offline share. WalkDir would swallow the
            // error and report an empty, healthy-looking folder; skipping
            // it keeps the report honest, and `prune_missing` independently
            // refuses to touch its tracks.
            log::warn!("watched folder unreachable, skipped: {}", path);
            continue;
        }
        let sub = scan_folder(db, id, root);
        report.added += sub.added;
        report.updated += sub.updated;
        report.skipped += sub.skipped;
    }
    report.removed = prune_missing(db)?;
    Ok(report)
}

/// The gains a track's tags — or its server — carry, in dB, each with the
/// linear peak measured alongside it when one was.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gains {
    pub album: Option<f64>,
    pub album_peak: Option<f64>,
    pub track: Option<f64>,
    pub track_peak: Option<f64>,
}

/// The gains measured while Janis runs, at most one per local file: a queue
/// takes a track's from here once it is known, ahead of the gain the library
/// read before the measurement.
///
/// Each gain is kept under the row and the path it was measured for: SQLite
/// hands a removed row's id to the next file added, and that file must not
/// play at the gain of the one it replaced.
#[derive(Debug, Default)]
pub struct SessionGains {
    measured: std::collections::HashMap<PendingLoudness, f64>,
}

impl SessionGains {
    /// The gain `track` plays at.
    pub fn gain_of(&self, track: &Track) -> f64 {
        self.measured
            .get(&PendingLoudness {
                track_id: track.id,
                path: track.path.clone(),
            })
            .copied()
            .unwrap_or(track.gain_db)
    }

    /// A measurement of `file` ended: the gain it plays at from now on.
    pub fn measured(&mut self, file: PendingLoudness, lufs: f64, peak: f64) -> f64 {
        let gain = resolved_gain(Gains::default(), Some(lufs), Some(peak));
        self.measured.insert(file, gain);
        gain
    }
}

/// The gain a track plays at, in dB.
///
/// The album gain first, so the songs of one record keep the loudness
/// differences their mastering gave them; the track gain when the album has
/// none; the measured loudness when there is no gain at all. Each gain keeps
/// its own peak, and borrows the other's when it has none, so a boost never
/// lifts the loudest sample past full scale for want of a figure.
pub fn resolved_gain(gains: Gains, lufs: Option<f64>, measured_peak: Option<f64>) -> f64 {
    let tagged = match (gains.album, gains.track) {
        (Some(album), _) if album.is_finite() => {
            Some((album, gains.album_peak.or(gains.track_peak)))
        }
        (_, Some(track)) if track.is_finite() => {
            Some((track, gains.track_peak.or(gains.album_peak)))
        }
        _ => None,
    };
    match tagged {
        Some((gain, peak)) => gain_db(Some(gain), None, peak.or(measured_peak)),
        None => gain_db(None, lufs, measured_peak),
    }
}

/// A file the background measurement still has to read: its row and its path
/// together, since a row id alone can name another file by the time the
/// measurement ends.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PendingLoudness {
    pub track_id: i64,
    pub path: String,
}

/// The files with no gain at all — no ReplayGain tag, no measurement — that
/// the background measurement should read, in library order. Remote songs
/// are left out: their audio is not on this disk.
pub fn tracks_needing_loudness(db: &DbState) -> Result<Vec<PendingLoudness>, String> {
    let conn = db.lock();
    let mut statement = conn
        .prepare(
            "SELECT id, path FROM tracks
             WHERE source = 'file'
               AND rg_track_gain_db IS NULL AND rg_album_gain_db IS NULL
               AND loudness_lufs IS NULL
             ORDER BY added_at DESC, id DESC",
        )
        .map_err(|e| format!("prepare tracks_needing_loudness: {}", e))?;
    let pending = statement
        .query_map([], |row| {
            Ok(PendingLoudness {
                track_id: row.get(0)?,
                path: row.get(1)?,
            })
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("read tracks needing loudness: {}", e))?;
    Ok(pending)
}

/// The engine's route to the loudness columns.
pub struct LoudnessStore(pub Arc<DbState>);

impl crate::audio::Store for LoudnessStore {
    fn needs_measurement(&self, track_id: i64) -> bool {
        needs_loudness(&self.0, track_id)
    }

    fn record(&self, track_id: i64, measured: Measured) {
        store_loudness(&self.0, track_id, measured.lufs, measured.peak);
    }
}

/// Whether a track still has no idea how loud it is.
///
/// True only when neither the file's tags nor a previous measurement said,
/// which is what decides if playing it is worth measuring.
pub fn needs_loudness(db: &DbState, track_id: i64) -> bool {
    let conn = db.lock();
    conn.query_row(
        "SELECT rg_track_gain_db IS NULL AND rg_album_gain_db IS NULL
                AND loudness_lufs IS NULL
         FROM tracks WHERE id = ?1",
        [track_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|needs| needs != 0)
    .unwrap_or(false)
}

/// Records a loudness measurement, taken while a track played or read from
/// its file in the background.
///
/// Written outside `upsert_track` on purpose: a rescan re-reads tags and would
/// otherwise wipe a measurement it knows nothing about.
pub fn store_loudness(db: &DbState, track_id: i64, lufs: f64, peak: f64) {
    let conn = db.lock();
    if let Err(e) = conn.execute(
        "UPDATE tracks SET loudness_lufs = ?2, loudness_peak = ?3 WHERE id = ?1",
        rusqlite::params![track_id, lufs, peak],
    ) {
        log::warn!("store loudness for track {}: {}", track_id, e);
    }
}

/// Records what the background measurement read from `file`, only while its
/// row still holds that file: one removed during the measurement may have
/// handed its id to a file added since. Returns whether it was stored.
pub fn store_measured_loudness(
    database: &DbState,
    file: &PendingLoudness,
    lufs: f64,
    peak: f64,
) -> bool {
    let connection = database.lock();
    match connection.execute(
        "UPDATE tracks SET loudness_lufs = ?3, loudness_peak = ?4
         WHERE id = ?1 AND path = ?2 AND source = 'file'",
        rusqlite::params![file.track_id, file.path, lufs, peak],
    ) {
        Ok(changed) => changed > 0,
        Err(error) => {
            log::warn!("store loudness for track {}: {error}", file.track_id);
            false
        }
    }
}

/// Cover art for one track, as the file embeds it.
///
/// Falls back to the rest of the album: a rip where only the first file carries
/// the picture is common, and every track on that record should still show the
/// sleeve. `None` only when nothing in the album has art — the tile then draws
/// its generated gradient. Opens files, so it runs off the user-interface
/// thread.
pub fn track_cover(db: &DbState, track_id: i64) -> Option<CoverArt> {
    cover_candidates(db, track_id)
        .iter()
        .find_map(|path| read_cover(path))
}

/// How many album siblings to open before giving up. A record whose first dozen
/// files carry no art almost certainly has none.
const COVER_SIBLING_LIMIT: usize = 12;

/// The track's own file first, then its album siblings in playing order.
fn cover_candidates(db: &DbState, track_id: i64) -> Vec<String> {
    let conn = db.lock();

    // Remote covers never come from a local file or its album siblings.
    let Ok(own) = conn.query_row(
        "SELECT path FROM tracks WHERE id = ?1 AND source = 'file'",
        [track_id],
        |r| r.get::<_, String>(0),
    ) else {
        return Vec::new();
    };

    // `IS` rather than `=` so two tracks with no album artist still match.
    let siblings = conn
        .prepare(
            "SELECT t.path FROM tracks t, tracks cur
             WHERE cur.id = ?1 AND cur.source = 'file'
               AND t.id != cur.id
               AND t.source = 'file'
               AND cur.album IS NOT NULL
               AND t.album = cur.album
               AND COALESCE(t.album_artist, t.artist) IS COALESCE(cur.album_artist, cur.artist)
             ORDER BY t.disc_number, t.track_number, t.id
             LIMIT ?2",
        )
        .and_then(|mut stmt| {
            stmt.query_map(
                rusqlite::params![track_id, COVER_SIBLING_LIMIT as i64],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let mut paths = Vec::with_capacity(siblings.len() + 1);
    paths.push(own);
    paths.extend(siblings);
    paths
}

/// A blank track for tests across the crate to fill in.
#[cfg(test)]
pub mod test_support {
    use super::{Track, TrackSource};

    pub fn track(id: i64, title: &str) -> Track {
        Track {
            id,
            source: TrackSource::File,
            artwork_address: None,
            folder_id: None,
            server_id: None,
            artwork_item: None,
            server_libraries: Vec::new(),
            path: format!("/music/{id}.flac"),
            title: title.to_owned(),
            artist: None,
            album: None,
            composer: None,
            duration_secs: 0.0,
            format: "FLAC".to_owned(),
            sample_rate: None,
            bit_depth: None,
            channels: None,
            lossless: true,
            added_at: 0,
            track_number: None,
            track_total: None,
            disc_number: None,
            disc_total: None,
            year: None,
            genre: None,
            album_artist: None,
            gain_db: 0.0,
        }
    }

    /// Builder steps over `track`, so a test names only the tags it cares
    /// about: `track(1, "A").album("X").artist("Y").number(3)`.
    impl Track {
        pub fn album(mut self, album: &str) -> Self {
            self.album = Some(album.to_owned());
            self
        }

        pub fn artist(mut self, artist: &str) -> Self {
            self.artist = Some(artist.to_owned());
            self
        }

        pub fn album_artist(mut self, album_artist: &str) -> Self {
            self.album_artist = Some(album_artist.to_owned());
            self
        }

        pub fn composer(mut self, composer: &str) -> Self {
            self.composer = Some(composer.to_owned());
            self
        }

        pub fn genre(mut self, genre: &str) -> Self {
            self.genre = Some(genre.to_owned());
            self
        }

        pub fn year(mut self, year: u32) -> Self {
            self.year = Some(year);
            self
        }

        pub fn number(mut self, number: u32) -> Self {
            self.track_number = Some(number);
            self
        }

        pub fn disc(mut self, disc: u32) -> Self {
            self.disc_number = Some(disc);
            self
        }

        pub fn added(mut self, added_at: i64) -> Self {
            self.added_at = added_at;
            self
        }

        pub fn duration(mut self, seconds: f64) -> Self {
            self.duration_secs = seconds;
            self
        }

        pub fn quality(mut self, format: &str, bit_depth: u8, sample_rate: u32) -> Self {
            self.format = format.to_owned();
            self.bit_depth = Some(bit_depth);
            self.sample_rate = Some(sample_rate);
            self
        }

        /// A song of the media server `server_id`, showing `item`'s picture.
        pub fn server(mut self, server_id: i64, item: &str) -> Self {
            self.source = TrackSource::Jellyfin;
            self.server_id = Some(server_id);
            self.artwork_item = Some(item.to_owned());
            self
        }

        /// Listed in these libraries of its server, by their row.
        pub fn in_libraries(mut self, libraries: &[i64]) -> Self {
            self.server_libraries = libraries.to_vec();
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal untagged WAV, so the pruner can be exercised without shipping
    /// a binary fixture. (Per-file metadata parsing is tested in the
    /// `audio-stack-rs` crate; here we only need real files on disk.)
    fn write_wav(path: &Path) {
        use std::io::Write;
        let (rate, channels, frames) = (44_100u32, 1u16, 100usize);
        let block_align = channels * 2;
        let data_len = frames as u32 * block_align as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&vec![0u8; data_len as usize]);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("temp directory");
        std::fs::File::create(path)
            .and_then(|mut file| file.write_all(&wav))
            .expect("write test wav");
    }

    #[test]
    fn importing_a_file_of_a_watched_folder_leaves_it_in_that_folder() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let folder = directory.path().join("watched");
        let file = folder.join("song.wav");
        write_wav(&file);

        let scanned = add_watched_folder(&db, &folder.to_string_lossy()).expect("scan");
        assert_eq!(scanned.added, 1);
        let imported = import_files(&db, vec![file.to_string_lossy().into_owned()]);
        assert_eq!(imported.updated, 1);

        let folders = list_watched_folders(&db).expect("folders");
        assert_eq!(folders[0].track_count, 1);
        let tracks = list_tracks(&db).expect("tracks");
        assert_eq!(tracks[0].folder_id, Some(folders[0].id));
    }

    fn server_song(item: &str, title: &str) -> ServerSong {
        ServerSong {
            address: format!("jellyfin://server/{item}"),
            metadata: Metadata {
                title: title.to_owned(),
                artist: Some("Artist".to_owned()),
                album: Some("Album".to_owned()),
                composer: None,
                duration_secs: 200.0,
                format: "FLAC".to_owned(),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                lossless: true,
                track_number: Some(1),
                track_total: None,
                disc_number: None,
                disc_total: None,
                year: None,
                genre: None,
                album_artist: None,
                rg_track_gain_db: None,
                rg_track_peak: None,
                rg_album_gain_db: None,
                rg_album_peak: None,
            },
            artwork_item: Some(format!("album-of-{item}")),
            libraries: Vec::new(),
            favourite: false,
        }
    }

    fn starred(mut song: ServerSong) -> ServerSong {
        song.favourite = true;
        song
    }

    #[test]
    fn a_sync_stars_the_songs_the_server_stars_and_takes_away_the_others() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        let identifier_of = |title: &str| {
            list_tracks(&database)
                .expect("tracks")
                .iter()
                .find(|track| track.title == title)
                .map(|track| track.id)
                .expect("a listed song")
        };

        sync_server_tracks(
            &database,
            server_id,
            &[],
            vec![starred(server_song("a", "One")), server_song("b", "Two")],
        )
        .expect("first sync");
        let (one, two) = (identifier_of("One"), identifier_of("Two"));
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![one])
        );

        // The listener starred Two in Janis; the server then took One's star
        // away and starred Two as well.
        crate::persistence::set_favourite_track(&database, two, true).expect("star Two");
        sync_server_tracks(
            &database,
            server_id,
            &[],
            vec![server_song("a", "One"), starred(server_song("b", "Two"))],
        )
        .expect("second sync");
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![two])
        );
    }

    #[test]
    fn a_playlist_sync_keeps_the_listeners_group_and_picture_and_drops_what_the_server_dropped() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        sync_server_tracks(
            &database,
            server_id,
            &[],
            vec![server_song("a", "One"), server_song("b", "Two")],
        )
        .expect("song sync");
        let identifier_of = |title: &str| {
            list_tracks(&database)
                .expect("tracks")
                .iter()
                .find(|track| track.title == title)
                .map(|track| track.id)
                .expect("a listed song")
        };
        let (one, two) = (identifier_of("One"), identifier_of("Two"));
        let listed = |item: &str, name: &str, songs: &[&str]| ListedPlaylist {
            item: item.to_owned(),
            name: name.to_owned(),
            picture: true,
            songs: songs
                .iter()
                .map(|song| format!("jellyfin://server/{song}"))
                .collect(),
        };

        sync_server_playlists(
            &database,
            server_id,
            &[
                // A duplicate keeps its first place; a video, unknown here, is
                // left out.
                listed("road", "Road", &["b", "video", "a", "b"]),
                listed("gone", "Gone soon", &["a"]),
            ],
        )
        .expect("first playlist sync");
        let saved = crate::persistence::playlists(&database).expect("read");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].track_ids, [two, one]);
        assert!(saved[0].server_picture);
        assert_eq!(saved[0].server, Some((server_id, "road".to_owned())));

        let group = crate::persistence::create_playlist_group(&database, "Trips").expect("group");
        crate::persistence::set_playlist_group(&database, saved[0].id, Some(group)).expect("file");
        crate::persistence::set_playlist_picture(&database, saved[0].id, Some(&[7]))
            .expect("picture");

        sync_server_playlists(&database, server_id, &[listed("road", "Road trip", &["a"])])
            .expect("second playlist sync");
        let saved_again = crate::persistence::playlists(&database).expect("read");
        assert_eq!(
            saved_again.len(),
            1,
            "the playlist gone from the server is gone"
        );
        assert_eq!(
            saved_again[0].id, saved[0].id,
            "a re-listed playlist keeps its row"
        );
        assert_eq!(saved_again[0].name, "Road trip");
        assert_eq!(saved_again[0].track_ids, [one]);
        assert_eq!(saved_again[0].group_id, Some(group));
        assert!(saved_again[0].picture);

        // A playlist of Janis's own is not the server's to remove.
        crate::persistence::create_playlist(&database, "Mine", None, &[]).expect("local");
        sync_server_playlists(&database, server_id, &[]).expect("third playlist sync");
        let names: Vec<String> = crate::persistence::playlists(&database)
            .expect("read")
            .into_iter()
            .map(|playlist| playlist.name)
            .collect();
        assert_eq!(names, ["Mine"]);
    }

    #[test]
    fn a_playlist_longer_than_a_burst_is_synced_whole_and_in_order() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        let count = PLAYLIST_BURST * 2 + 3;
        let songs: Vec<ServerSong> = (0..count)
            .map(|index| server_song(&format!("{index:04}"), &format!("{index:04}")))
            .collect();
        sync_server_tracks(&database, server_id, &[], songs).expect("song sync");
        let identifiers: std::collections::HashMap<String, i64> = list_tracks(&database)
            .expect("tracks")
            .into_iter()
            .map(|track| (track.title, track.id))
            .collect();
        // Backwards, and the first song again at the end, past a burst.
        let order: Vec<String> = (0..count)
            .rev()
            .map(|index| format!("{index:04}"))
            .collect();
        let mut addresses: Vec<String> = order
            .iter()
            .map(|item| format!("jellyfin://server/{item}"))
            .collect();
        addresses.push(addresses[0].clone());
        let playlist = |item: &str| ListedPlaylist {
            item: item.to_owned(),
            name: item.to_owned(),
            picture: false,
            songs: addresses.clone(),
        };

        sync_server_playlists(&database, server_id, &[playlist("long"), playlist("again")])
            .expect("playlist sync");

        let expected: Vec<i64> = order.iter().map(|title| identifiers[title]).collect();
        let saved = crate::persistence::playlists(&database).expect("read");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].track_ids, expected);
        assert_eq!(saved[1].track_ids, expected);
    }

    fn signed_in(database: &DbState) -> i64 {
        signed_in_from(database, "http://server")
    }

    /// Signs in to the same server as the same user, reached at `address`.
    fn signed_in_from(database: &DbState, address: &str) -> i64 {
        crate::persistence::save_media_server(
            database,
            &crate::persistence::NewMediaServer {
                address: address.to_owned(),
                server_identifier: "server".to_owned(),
                server_name: "Home".to_owned(),
                user_identifier: "user".to_owned(),
                user_name: "listener".to_owned(),
                access_token: "token".to_owned(),
            },
        )
        .expect("sign in")
        .id
    }

    #[test]
    fn a_sync_adds_updates_and_removes_what_the_server_lists() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&db);

        let first = sync_server_tracks(
            &db,
            server_id,
            &[],
            vec![server_song("a", "One"), server_song("b", "Two")],
        )
        .expect("first sync");
        assert_eq!((first.added, first.updated, first.removed), (2, 0, 0));
        let a_id = list_tracks(&db)
            .expect("tracks")
            .iter()
            .find(|track| track.title == "One")
            .expect("a")
            .id;
        store_loudness(&db, a_id, -14.0, 0.9);

        let second = sync_server_tracks(&db, server_id, &[], vec![server_song("a", "One again")])
            .expect("second sync");
        assert_eq!((second.added, second.updated, second.removed), (0, 1, 1));

        let tracks = list_tracks(&db).expect("tracks");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "One again");
        assert_eq!(tracks[0].server_id, Some(server_id));
        assert_eq!(
            tracks[0].artwork_item.as_deref(),
            Some("album-of-a"),
            "the listing carries the song's picture"
        );
        assert_eq!(tracks[0].id, a_id, "a re-listed song keeps its row");
        assert!(!needs_loudness(&db, a_id), "and its loudness measurement");
        assert_eq!(
            cover_source(&db, a_id),
            CoverSource::Server {
                artwork_item: Some("album-of-a".to_owned())
            }
        );
    }

    /// Every server item, with the number of rows that point at it.
    fn rows_per_item(database: &DbState) -> Vec<(String, i64)> {
        database
            .lock()
            .prepare(
                "SELECT path, count(*) FROM tracks WHERE server_id IS NOT NULL
                 GROUP BY path ORDER BY path",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .expect("the rows per item")
    }

    #[test]
    fn a_server_item_is_one_row_however_often_and_from_wherever_it_is_listed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        let libraries = [library("music", "Music"), library("kids", "Kids")];
        // The listing files an item two libraries share in both; should it
        // still name the item twice, the second is the same item again.
        let listed = || {
            vec![
                listed_in(server_song("a", "One"), &["music", "kids"]),
                listed_in(server_song("b", "Two"), &["music"]),
                listed_in(server_song("a", "One"), &["music", "kids"]),
            ]
        };
        let rows = |database: &DbState| -> Vec<(i64, String, Vec<i64>)> {
            let mut rows: Vec<_> = list_tracks(database)
                .expect("tracks")
                .into_iter()
                .map(|mut track| {
                    track.server_libraries.sort_unstable();
                    (track.id, track.path, track.server_libraries)
                })
                .collect();
            rows.sort();
            rows
        };

        sync_server_tracks(&database, server_id, &libraries, listed()).expect("first sync");
        let first = rows(&database);
        assert_eq!(
            rows_per_item(&database),
            [
                ("jellyfin://server/a".to_owned(), 1),
                ("jellyfin://server/b".to_owned(), 1),
            ]
        );
        let mut library_rows: Vec<i64> = crate::persistence::server_libraries(&database)
            .expect("libraries")
            .iter()
            .map(|library| library.id)
            .collect();
        library_rows.sort_unstable();
        assert_eq!(
            first[0].2, library_rows,
            "an item listed in two places is filed in both, once each"
        );

        // Synced again, then again after signing in from another address —
        // the same server and user reached another way.
        sync_server_tracks(&database, server_id, &libraries, listed()).expect("second sync");
        assert_eq!(
            signed_in_from(&database, "http://100.64.0.1:8096"),
            server_id,
            "the same server and user keep their row"
        );
        sync_server_tracks(&database, server_id, &libraries, listed()).expect("third sync");

        assert_eq!(rows(&database), first, "no row is added, every row is kept");
    }

    fn library(identifier: &str, name: &str) -> ListedLibrary {
        ListedLibrary {
            identifier: identifier.to_owned(),
            name: name.to_owned(),
        }
    }

    fn listed_in(mut song: ServerSong, libraries: &[&str]) -> ServerSong {
        song.libraries = libraries
            .iter()
            .map(|library| (*library).to_owned())
            .collect();
        song
    }

    #[test]
    fn a_sync_files_each_song_in_its_libraries_and_keeps_which_are_shown() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);

        sync_server_tracks(
            &database,
            server_id,
            &[library("music", "Music"), library("kids", "Kids")],
            vec![
                listed_in(server_song("a", "One"), &["music"]),
                listed_in(server_song("b", "Two"), &["music", "kids"]),
            ],
        )
        .expect("first sync");
        let libraries = crate::persistence::server_libraries(&database).expect("libraries");
        let names: Vec<&str> = libraries
            .iter()
            .map(|library| library.name.as_str())
            .collect();
        assert_eq!(names, ["Music", "Kids"], "in the server's order");
        assert!(
            libraries.iter().all(|library| library.shown),
            "shown at first"
        );
        let (music, kids) = (libraries[0].id, libraries[1].id);
        let libraries_of = |title: &str| {
            list_tracks(&database)
                .expect("tracks")
                .into_iter()
                .find(|track| track.title == title)
                .map(|track| track.server_libraries)
        };
        assert_eq!(libraries_of("One"), Some(vec![music]));
        assert_eq!(
            libraries_of("Two"),
            Some(vec![music, kids]),
            "a song two libraries share is in both"
        );

        crate::persistence::set_server_library_shown(&database, kids, false).expect("hide");
        sync_server_tracks(
            &database,
            server_id,
            &[library("kids", "Children"), library("talk", "Talk")],
            vec![
                listed_in(server_song("b", "Two"), &["kids"]),
                listed_in(server_song("c", "Three"), &["talk"]),
            ],
        )
        .expect("second sync");
        let libraries = crate::persistence::server_libraries(&database).expect("libraries");
        assert_eq!(
            libraries,
            [
                crate::persistence::ServerLibrary {
                    id: kids,
                    name: "Children".to_owned(),
                    shown: false,
                },
                crate::persistence::ServerLibrary {
                    id: libraries[1].id,
                    name: "Talk".to_owned(),
                    shown: true,
                },
            ],
            "a renamed library keeps its row and its choice; one no longer listed goes"
        );
        assert_eq!(libraries_of("Two"), Some(vec![kids]));
        assert_eq!(libraries_of("One"), None, "gone from the server");
    }

    #[test]
    fn a_song_that_cannot_be_filed_keeps_what_it_had_and_is_counted() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        let libraries = [library("music", "Music"), library("kids", "Kids")];
        sync_server_tracks(
            &database,
            server_id,
            &libraries,
            vec![listed_in(server_song("a", "One"), &["kids"])],
        )
        .expect("first sync");
        // Filing this song anywhere now fails half-way: after its old
        // libraries are cleared, at the first new one.
        database
            .lock()
            .execute_batch(
                "CREATE TEMP TRIGGER refuse BEFORE INSERT ON track_libraries
                 BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .expect("a trigger");

        let report = sync_server_tracks(
            &database,
            server_id,
            &libraries,
            vec![listed_in(server_song("a", "One renamed"), &["music"])],
        )
        .expect("second sync");

        assert_eq!(report.skipped, 1);
        let tracks = list_tracks(&database).expect("tracks");
        let kids = crate::persistence::server_libraries(&database).expect("libraries")[1].id;
        assert_eq!(
            (
                tracks[0].title.as_str(),
                tracks[0].server_libraries.as_slice()
            ),
            ("One", [kids].as_slice()),
            "neither its new tags nor an empty filing were kept"
        );
    }

    #[test]
    fn signing_out_takes_the_libraries_away_with_the_songs() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        sync_server_tracks(
            &database,
            server_id,
            &[library("music", "Music")],
            vec![listed_in(server_song("a", "One"), &["music"])],
        )
        .expect("sync");

        crate::persistence::remove_media_server(&database, server_id).expect("sign out");

        assert_eq!(
            crate::persistence::server_libraries(&database),
            Ok(Vec::new())
        );
        assert_eq!(list_tracks(&database).map(|tracks| tracks.len()), Ok(0));
    }

    #[test]
    fn a_rescan_never_touches_a_servers_songs() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&db);
        sync_server_tracks(&db, server_id, &[], vec![server_song("a", "One")]).expect("sync");

        let report = rescan_library(&db).expect("rescan");

        assert_eq!(report.removed, 0);
        assert_eq!(list_tracks(&db).expect("tracks").len(), 1);
    }

    #[test]
    fn a_files_star_lives_in_janis_alone_through_rescans_and_syncs_and_leaves_with_the_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let folder = directory.path().join("watched");
        let file = folder.join("song.wav");
        write_wav(&file);
        add_watched_folder(&database, &folder.to_string_lossy()).expect("scan");
        let file_id = list_tracks(&database).expect("tracks")[0].id;
        crate::persistence::set_favourite_track(&database, file_id, true).expect("star the file");

        // A rescan upserts the file in place: its row, and so its star, stay.
        rescan_library(&database).expect("rescan");
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![file_id])
        );

        // A sync stars and unstars the server's songs only.
        let server_id = signed_in(&database);
        sync_server_tracks(&database, server_id, &[], vec![server_song("a", "One")]).expect("sync");
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![file_id])
        );

        // Gone from its folder, the file is pruned, and its star with it.
        std::fs::remove_file(&file).expect("delete the file");
        rescan_library(&database).expect("rescan");
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(Vec::new())
        );
    }

    #[test]
    fn prune_spares_tracks_whose_root_is_unreachable() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = crate::persistence::init(directory.path().to_path_buf()).expect("db");

        let live_root = directory.path().join("live");
        let kept = live_root.join("kept.wav");
        write_wav(&kept);
        let ejected_root = directory.path().join("ejected");

        {
            let conn = db.lock();
            let folder = |id: i64, path: &Path| {
                conn.execute(
                    "INSERT INTO watched_folders (id, path) VALUES (?1, ?2)",
                    rusqlite::params![id, path.to_string_lossy()],
                )
                .expect("insert folder");
            };
            folder(1, &live_root);
            folder(2, &ejected_root);
            let track = |folder_id: i64, path: &Path| {
                conn.execute(
                    "INSERT INTO tracks (folder_id, path, title, format)
                     VALUES (?1, ?2, 'T', 'WAV')",
                    rusqlite::params![folder_id, path.to_string_lossy()],
                )
                .expect("insert track");
            };
            track(1, &kept);
            track(1, &live_root.join("gone.wav"));
            track(2, &ejected_root.join("on-the-ejected-drive.wav"));
        }

        let removed = prune_missing(&db).expect("prune");

        assert_eq!(
            removed, 1,
            "only the file that vanished from a reachable root is pruned"
        );
        let survivors: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(
            survivors, 2,
            "the unreachable volume's track must survive: an ejected drive \
             must not empty the library"
        );
    }

    #[test]
    fn an_album_gain_wins_over_the_track_gain() {
        let gains = Gains {
            album: Some(-5.0),
            track: Some(-8.0),
            ..Gains::default()
        };
        assert_eq!(resolved_gain(gains, Some(-30.0), None), -5.0);
    }

    #[test]
    fn a_track_gain_stands_in_when_the_album_has_none() {
        let gains = Gains {
            track: Some(-8.0),
            ..Gains::default()
        };
        assert_eq!(resolved_gain(gains, Some(-30.0), None), -8.0);
    }

    #[test]
    fn a_measurement_stands_in_when_no_gain_is_known() {
        assert_eq!(resolved_gain(Gains::default(), Some(-23.0), None), 5.0);
        assert_eq!(resolved_gain(Gains::default(), None, None), 0.0);
    }

    #[test]
    fn a_boost_is_held_below_the_peak_of_the_gain_it_uses() {
        // A peak of 0.5 is -6 dBFS: with the -1 dB ceiling, at most +5 dB.
        let gains = Gains {
            album: Some(9.0),
            album_peak: Some(0.5),
            ..Gains::default()
        };
        assert!((resolved_gain(gains, None, None) - 5.021).abs() < 0.01);
        let borrowed = Gains {
            album: Some(9.0),
            track_peak: Some(0.5),
            ..Gains::default()
        };
        assert!(
            (resolved_gain(borrowed, None, None) - 5.021).abs() < 0.01,
            "an album gain without a peak borrows the track's"
        );
    }

    #[test]
    fn only_local_files_without_any_gain_wait_for_a_measurement() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let folder = directory.path().join("watched");
        write_wav(&folder.join("first.wav"));
        write_wav(&folder.join("second.wav"));
        add_watched_folder(&database, &folder.to_string_lossy()).expect("scan");
        let server_id = signed_in(&database);
        sync_server_tracks(&database, server_id, &[], vec![server_song("a", "One")]).expect("sync");

        let pending = tracks_needing_loudness(&database).expect("pending");
        assert_eq!(pending.len(), 2, "the server's song is not a file here");
        assert!(pending.iter().all(|track| track.path.ends_with(".wav")));

        store_loudness(&database, pending[0].track_id, -20.0, 0.5);
        let still = tracks_needing_loudness(&database).expect("pending");
        assert_eq!(still, vec![pending[1].clone()]);
        assert!(!needs_loudness(&database, pending[0].track_id));
    }

    #[test]
    fn a_servers_gains_are_kept_and_played() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&database);
        let mut song = server_song("a", "One");
        song.metadata.rg_track_gain_db = Some(-7.0);
        song.metadata.rg_album_gain_db = Some(-4.0);
        sync_server_tracks(&database, server_id, &[], vec![song]).expect("sync");

        let tracks = list_tracks(&database).expect("tracks");
        assert_eq!(tracks[0].gain_db, -4.0, "the album's gain");
        assert!(
            !needs_loudness(&database, tracks[0].id),
            "a song with a gain is not measured as it plays"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_file_and_a_linked_folder_are_scanned() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let elsewhere = directory.path().join("elsewhere");
        write_wav(&elsewhere.join("single.wav"));
        write_wav(&elsewhere.join("album").join("track.wav"));
        let watched = directory.path().join("watched");
        std::fs::create_dir_all(&watched).expect("the watched folder");
        symlink(elsewhere.join("single.wav"), watched.join("single.wav")).expect("link a file");
        symlink(elsewhere.join("album"), watched.join("album")).expect("link a folder");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");

        let report = add_watched_folder(&database, watched.to_str().expect("a Unicode path"))
            .expect("the folder is watched");

        assert_eq!((report.added, report.skipped), (2, 0));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_back_up_the_folder_ends_the_walk_and_is_counted() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let watched = directory.path().join("watched");
        write_wav(&watched.join("song.wav"));
        std::os::unix::fs::symlink(&watched, watched.join("loop")).expect("link the folder");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");

        let report = add_watched_folder(&database, watched.to_str().expect("a Unicode path"))
            .expect("the folder is watched");

        assert_eq!((report.added, report.skipped), (1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_is_counted_as_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let watched = directory.path().join("watched");
        write_wav(&watched.join("song.wav"));
        let locked = watched.join("locked");
        write_wav(&locked.join("hidden.wav"));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
            .expect("lock the folder");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");

        let report = add_watched_folder(&database, watched.to_str().expect("a Unicode path"));

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("unlock the folder for clean-up");
        let report = report.expect("the folder is watched");
        assert_eq!((report.added, report.skipped), (1, 1));
    }

    fn watch(database: &DbState, folder: &Path) -> Result<ScanReport, ScanError> {
        add_watched_folder(database, folder.to_str().expect("a Unicode path"))
    }

    #[test]
    fn a_folder_inside_a_watched_one_is_refused() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let music = directory.path().join("music");
        write_wav(&music.join("album").join("song.wav"));
        write_wav(&directory.path().join("music2").join("song.wav"));
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        watch(&database, &music).expect("the parent is watched");

        assert_eq!(
            watch(&database, &music.join("album")),
            Err(ScanError::InsideWatched(
                music.to_str().expect("a Unicode path").to_owned()
            ))
        );
        assert!(
            watch(&database, &directory.path().join("music2")).is_ok(),
            "a folder whose name only starts like a watched one is not inside it"
        );
        assert!(
            watch(&database, &music).is_ok(),
            "watching a folder again rescans it"
        );
    }

    #[test]
    fn watching_a_parent_takes_over_its_watched_subfolders_and_removing_it_removes_all() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let music = directory.path().join("music");
        write_wav(&music.join("album").join("song.wav"));
        write_wav(&music.join("loose.wav"));
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        watch(&database, &music.join("album")).expect("the subfolder is watched");

        let report = watch(&database, &music).expect("the parent is watched");

        assert_eq!((report.added, report.updated), (1, 1));
        let folders = list_watched_folders(&database).expect("the folders");
        assert_eq!(
            folders.len(),
            1,
            "the subfolder's row is merged into the parent"
        );
        assert_eq!(folders[0].path, music.to_str().expect("a Unicode path"));
        assert_eq!(folders[0].track_count, 2);

        remove_watched_folder(&database, folders[0].id).expect("the parent is removed");
        assert_eq!(list_tracks(&database).map(|tracks| tracks.len()), Ok(0));
    }

    #[test]
    fn a_gain_measured_this_session_wins_over_the_one_the_library_read() {
        let mut gains = SessionGains::default();
        let tagged = Track {
            gain_db: -3.0,
            ..test_support::track(1, "a")
        };
        assert_eq!(gains.gain_of(&tagged), -3.0);

        let gain = gains.measured(
            PendingLoudness {
                track_id: 1,
                path: tagged.path.clone(),
            },
            -12.0,
            0.5,
        );

        assert_eq!(
            gain,
            resolved_gain(Gains::default(), Some(-12.0), Some(0.5))
        );
        assert_eq!(gains.gain_of(&tagged), gain);
        assert_eq!(
            gains.gain_of(&test_support::track(2, "b")),
            0.0,
            "another track keeps its own"
        );
        assert_eq!(
            gains.gain_of(&Track {
                path: "/music/another-file.flac".to_owned(),
                ..tagged
            }),
            -3.0,
            "another file given the same row keeps the gain the library read"
        );
    }

    #[test]
    fn a_measurement_is_not_stored_on_a_row_now_holding_another_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let first = directory.path().join("first.wav");
        write_wav(&first);
        import_files(&database, vec![first.to_string_lossy().into_owned()]);
        let measuring = tracks_needing_loudness(&database).expect("pending")[0].clone();

        // While it is measured, the file goes and another takes its row.
        database
            .lock()
            .execute("DELETE FROM tracks WHERE id = ?1", [measuring.track_id])
            .expect("remove the song");
        let second = directory.path().join("second.wav");
        write_wav(&second);
        import_files(&database, vec![second.to_string_lossy().into_owned()]);
        let replacement = tracks_needing_loudness(&database).expect("pending")[0].clone();
        assert_eq!(
            replacement.track_id, measuring.track_id,
            "SQLite reuses the id"
        );

        assert!(!store_measured_loudness(&database, &measuring, -20.0, 0.5));
        assert!(needs_loudness(&database, replacement.track_id));
        assert!(store_measured_loudness(&database, &replacement, -20.0, 0.5));
        assert!(!needs_loudness(&database, replacement.track_id));
    }

    /// A row of `tracks`, as a scan or a sync would leave it.
    struct Row<'row> {
        id: i64,
        path: &'row str,
        album: Option<&'row str>,
        artist: Option<&'row str>,
        album_artist: Option<&'row str>,
        disc: Option<u32>,
        number: Option<u32>,
        server_id: Option<i64>,
    }

    impl Default for Row<'_> {
        fn default() -> Self {
            Row {
                id: 0,
                path: "",
                album: Some("Blue"),
                artist: Some("Joni"),
                album_artist: None,
                disc: None,
                number: None,
                server_id: None,
            }
        }
    }

    fn insert(database: &DbState, row: Row) {
        database
            .lock()
            .execute(
                "INSERT INTO tracks (id, path, title, format, album, artist, album_artist,
                                     disc_number, track_number, server_id, source)
                 VALUES (?1, ?2, 'T', 'FLAC', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    row.id,
                    row.path,
                    row.album,
                    row.artist,
                    row.album_artist,
                    row.disc,
                    row.number,
                    row.server_id,
                    if row.server_id.is_some() {
                        "jellyfin"
                    } else {
                        "file"
                    }
                ],
            )
            .expect("insert a track");
    }

    fn fresh_database() -> (tempfile::TempDir, DbState) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        (directory, database)
    }

    #[test]
    fn a_track_offers_its_own_file_then_its_album_siblings_in_disc_and_track_order() {
        let (_directory, database) = fresh_database();
        let server = crate::persistence::save_media_server(
            &database,
            &crate::persistence::NewMediaServer {
                address: "http://jellyfin.local:8096".to_owned(),
                server_identifier: "server".to_owned(),
                server_name: "Home".to_owned(),
                user_identifier: "user".to_owned(),
                user_name: "listener".to_owned(),
                access_token: "token".to_owned(),
            },
        )
        .expect("a server");
        insert(
            &database,
            Row {
                id: 1,
                path: "/own.flac",
                disc: Some(1),
                number: Some(3),
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 2,
                path: "/second-disc.flac",
                disc: Some(2),
                number: Some(1),
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 3,
                path: "/first.flac",
                disc: Some(1),
                number: Some(1),
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 4,
                path: "/other-album.flac",
                album: Some("Hejira"),
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 5,
                path: "/other-artist.flac",
                artist: Some("Someone"),
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 6,
                path: "jellyfin://server/item",
                server_id: Some(server.id),
                ..Row::default()
            },
        );

        assert_eq!(
            cover_candidates(&database, 1),
            ["/own.flac", "/first.flac", "/second-disc.flac"]
        );
        assert!(
            cover_candidates(&database, 6).is_empty(),
            "a server song offers no file"
        );
        assert_eq!(
            cover_source(&database, 6),
            CoverSource::Server { artwork_item: None }
        );
        assert_eq!(cover_source(&database, 1), CoverSource::File);
        assert_eq!(cover_source(&database, 99), CoverSource::File);
    }

    #[test]
    fn siblings_offered_for_a_cover_stop_at_the_limit() {
        let (_directory, database) = fresh_database();
        let paths: Vec<String> = (1..=20).map(|id| format!("/{id}.flac")).collect();
        for (index, path) in paths.iter().enumerate() {
            insert(
                &database,
                Row {
                    id: index as i64 + 1,
                    path,
                    ..Row::default()
                },
            );
        }
        assert_eq!(
            cover_candidates(&database, 1).len(),
            COVER_SIBLING_LIMIT + 1
        );
    }

    #[test]
    fn a_track_on_no_album_offers_only_its_own_file_and_untagged_artists_still_match() {
        let (_directory, database) = fresh_database();
        insert(
            &database,
            Row {
                id: 1,
                path: "/loose.flac",
                album: None,
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 2,
                path: "/loose-too.flac",
                album: None,
                ..Row::default()
            },
        );
        assert_eq!(cover_candidates(&database, 1), ["/loose.flac"]);

        insert(
            &database,
            Row {
                id: 3,
                path: "/a.flac",
                album: Some("Untitled"),
                artist: None,
                ..Row::default()
            },
        );
        insert(
            &database,
            Row {
                id: 4,
                path: "/b.flac",
                album: Some("Untitled"),
                artist: None,
                ..Row::default()
            },
        );
        assert_eq!(cover_candidates(&database, 3), ["/a.flac", "/b.flac"]);
    }

    #[test]
    fn an_imported_file_is_pruned_only_once_its_folder_is_back_without_it() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let present = directory.path().join("present");
        write_wav(&present.join("kept.wav"));
        write_wav(&present.join("deleted.wav"));
        let elsewhere = directory.path().join("elsewhere");
        write_wav(&elsewhere.join("on-a-drive.wav"));
        let path = |file: &Path| file.to_str().expect("a Unicode path").to_owned();
        let report = import_files(
            &database,
            vec![
                path(&present.join("kept.wav")),
                path(&present.join("deleted.wav")),
                path(&elsewhere.join("on-a-drive.wav")),
            ],
        );
        assert_eq!(report.added, 3);
        std::fs::remove_file(present.join("deleted.wav")).expect("delete one file");
        std::fs::remove_dir_all(&elsewhere).expect("eject the drive");

        assert_eq!(prune_missing(&database), Ok(1));
        assert_eq!(list_tracks(&database).map(|tracks| tracks.len()), Ok(2));
    }

    #[cfg(unix)]
    #[test]
    fn a_rescan_keeps_the_songs_of_a_folder_it_cannot_read() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let music = directory.path().join("music");
        let album = music.join("album");
        write_wav(&album.join("song.wav"));
        watch(&database, &music).expect("watched");
        let song = list_tracks(&database).expect("tracks")[0].id;
        crate::persistence::set_favourite_track(&database, song, true).expect("star the song");

        std::fs::set_permissions(&album, std::fs::Permissions::from_mode(0o000))
            .expect("lock the album folder");
        let report = rescan_library(&database);
        std::fs::set_permissions(&album, std::fs::Permissions::from_mode(0o755))
            .expect("unlock the album folder");

        assert_eq!(report.map(|report| report.removed), Ok(0));
        assert_eq!(list_tracks(&database).map(|tracks| tracks.len()), Ok(1));
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![song])
        );
    }

    #[test]
    fn a_snapshot_read_before_a_star_is_overtaken_by_it() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let music = directory.path().join("music");
        write_wav(&music.join("song.wav"));
        watch(&database, &music).expect("watched");
        let before = LibrarySnapshot::read(&database).expect("a snapshot");
        let song = before.tracks[0].id;
        assert!(before.is_current(&database));

        // Neither a measurement nor a preference changes what it read.
        store_loudness(&database, song, -14.0, 0.9);
        crate::persistence::set_volume(&database, 0.5).expect("a volume");
        assert!(before.is_current(&database));

        crate::persistence::set_favourite_track(&database, song, true).expect("star the song");
        assert!(!before.is_current(&database), "the star is not in it");

        let after = LibrarySnapshot::read(&database).expect("a snapshot");
        assert!(after.is_current(&database));
        assert_eq!(after.favourite_track_ids, vec![song]);
    }

    #[test]
    fn a_rescan_picks_up_new_files_and_prunes_deleted_ones() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let music = directory.path().join("music");
        write_wav(&music.join("first.wav"));
        write_wav(&music.join("second.wav"));
        watch(&database, &music).expect("watched");

        std::fs::remove_file(music.join("first.wav")).expect("delete a file");
        write_wav(&music.join("third.wav"));
        let report = rescan_library(&database).expect("rescanned");

        assert_eq!((report.added, report.updated, report.removed), (1, 1, 1));
        let mut titles: Vec<String> = list_tracks(&database)
            .expect("tracks")
            .into_iter()
            .map(|track| track.path)
            .collect();
        titles.sort();
        assert_eq!(titles.len(), 2);
        assert!(titles[0].ends_with("second.wav") && titles[1].ends_with("third.wav"));
    }

    #[test]
    fn a_folder_larger_than_one_burst_is_written_whole() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let music = directory.path().join("music");
        for index in 0..=SCAN_BATCH {
            write_wav(&music.join(format!("{index:03}.wav")));
        }
        let report = watch(&database, &music).expect("watched");
        assert_eq!(report.added, SCAN_BATCH + 1);
    }

    #[test]
    fn what_cannot_be_watched_or_imported_is_refused_or_counted() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");
        let file = directory.path().join("song.wav");
        write_wav(&file);
        assert!(matches!(watch(&database, &file), Err(ScanError::Failed(_))));

        let text = directory.path().join("notes.wav");
        std::fs::write(&text, "not audio").expect("write a fake");
        let report = import_files(
            &database,
            vec![
                directory
                    .path()
                    .join("missing.wav")
                    .to_string_lossy()
                    .into_owned(),
                text.to_string_lossy().into_owned(),
            ],
        );
        assert_eq!((report.added, report.skipped), (0, 2));
        assert!(
            !needs_loudness(&database, 99),
            "an unknown track needs nothing"
        );
    }

    /// The checked-in album: two tiny FLAC files with the same album tags,
    /// the first embedding a 1×1 picture, the second none.
    fn fixture_album() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/album")
    }

    #[test]
    fn a_scanned_file_brings_its_tags_into_its_row() {
        let (_directory, database) = fresh_database();
        let report = watch(&database, &fixture_album()).expect("the fixture album is watched");
        assert_eq!((report.added, report.skipped), (2, 0));

        let tracks = list_tracks(&database).expect("tracks");
        let with_cover = tracks
            .iter()
            .find(|track| track.title == "With Cover")
            .expect("the first file's title");
        assert_eq!(with_cover.artist.as_deref(), Some("Fixture Artist"));
        assert_eq!(with_cover.album.as_deref(), Some("Fixture Album"));
        assert_eq!(with_cover.album_artist.as_deref(), Some("Fixture Artist"));
        assert_eq!(with_cover.track_number, Some(1));
        assert_eq!(with_cover.year, Some(1999));
        assert_eq!(with_cover.genre.as_deref(), Some("Jazz"));
        assert_eq!(with_cover.format, "FLAC");
        assert!(with_cover.lossless);
    }

    #[test]
    fn a_file_without_art_shows_the_picture_its_album_sibling_embeds() {
        let (_directory, database) = fresh_database();
        watch(&database, &fixture_album()).expect("the fixture album is watched");
        let tracks = list_tracks(&database).expect("tracks");
        let id_of = |title: &str| {
            tracks
                .iter()
                .find(|track| track.title == title)
                .map(|track| track.id)
                .expect("a fixture track")
        };

        let own = track_cover(&database, id_of("With Cover")).expect("its own picture");
        let borrowed = track_cover(&database, id_of("Without Cover")).expect("its sibling's");
        assert_eq!(own.data_base64, borrowed.data_base64);
        assert!(!own.data_base64.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_whose_path_is_not_unicode_is_skipped_and_counted() {
        use std::os::unix::ffi::OsStrExt;
        let directory = tempfile::tempdir().expect("a temporary directory");
        let music = directory.path().join("music");
        write_wav(&music.join("named.wav"));
        let odd = music.join(std::ffi::OsStr::from_bytes(b"caf\xe9.wav"));
        std::fs::copy(music.join("named.wav"), &odd).expect("a file with a Latin-1 name");
        let database = crate::persistence::init(directory.path().join("data")).expect("a database");

        let report = watch(&database, &music).expect("watched");

        assert_eq!((report.added, report.skipped), (1, 1));
    }

    fn youtube_music_song(identifier: &str, title: &str) -> YouTubeMusicTrack {
        let mut metadata = server_song("unused", title).metadata;
        metadata.format = "AAC".to_owned();
        metadata.sample_rate = None;
        metadata.bit_depth = None;
        metadata.channels = None;
        metadata.lossless = false;
        YouTubeMusicTrack {
            identifier: identifier.to_owned(),
            metadata,
            artwork_address: Some(format!("https://images.example/{identifier}.jpg")),
        }
    }

    fn youtube_music_playlist(
        identifier: &str,
        name: &str,
        track_identifiers: &[&str],
    ) -> YouTubeMusicPlaylist {
        YouTubeMusicPlaylist {
            identifier: identifier.to_owned(),
            name: name.to_owned(),
            track_identifiers: track_identifiers
                .iter()
                .map(|identifier| (*identifier).to_owned())
                .collect(),
        }
    }

    #[test]
    fn youtube_music_playlists_keep_identity_order_and_local_customization_without_touching_other_sources(
    ) {
        use crate::persistence::{self, PlaylistSource};

        let (_directory, database) = fresh_database();
        let server_id = signed_in(&database);
        sync_server_tracks(&database, server_id, &[], vec![server_song("a", "Server")])
            .expect("server song");
        sync_server_playlists(
            &database,
            server_id,
            &[ListedPlaylist {
                item: "server-list".to_owned(),
                name: "Server playlist".to_owned(),
                picture: true,
                songs: vec!["jellyfin://server/a".to_owned()],
            }],
        )
        .expect("server playlist");
        database
            .lock_for_library_change()
            .execute(
                "INSERT INTO tracks (path, title, format) VALUES ('/file.flac', 'File', 'FLAC')",
                [],
            )
            .expect("local file");
        let file_id = list_tracks(&database)
            .expect("tracks")
            .iter()
            .find(|track| track.source == TrackSource::File)
            .expect("file")
            .id;
        let local_id = persistence::create_playlist(&database, "Files", None, &[file_id])
            .expect("create local")
            .expect("local saved");
        let group_id = persistence::create_playlist_group(&database, "Moods").expect("group");
        persistence::set_playlist_group(&database, local_id, Some(group_id)).expect("local group");
        persistence::set_playlist_picture(&database, local_id, Some(&[4, 5]))
            .expect("local picture");
        let other_playlists = persistence::playlists(&database).expect("other playlists");
        let listing = YouTubeMusicLibrary {
            tracks: vec![
                youtube_music_song("abcdefghijk", "Saved song"),
                youtube_music_song("lmnopqrstuv", "Playlist-only song"),
            ],
            playlists: vec![
                youtube_music_playlist(
                    "retained-list",
                    "Imported",
                    &["lmnopqrstuv", "abcdefghijk", "lmnopqrstuv"],
                ),
                youtube_music_playlist("dropped-list", "Dropped", &["abcdefghijk"]),
            ],
        };
        let before = LibrarySnapshot::read(&database).expect("snapshot before import");
        sync_youtube_music_library(&database, &listing, Path::new("cookies.txt")).expect("import");
        assert!(!before.is_current(&database));
        let snapshot = LibrarySnapshot::read(&database).expect("snapshot includes playlists");
        assert!(snapshot.is_current(&database));
        let song_id = |identifier: &str| {
            snapshot
                .tracks
                .iter()
                .find(|track| track.path == format!("youtube-music://{identifier}"))
                .expect("imported song")
                .id
        };
        let saved_id = song_id("abcdefghijk");
        let playlist_only_id = song_id("lmnopqrstuv");
        let imported = snapshot
            .playlists
            .iter()
            .find(|playlist| playlist.name == "Imported")
            .expect("imported playlist")
            .clone();
        let dropped_id = snapshot
            .playlists
            .iter()
            .find(|playlist| playlist.name == "Dropped")
            .expect("second playlist")
            .id;
        assert_eq!(imported.source, PlaylistSource::YouTubeMusic);
        assert_eq!(imported.server, None);
        assert_eq!(imported.track_ids, [playlist_only_id, saved_id]);
        persistence::set_favourite_track(&database, playlist_only_id, true)
            .expect("star playlist song");

        persistence::rename_playlist(&database, imported.id, "Forbidden").expect("name protected");
        persistence::remove_playlist(&database, imported.id).expect("deletion protected");
        persistence::set_playlist_entry(&database, imported.id, saved_id, false)
            .expect("removal protected");
        persistence::set_playlist_entry(&database, imported.id, file_id, true)
            .expect("addition protected");
        persistence::set_playlist_entry(&database, local_id, playlist_only_id, true)
            .expect("local playlist rejects remote song");
        assert!(persistence::playlists(&database)
            .expect("read-only content")
            .contains(&imported));
        persistence::set_playlist_group(&database, imported.id, Some(group_id))
            .expect("local grouping");
        persistence::set_playlist_picture(&database, imported.id, Some(&[1, 2, 3]))
            .expect("local picture");

        let mut changed = listing.clone();
        changed.playlists = vec![youtube_music_playlist(
            "retained-list",
            "Provider renamed",
            &["abcdefghijk", "lmnopqrstuv", "abcdefghijk"],
        )];
        let before_resync = LibrarySnapshot::read(&database).expect("before resync");
        sync_youtube_music_library(&database, &changed, Path::new("cookies.txt")).expect("resync");
        assert!(!before_resync.is_current(&database));
        let playlists = persistence::playlists(&database).expect("updated playlists");
        let retained = playlists
            .iter()
            .find(|playlist| playlist.id == imported.id)
            .expect("same row");
        assert_eq!(retained.name, "Provider renamed");
        assert_eq!(retained.track_ids, [saved_id, playlist_only_id]);
        assert_eq!(retained.group_id, Some(group_id));
        assert!(retained.picture);
        assert_eq!(
            persistence::playlist_picture(&database, imported.id),
            Ok(Some(vec![1, 2, 3]))
        );
        assert!(!playlists.iter().any(|playlist| playlist.id == dropped_id));
        assert_eq!(
            playlists
                .into_iter()
                .filter(|playlist| playlist.source != PlaylistSource::YouTubeMusic)
                .collect::<Vec<_>>(),
            other_playlists
        );
        assert_eq!(
            persistence::favourite_track_ids(&database),
            Ok(vec![playlist_only_id])
        );
        assert!(list_tracks(&database)
            .expect("playlist-only song retained")
            .iter()
            .any(|track| track.id == playlist_only_id));

        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary::default(),
            Path::new("cookies.txt"),
        )
        .expect("complete empty library");
        assert_eq!(
            persistence::playlists(&database),
            Ok(other_playlists.clone())
        );
        assert_eq!(count_youtube_music_tracks(&database), Ok(0));
        assert_eq!(
            persistence::playlist_groups(&database)
                .expect("groups")
                .len(),
            1
        );
        assert_eq!(
            persistence::playlist_picture(&database, local_id),
            Ok(Some(vec![4, 5]))
        );

        sync_youtube_music_library(&database, &listing, Path::new("cookies.txt"))
            .expect("reconnect");
        let before_disconnect = LibrarySnapshot::read(&database).expect("before disconnect");
        remove_youtube_music_tracks(&database).expect("disconnect");
        assert!(!before_disconnect.is_current(&database));
        assert_eq!(persistence::playlists(&database), Ok(other_playlists));
        assert_eq!(
            persistence::playlist_groups(&database)
                .expect("groups")
                .len(),
            1
        );
        assert_eq!(list_tracks(&database).expect("other sources").len(), 2);
    }

    #[test]
    fn youtube_music_playlist_validation_prevents_partial_imports_or_pruning() {
        let (_directory, database) = fresh_database();
        let original = YouTubeMusicLibrary {
            tracks: vec![youtube_music_song("abcdefghijk", "Kept")],
            playlists: vec![youtube_music_playlist(
                "kept-list",
                "Kept",
                &["abcdefghijk"],
            )],
        };
        sync_youtube_music_library(&database, &original, Path::new("kept-cookies.txt"))
            .expect("saved");
        let before = LibrarySnapshot::read(&database).expect("saved snapshot");
        let time = crate::persistence::youtube_music_last_sync(&database).expect("sync time");
        let tracks: Vec<_> = (0..=SCAN_BATCH)
            .map(|index| youtube_music_song(&format!("video{index:06}"), "New"))
            .collect();
        for playlists in [
            vec![youtube_music_playlist(
                "missing-song",
                "Incomplete",
                &["abcdefghijk"],
            )],
            vec![youtube_music_playlist("", "Missing identity", &[])],
            vec![
                youtube_music_playlist("duplicate", "First", &[]),
                youtube_music_playlist("duplicate", "Second", &[]),
            ],
        ] {
            let invalid = YouTubeMusicLibrary {
                tracks: tracks.clone(),
                playlists,
            };
            assert!(
                sync_youtube_music_library(&database, &invalid, Path::new("rejected.txt")).is_err()
            );
            assert!(before.is_current(&database));
            assert_eq!(list_tracks(&database), Ok(before.tracks.clone()));
            assert_eq!(
                crate::persistence::playlists(&database),
                Ok(before.playlists.clone())
            );
            assert_eq!(
                crate::persistence::youtube_music_last_sync(&database),
                Ok(time)
            );
            assert_eq!(
                crate::persistence::youtube_music_cookie_file(&database),
                Ok(Some(PathBuf::from("kept-cookies.txt")))
            );
        }
    }

    #[test]
    fn youtube_music_playlist_order_spans_write_bursts_without_duplicates_or_gaps() {
        let (_directory, database) = fresh_database();
        let mut listing = YouTubeMusicLibrary {
            tracks: (0..=PLAYLIST_BURST)
                .map(|index| youtube_music_song(&format!("video{index:06}"), "Song"))
                .collect(),
            playlists: Vec::new(),
        };
        let mut track_identifiers: Vec<_> = listing
            .tracks
            .iter()
            .rev()
            .map(|track| track.identifier.clone())
            .collect();
        track_identifiers.insert(PLAYLIST_BURST - 1, track_identifiers[0].clone());
        listing.playlists = vec![
            YouTubeMusicPlaylist {
                identifier: "large".to_owned(),
                name: "Large".to_owned(),
                track_identifiers,
            },
            youtube_music_playlist("empty", "Empty", &[]),
        ];
        sync_youtube_music_library(&database, &listing, Path::new("cookies.txt"))
            .expect("large sync");
        let tracks = list_tracks(&database).expect("tracks");
        let expected: Vec<_> = listing
            .tracks
            .iter()
            .rev()
            .map(|song| {
                tracks
                    .iter()
                    .find(|track| track.path == format!("youtube-music://{}", song.identifier))
                    .expect("song row")
                    .id
            })
            .collect();
        let playlists = crate::persistence::playlists(&database).expect("playlists");
        assert_eq!(playlists[0].track_ids, expected);
        assert!(playlists[1].track_ids.is_empty());
        let positions: Vec<i64> = database
            .lock()
            .prepare(
                "SELECT position FROM playlist_entries WHERE playlist_id = ?1 ORDER BY position",
            )
            .expect("positions")
            .query_map([playlists[0].id], |row| row.get(0))
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("positions read");
        assert_eq!(positions, (0..=PLAYLIST_BURST as i64).collect::<Vec<_>>());
    }

    #[test]
    fn youtube_music_sync_updates_metadata_and_preserves_retained_track_state() {
        let (directory, database) = fresh_database();
        let first = YouTubeMusicLibrary {
            tracks: vec![
                youtube_music_song("abcdefghijk", "Kept"),
                youtube_music_song("lmnopqrstuv", "Removed"),
            ],
            ..YouTubeMusicLibrary::default()
        };
        let report =
            sync_youtube_music_library(&database, &first, Path::new("fixture-cookies.txt"))
                .expect("first sync");
        assert_eq!((report.added, report.updated, report.removed), (2, 0, 0));
        assert_eq!(count_youtube_music_tracks(&database), Ok(2));
        let tracks = list_tracks(&database).expect("tracks");
        let kept = tracks
            .iter()
            .find(|track| track.title == "Kept")
            .expect("kept");
        let removed = tracks
            .iter()
            .find(|track| track.title == "Removed")
            .expect("removed");
        let kept_id = kept.id;
        crate::persistence::set_favourite_track(&database, kept_id, true).expect("star kept");
        crate::persistence::set_favourite_track(&database, removed.id, true).expect("star removed");
        store_loudness(&database, kept_id, -14.0, 0.9);
        database
            .lock()
            .execute("UPDATE tracks SET added_at = 123 WHERE id = ?1", [kept_id])
            .expect("fixed date");

        let mut changed = youtube_music_song("abcdefghijk", "Updated");
        changed.metadata.artist = Some("New artist".to_owned());
        changed.metadata.album = Some("New album".to_owned());
        changed.metadata.album_artist = Some("Album artist".to_owned());
        changed.metadata.composer = Some("Composer".to_owned());
        changed.metadata.genre = Some("Jazz".to_owned());
        changed.metadata.year = Some(2026);
        changed.metadata.track_number = Some(4);
        changed.metadata.track_total = Some(12);
        changed.metadata.disc_number = Some(2);
        changed.metadata.disc_total = Some(3);
        changed.metadata.duration_secs = 321.0;
        changed.artwork_address = Some("https://images.example/new-cover.jpg".to_owned());
        let report = sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![changed],
                ..YouTubeMusicLibrary::default()
            },
            Path::new("replacement-cookies.txt"),
        )
        .expect("second sync");
        assert_eq!((report.added, report.updated, report.removed), (0, 1, 1));
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![kept_id])
        );

        let tracks = list_tracks(&database).expect("updated tracks");
        assert_eq!(tracks.len(), 1);
        let track = &tracks[0];
        assert_eq!(track.id, kept_id);
        assert_eq!(track.source, TrackSource::YouTubeMusic);
        assert_eq!(track.path, "youtube-music://abcdefghijk");
        assert_eq!(track.folder_id, None);
        assert_eq!(track.server_id, None);
        assert_eq!(track.artwork_item, None);
        assert!(track.server_libraries.is_empty());
        assert_eq!(track.title, "Updated");
        assert_eq!(track.artist.as_deref(), Some("New artist"));
        assert_eq!(track.album.as_deref(), Some("New album"));
        assert_eq!(track.album_artist.as_deref(), Some("Album artist"));
        assert_eq!(track.composer.as_deref(), Some("Composer"));
        assert_eq!(track.genre.as_deref(), Some("Jazz"));
        assert_eq!(track.year, Some(2026));
        assert_eq!(track.track_number, Some(4));
        assert_eq!(track.track_total, Some(12));
        assert_eq!(track.disc_number, Some(2));
        assert_eq!(track.disc_total, Some(3));
        assert_eq!(track.duration_secs, 321.0);
        assert_eq!(track.added_at, 123);
        assert_eq!(
            track.artwork_address.as_deref(),
            Some("https://images.example/new-cover.jpg")
        );
        assert_eq!(
            cover_source(&database, kept_id),
            CoverSource::Address {
                artwork_address: track.artwork_address.clone()
            }
        );
        let measurement: (f64, f64) = database
            .lock()
            .query_row(
                "SELECT loudness_lufs, loudness_peak FROM tracks WHERE id = ?1",
                [kept_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("measurement survives");
        assert_eq!(measurement, (-14.0, 0.9));
        let last_sync = crate::persistence::youtube_music_last_sync(&database).expect("sync time");
        assert!(last_sync.is_some_and(|time| time > 0));
        drop(database);

        let reopened = crate::persistence::init(directory.path().join("data")).expect("restart");
        assert_eq!(list_tracks(&reopened), Ok(tracks));
        assert_eq!(
            crate::persistence::favourite_track_ids(&reopened),
            Ok(vec![kept_id])
        );
        assert_eq!(
            crate::persistence::youtube_music_last_sync(&reopened),
            Ok(last_sync)
        );
        assert_eq!(
            crate::persistence::youtube_music_cookie_file(&reopened),
            Ok(Some(PathBuf::from("replacement-cookies.txt")))
        );
    }

    #[test]
    fn provider_sync_and_disconnect_leave_other_sources_and_their_stars_alone() {
        let (directory, database) = fresh_database();
        let file = directory.path().join("music/file.wav");
        write_wav(&file);
        assert_eq!(
            import_files(
                &database,
                vec![file.to_str().expect("Unicode path").to_owned()]
            )
            .added,
            1
        );
        let file_id = list_tracks(&database).expect("file")[0].id;
        crate::persistence::set_favourite_track(&database, file_id, true).expect("star file");
        let server_id = signed_in(&database);
        sync_server_tracks(
            &database,
            server_id,
            &[library("music", "Music")],
            vec![listed_in(starred(server_song("a", "Server")), &["music"])],
        )
        .expect("server sync");
        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "YouTube")],
                ..YouTubeMusicLibrary::default()
            },
            Path::new("fixture-cookies.txt"),
        )
        .expect("YouTube sync");
        let tracks = list_tracks(&database).expect("all sources");
        assert_eq!(tracks.len(), 3);
        assert_eq!(
            tracks
                .iter()
                .filter(|track| track.source == TrackSource::File)
                .count(),
            1
        );
        assert_eq!(
            tracks
                .iter()
                .filter(|track| track.source == TrackSource::Jellyfin)
                .count(),
            1
        );
        assert_eq!(
            tracks
                .iter()
                .filter(|track| track.source == TrackSource::YouTubeMusic)
                .count(),
            1
        );
        let youtube_id = tracks
            .iter()
            .find(|track| track.source == TrackSource::YouTubeMusic)
            .expect("YouTube track")
            .id;
        crate::persistence::set_favourite_track(&database, youtube_id, true).expect("star YouTube");
        let server_track = tracks
            .iter()
            .find(|track| track.source == TrackSource::Jellyfin)
            .expect("server track")
            .clone();
        let server_libraries = crate::persistence::server_libraries(&database).expect("libraries");

        remove_youtube_music_tracks(&database).expect("disconnect YouTube");
        assert_eq!(count_youtube_music_tracks(&database), Ok(0));
        assert_eq!(
            crate::persistence::youtube_music_last_sync(&database),
            Ok(None)
        );
        assert_eq!(
            crate::persistence::youtube_music_cookie_file(&database),
            Ok(None)
        );
        let remaining = list_tracks(&database).expect("remaining");
        assert_eq!(remaining.len(), 2);
        assert!(remaining.contains(&server_track));
        assert!(remaining.iter().any(|track| track.id == file_id));
        assert_eq!(
            crate::persistence::server_libraries(&database),
            Ok(server_libraries)
        );
        let favourites =
            crate::persistence::favourite_track_ids(&database).expect("remaining stars");
        assert_eq!(favourites.len(), 2);
        assert!(favourites.contains(&file_id) && favourites.contains(&server_track.id));

        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "YouTube again")],
                ..YouTubeMusicLibrary::default()
            },
            Path::new("fixture-cookies.txt"),
        )
        .expect("reconnect YouTube");
        sync_server_tracks(&database, server_id, &[], Vec::new()).expect("empty server sync");
        assert_eq!(count_youtube_music_tracks(&database), Ok(1));
        crate::persistence::remove_media_server(&database, server_id).expect("disconnect server");
        assert_eq!(count_youtube_music_tracks(&database), Ok(1));
        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary::default(),
            Path::new("fixture-cookies.txt"),
        )
        .expect("empty YouTube sync");
        assert_eq!(list_tracks(&database).expect("only file").len(), 1);
        assert_eq!(
            crate::persistence::favourite_track_ids(&database),
            Ok(vec![file_id])
        );
        assert!(crate::persistence::youtube_music_last_sync(&database)
            .expect("empty sync time")
            .is_some());
    }

    #[test]
    fn file_scans_pruning_covers_and_background_loudness_never_use_youtube_music_rows() {
        let (directory, database) = fresh_database();
        let folder = directory.path().join("music");
        let file = folder.join("song.wav");
        write_wav(&file);
        let watched = watch(&database, &folder).expect("watch folder");
        assert_eq!(watched.added, 1);
        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "Remote")],
                ..YouTubeMusicLibrary::default()
            },
            Path::new("fixture-cookies.txt"),
        )
        .expect("sync remote");
        let tracks = list_tracks(&database).expect("tracks");
        let remote = tracks
            .iter()
            .find(|track| track.source == TrackSource::YouTubeMusic)
            .expect("remote");
        let file_id = tracks
            .iter()
            .find(|track| track.source == TrackSource::File)
            .expect("file")
            .id;
        assert!(cover_candidates(&database, remote.id).is_empty());
        assert!(track_cover(&database, remote.id).is_none());
        assert!(!store_measured_loudness(
            &database,
            &PendingLoudness {
                track_id: remote.id,
                path: remote.path.clone(),
            },
            -14.0,
            0.9,
        ));
        assert_eq!(
            tracks_needing_loudness(&database).expect("pending"),
            [PendingLoudness {
                track_id: file_id,
                path: file.to_str().expect("Unicode").to_owned()
            }]
        );
        let folder_id = list_watched_folders(&database).expect("folders")[0].id;
        assert_eq!(
            list_watched_folders(&database).expect("folders")[0].track_count,
            1
        );
        assert_eq!(rescan_library(&database).expect("rescan").updated, 1);
        assert_eq!(count_youtube_music_tracks(&database), Ok(1));

        // Ownership, not the spelling of the path, guards all local-file work.
        database
            .lock()
            .execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                rusqlite::params![
                    folder.join("missing.wav").to_str().expect("Unicode"),
                    remote.id
                ],
            )
            .expect("source ownership fixture");
        std::fs::remove_file(&file).expect("remove local file");
        let report = rescan_library(&database).expect("prune vanished file");
        assert_eq!(report.removed, 1);
        assert_eq!(count_youtube_music_tracks(&database), Ok(1));
        assert!(tracks_needing_loudness(&database)
            .expect("no pending files")
            .is_empty());
        assert!(cover_candidates(&database, remote.id).is_empty());
        remove_watched_folder(&database, folder_id).expect("remove watched folder");
        assert_eq!(count_youtube_music_tracks(&database), Ok(1));
    }

    #[test]
    fn youtube_music_validation_checks_the_entire_listing_before_any_write() {
        let (_directory, database) = fresh_database();
        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "Cached")],
                ..YouTubeMusicLibrary::default()
            },
            Path::new("fixture-cookies.txt"),
        )
        .expect("initial sync");
        database
            .lock()
            .execute(
                "UPDATE user_preferences SET youtube_music_last_sync_at = 123 WHERE id = 1",
                [],
            )
            .expect("known last sync");
        let before = list_tracks(&database).expect("cached");
        let valid: Vec<_> = (0..=SCAN_BATCH)
            .map(|index| youtube_music_song(&format!("video{index:06}"), "New"))
            .collect();
        for invalid in [
            youtube_music_song("", "Missing identifier"),
            youtube_music_song("too/long/id", "Path separator"),
            youtube_music_song("abcdefghij!", "Invalid character"),
            {
                let mut song = youtube_music_song("lmnopqrstuv", "Invalid duration");
                song.metadata.duration_secs = f64::NAN;
                song
            },
            {
                let mut song = youtube_music_song("lmnopqrstuv", "Invalid gain");
                song.metadata.rg_album_gain_db = Some(f64::INFINITY);
                song
            },
            valid[0].clone(),
        ] {
            let mut listing = valid.clone();
            listing.push(invalid);
            assert!(sync_youtube_music_library(
                &database,
                &YouTubeMusicLibrary {
                    tracks: listing,
                    ..YouTubeMusicLibrary::default()
                },
                Path::new("replacement-cookies.txt")
            )
            .is_err());
            assert_eq!(list_tracks(&database), Ok(before.clone()));
            assert_eq!(
                crate::persistence::youtube_music_last_sync(&database),
                Ok(Some(123))
            );
            assert_eq!(
                crate::persistence::youtube_music_cookie_file(&database),
                Ok(Some(PathBuf::from("fixture-cookies.txt")))
            );
        }
        let report = sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: valid,
                ..YouTubeMusicLibrary::default()
            },
            Path::new("fixture-cookies.txt"),
        )
        .expect("multiple write bursts");
        assert_eq!(report.added, SCAN_BATCH + 1);
        assert_eq!(report.removed, 1);
        assert_eq!(count_youtube_music_tracks(&database), Ok(SCAN_BATCH + 1));
    }

    #[test]
    fn youtube_music_pruning_connection_and_sync_time_commit_as_one_change() {
        let (_directory, database) = fresh_database();
        sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "Cached")],
                playlists: vec![youtube_music_playlist(
                    "cached-list",
                    "Cached",
                    &["abcdefghijk"],
                )],
            },
            Path::new("kept-cookies.txt"),
        )
        .expect("initial sync");
        let before = list_tracks(&database).expect("cached songs");
        let playlists_before = crate::persistence::playlists(&database).expect("cached playlists");
        let last_sync = crate::persistence::youtube_music_last_sync(&database).expect("sync time");
        database
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_connection_update BEFORE UPDATE ON user_preferences
             BEGIN SELECT RAISE(ABORT, 'preference write failed'); END;",
            )
            .expect("force a final transaction failure");

        assert!(sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary::default(),
            Path::new("replacement-cookies.txt")
        )
        .is_err());
        assert_eq!(list_tracks(&database), Ok(before.clone()));
        assert_eq!(
            crate::persistence::playlists(&database),
            Ok(playlists_before.clone())
        );
        assert_eq!(
            crate::persistence::youtube_music_last_sync(&database),
            Ok(last_sync)
        );
        assert_eq!(
            crate::persistence::youtube_music_cookie_file(&database),
            Ok(Some(PathBuf::from("kept-cookies.txt")))
        );
        assert!(remove_youtube_music_tracks(&database).is_err());
        assert_eq!(list_tracks(&database), Ok(before));
        assert_eq!(
            crate::persistence::playlists(&database),
            Ok(playlists_before)
        );
        assert_eq!(
            crate::persistence::youtube_music_last_sync(&database),
            Ok(last_sync)
        );
        assert_eq!(
            crate::persistence::youtube_music_cookie_file(&database),
            Ok(Some(PathBuf::from("kept-cookies.txt")))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_cookie_path_that_is_not_unicode_is_rejected_before_importing_songs() {
        use std::os::unix::ffi::OsStrExt;
        let (_directory, database) = fresh_database();
        let cookie_file = Path::new(std::ffi::OsStr::from_bytes(b"caf\xe9.txt"));
        assert!(sync_youtube_music_library(
            &database,
            &YouTubeMusicLibrary {
                tracks: vec![youtube_music_song("abcdefghijk", "Not imported")],
                ..YouTubeMusicLibrary::default()
            },
            cookie_file,
        )
        .is_err());
        assert_eq!(count_youtube_music_tracks(&database), Ok(0));
        assert_eq!(
            crate::persistence::youtube_music_last_sync(&database),
            Ok(None)
        );
        assert_eq!(
            crate::persistence::youtube_music_cookie_file(&database),
            Ok(None)
        );
    }

    #[test]
    fn unknown_track_sources_fail_row_mapping_instead_of_becoming_files() {
        let (_directory, database) = fresh_database();
        database
            .lock()
            .execute(
                "INSERT INTO tracks (path, title, format, source)
             VALUES ('opaque://song', 'Unknown', 'AAC', 'unknown-provider')",
                [],
            )
            .expect("unknown source fixture");
        let error = list_tracks(&database).expect_err("source must be decoded strictly");
        assert!(error.contains("unknown track source"));
    }
}
