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
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
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
                    server_id, artwork_item
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
                // Tag first, measurement second, and the peak from whichever
                // source supplied the gain.
                gain_db: {
                    let tag_gain: Option<f64> = row.get(21)?;
                    let tag_peak: Option<f64> = row.get(22)?;
                    let lufs: Option<f64> = row.get(23)?;
                    let measured_peak: Option<f64> = row.get(24)?;
                    gain_db(tag_gain, lufs, tag_peak.or(measured_peak))
                },
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

/// Registers a folder and scans it. The scan is a long walk + tag reads.
pub fn add_watched_folder(db: &DbState, path: &str) -> Result<ScanReport, String> {
    let folder = PathBuf::from(path);
    if !folder.is_dir() {
        return Err(format!("not a directory: {}", path));
    }
    // The lock covers only the registration; the walk itself runs unlocked
    // (see `scan_folder`).
    let folder_id: i64 = {
        let conn = db.lock();
        conn.execute(
            "INSERT OR IGNORE INTO watched_folders (path) VALUES (?1)",
            [path],
        )
        .map_err(|e| format!("register folder: {}", e))?;
        conn.query_row(
            "SELECT id FROM watched_folders WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .map_err(|e| format!("resolve folder id: {}", e))?
    };
    Ok(scan_folder(db, folder_id, &folder))
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
        "SELECT rg_track_gain_db IS NULL AND loudness_lufs IS NULL
         FROM tracks WHERE id = ?1",
        [track_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|needs| needs != 0)
    .unwrap_or(false)
}

/// Records a loudness measurement taken while a track played.
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
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("temp dir");
        std::fs::File::create(path)
            .and_then(|mut file| file.write_all(&wav))
            .expect("write test wav");
    }

    #[test]
    fn importing_a_file_of_a_watched_folder_leaves_it_in_that_folder() {
        let dir = std::env::temp_dir().join("janis-import-test");
        let _ = std::fs::remove_dir_all(&dir);
        let db = crate::persistence::init(dir.clone()).expect("db");
        let folder = dir.join("watched");
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

        let _ = std::fs::remove_dir_all(&dir);
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
        let dir = std::env::temp_dir().join("janis-server-sync-test");
        let _ = std::fs::remove_dir_all(&dir);
        let db = crate::persistence::init(dir.clone()).expect("db");
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

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rescan_never_touches_a_servers_songs() {
        let dir = std::env::temp_dir().join("janis-server-rescan-test");
        let _ = std::fs::remove_dir_all(&dir);
        let db = crate::persistence::init(dir.clone()).expect("db");
        let server_id = signed_in(&db);
        sync_server_tracks(&db, server_id, vec![server_song("a", "One")]).expect("sync");

        let report = rescan_library(&db).expect("rescan");

        assert_eq!(report.removed, 0);
        assert_eq!(list_tracks(&db).expect("tracks").len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_spares_tracks_whose_root_is_unreachable() {
        let dir = std::env::temp_dir().join("janis-prune-test");
        let _ = std::fs::remove_dir_all(&dir);
        let db = crate::persistence::init(dir.clone()).expect("db");

        let live_root = dir.join("live");
        let kept = live_root.join("kept.wav");
        write_wav(&kept);
        let ejected_root = dir.join("ejected");

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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
