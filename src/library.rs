//! The track library: watched folders, the scanner, and cover art — the
//! database side of Janis's music collection.
//!
//! Per-file tag/property/cover reading lives in the `audio-stack-rs` facade
//! (`crate::audio::{read_metadata, read_cover, audio_extension}`); this module
//! walks folders, upserts the results into `tracks` keyed by absolute path (so
//! a rescan updates in place and never duplicates), and resolves playback gain.
//! A media server's songs are rows too, keyed by a `jellyfin://` address
//! instead of a path, written by [`sync_server_tracks`] and never touched by the
//! file scanner or the pruner.
//! Every scan here blocks for as long as the walk takes, so callers run them
//! off the user-interface thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

use crate::audio::{
    audio_extension, gain_db, read_cover, read_metadata, CoverArt, Measured, Metadata,
};
use crate::persistence::DbState;

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: i64,
    pub folder_id: Option<i64>,
    /// The media server the song lives on; `None` for a file on disk.
    pub server_id: Option<i64>,
    /// The server item whose picture is the song's cover — its own, or its
    /// album's. `None` for a file, and for a server song with no picture.
    pub artwork_item: Option<String>,
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

/// Where an upserted row comes from: a scanned file (in a watched folder, or
/// imported on its own), or a song on a media server.
#[derive(Clone, Copy, Debug)]
enum Origin<'a> {
    File {
        folder_id: Option<i64>,
    },
    Server {
        server_id: i64,
        artwork_item: Option<&'a str>,
    },
}

/// Upserts one scanned file or server song. Returns `true` when the row was
/// newly inserted.
fn upsert_track(
    conn: &rusqlite::Connection,
    origin: Origin<'_>,
    path: &str,
    meta: &Metadata,
) -> Result<bool, String> {
    let (folder_id, server_id, artwork_item) = match origin {
        Origin::File { folder_id } => (folder_id, None, None),
        Origin::Server {
            server_id,
            artwork_item,
        } => (None, Some(server_id), artwork_item),
    };
    let existed: bool = conn
        .query_row("SELECT 1 FROM tracks WHERE path = ?1", [path], |_| Ok(()))
        .map(|_| true)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(other),
        })
        .map_err(|e| format!("probe track row: {}", e))?;

    conn.execute(
        "INSERT INTO tracks (folder_id, path, title, artist, album, composer, duration_secs,
                             format, sample_rate, bit_depth, channels, lossless,
                             track_number, track_total, disc_number, disc_total,
                             year, genre, album_artist,
                             rg_track_gain_db, rg_track_peak,
                             rg_album_gain_db, rg_album_peak,
                             server_id, artwork_item)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
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
             artwork_item = excluded.artwork_item",
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
        ],
    )
    .map_err(|e| format!("upsert track {}: {}", path, e))?;
    Ok(!existed)
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
    let conn = db.lock();
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
            Ok(true) => written.added += 1,
            Ok(false) => written.updated += 1,
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
/// parent directory) is a directory right now and the file is not in it.
/// The guard is what keeps a rescan with an external drive ejected from
/// reading every row as missing and deleting the whole library — along with
/// the loudness measurements a rescan can never rebuild.
fn prune_missing(db: &DbState) -> Result<usize, String> {
    let (tracks, folders) = {
        let conn = db.lock();
        let tracks: Vec<(i64, Option<i64>, String)> = conn
            // A server's songs are not files: they are never stat-ed here.
            .prepare("SELECT id, folder_id, path FROM tracks WHERE server_id IS NULL")
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
            root_reachable && !path.exists()
        })
        .map(|(id, _, _)| id)
        .collect();

    let conn = db.lock();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("prune transaction: {}", e))?;
    for id in &doomed {
        tx.execute("DELETE FROM tracks WHERE id = ?1", [id])
            .map_err(|e| format!("prune track {}: {}", id, e))?;
    }
    tx.commit().map_err(|e| format!("prune commit: {}", e))?;
    Ok(doomed.len())
}

pub fn list_tracks(db: &DbState) -> Result<Vec<Track>, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, folder_id, path, title, artist, album, composer, duration_secs,
                    format, sample_rate, bit_depth, channels, lossless, added_at,
                    track_number, track_total, disc_number, disc_total,
                    year, genre, album_artist,
                    rg_track_gain_db, rg_track_peak, loudness_lufs, loudness_peak,
                    server_id, artwork_item, rg_album_gain_db, rg_album_peak
             FROM tracks ORDER BY added_at DESC, id DESC",
        )
        .map_err(|e| format!("prepare list_tracks: {}", e))?;
    let tracks = stmt
        .query_map([], |row| {
            Ok(Track {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                server_id: row.get(25)?,
                artwork_item: row.get(26)?,
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
    Ok(tracks)
}

pub fn list_watched_folders(db: &DbState) -> Result<Vec<WatchedFolder>, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT f.id, f.path, COUNT(t.id) FROM watched_folders f
             LEFT JOIN tracks t ON t.folder_id = f.id
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
        let connection = db.lock();
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
                    "UPDATE tracks SET folder_id = ?1 WHERE folder_id = ?2",
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
    let conn = db.lock();
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
}

/// Brings the server's songs in the library in line with a complete listing
/// of them: every listed song is upserted, and every song of this server
/// missing from the listing is removed — the listing is complete, so missing
/// means gone from the server. A listing that could not be read whole must
/// not reach here: it would read as songs deleted.
///
/// Same lock discipline as a folder scan: bursts of `SCAN_BATCH` upserts, one
/// transaction each, so the engine's loudness reads are never starved.
pub fn sync_server_tracks(
    database: &DbState,
    server_id: i64,
    songs: Vec<ServerSong>,
) -> Result<ScanReport, String> {
    let mut report = ScanReport::default();
    let listed: std::collections::HashSet<String> =
        songs.iter().map(|song| song.address.clone()).collect();
    for burst in songs.chunks(SCAN_BATCH) {
        let connection = database.lock();
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("server sync transaction: {error}"))?;
        let mut written = ScanReport::default();
        for song in burst {
            let origin = Origin::Server {
                server_id,
                artwork_item: song.artwork_item.as_deref(),
            };
            match upsert_track(&transaction, origin, &song.address, &song.metadata) {
                Ok(true) => written.added += 1,
                Ok(false) => written.updated += 1,
                Err(error) => {
                    log::warn!("{error}");
                    written.skipped += 1;
                }
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("server sync commit: {error}"))?;
        report.added += written.added;
        report.updated += written.updated;
        report.skipped += written.skipped;
    }

    let connection = database.lock();
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

/// Where a track's cover comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverSource {
    /// The file, or its album's files (`track_cover`).
    File,
    /// The server's picture of this item; `None` when the server has none.
    Server { artwork_item: Option<String> },
}

/// Where a track's cover comes from. An unknown track reads as a file, whose
/// lookup then finds nothing.
pub fn cover_source(database: &DbState, track_id: i64) -> CoverSource {
    let connection = database.lock();
    connection
        .query_row(
            "SELECT server_id IS NOT NULL, artwork_item FROM tracks WHERE id = ?1",
            [track_id],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map(|(on_server, artwork_item)| {
            if on_server {
                CoverSource::Server { artwork_item }
            } else {
                CoverSource::File
            }
        })
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

/// The gain a track plays at, in dB.
///
/// The album gain first, so the songs of one record keep the loudness
/// differences their mastering gave them; the track gain when the album has
/// The gains measured while Janis runs, at most one per local file: a queue
/// takes a track's from here once it is known, ahead of the gain the library
/// read before the measurement.
#[derive(Debug, Default)]
pub struct SessionGains {
    measured: std::collections::HashMap<i64, f64>,
}

impl SessionGains {
    /// The gain `track` plays at.
    pub fn gain_of(&self, track: &Track) -> f64 {
        self.measured
            .get(&track.id)
            .copied()
            .unwrap_or(track.gain_db)
    }

    /// A measurement of `track_id` ended: the gain it plays at from now on.
    pub fn measured(&mut self, track_id: i64, lufs: f64, peak: f64) -> f64 {
        let gain = resolved_gain(Gains::default(), Some(lufs), Some(peak));
        self.measured.insert(track_id, gain);
        gain
    }
}

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

/// A file the background measurement still has to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLoudness {
    pub track_id: i64,
    pub path: String,
}

/// The files with no gain at all — no ReplayGain tag, no measurement — that
/// the background measurement should read, in library order. A server's songs
/// are left out: their gains come with the server's listing, and their audio
/// is not on this disk.
pub fn tracks_needing_loudness(db: &DbState) -> Result<Vec<PendingLoudness>, String> {
    let conn = db.lock();
    let mut statement = conn
        .prepare(
            "SELECT id, path FROM tracks
             WHERE server_id IS NULL
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

    // A server song's cover comes from its server (`server_artwork`), never
    // from a file.
    let Ok(own) = conn.query_row(
        "SELECT path FROM tracks WHERE id = ?1 AND server_id IS NULL",
        [track_id],
        |r| r.get::<_, String>(0),
    ) else {
        return Vec::new();
    };

    // `IS` rather than `=` so two tracks with no album artist still match.
    let siblings = conn
        .prepare(
            "SELECT t.path FROM tracks t, tracks cur
             WHERE cur.id = ?1
               AND t.id != cur.id
               AND t.server_id IS NULL
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
    use super::Track;

    pub fn track(id: i64, title: &str) -> Track {
        Track {
            id,
            folder_id: None,
            server_id: None,
            artwork_item: None,
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
            self.server_id = Some(server_id);
            self.artwork_item = Some(item.to_owned());
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
        }
    }

    fn signed_in(database: &DbState) -> i64 {
        crate::persistence::save_media_server(
            database,
            &crate::persistence::NewMediaServer {
                address: "http://server".to_owned(),
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

        let second = sync_server_tracks(&db, server_id, vec![server_song("a", "One again")])
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

    #[test]
    fn a_rescan_never_touches_a_servers_songs() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let db = crate::persistence::init(directory.path().to_path_buf()).expect("db");
        let server_id = signed_in(&db);
        sync_server_tracks(&db, server_id, vec![server_song("a", "One")]).expect("sync");

        let report = rescan_library(&db).expect("rescan");

        assert_eq!(report.removed, 0);
        assert_eq!(list_tracks(&db).expect("tracks").len(), 1);
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
        sync_server_tracks(&database, server_id, vec![server_song("a", "One")]).expect("sync");

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
        sync_server_tracks(&database, server_id, vec![song]).expect("sync");

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

        let gain = gains.measured(1, -12.0, 0.5);

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
                                     disc_number, track_number, server_id)
                 VALUES (?1, ?2, 'T', 'FLAC', ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    row.id,
                    row.path,
                    row.album,
                    row.artist,
                    row.album_artist,
                    row.disc,
                    row.number,
                    row.server_id
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
}
