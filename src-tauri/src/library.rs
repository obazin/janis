//! The track library: watched folders, the scanner, and cover art — the
//! database side of Janis's music collection.
//!
//! Per-file tag/property/cover reading lives in the `audio-stack-rs` facade
//! (`crate::audio::{read_metadata, read_cover, audio_extension}`); this module
//! walks folders, upserts the results into `tracks` keyed by absolute path (so
//! a rescan updates in place and never duplicates), and resolves playback gain.
//! The webview never touches the filesystem — cover art reaches it as base64
//! over IPC.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

use crate::audio::{
    audio_extension, gain_db, read_cover, read_metadata, CoverArt, Measured, Metadata,
};
use crate::persistence::DbState;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub folder_id: Option<i64>,
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
    /// measured loudness. Zero when neither is known. Resolved here so the
    /// frontend never has to re-derive it.
    pub gain_db: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchedFolder {
    pub id: i64,
    pub path: String,
    pub track_count: i64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub removed: usize,
}

/// Upserts one scanned file. Returns `true` when the row was newly inserted.
fn upsert_track(
    conn: &rusqlite::Connection,
    folder_id: Option<i64>,
    path: &str,
    meta: &Metadata,
) -> Result<bool, String> {
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
                             rg_album_gain_db, rg_album_peak)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
         ON CONFLICT(path) DO UPDATE SET
             folder_id = excluded.folder_id,
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
             rg_album_peak = excluded.rg_album_peak",
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
        match read_metadata(path) {
            Ok(meta) => {
                batch.push((path.to_string_lossy().into_owned(), meta));
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
    for (path, meta) in batch.drain(..) {
        match upsert_track(&tx, folder_id, &path, &meta) {
            Ok(true) => report.added += 1,
            Ok(false) => report.updated += 1,
            Err(e) => {
                log::warn!("{}", e);
                report.skipped += 1;
            }
        }
    }
    if let Err(e) = tx.commit() {
        log::warn!("scan batch commit: {}", e);
    }
}

/// Deletes rows whose file vanished from disk. Returns the number removed.
///
/// "Vanished" requires the place the file lived to still be reachable: a track
/// is only pruned when its watched folder's root (or, for an ad-hoc import, its
/// parent directory) is a directory right now and the file is not in it.
/// Without that guard, a rescan with an external drive ejected read every row
/// as missing and silently deleted the whole library — along with the loudness
/// measurements a rescan can never rebuild.
fn prune_missing(db: &DbState) -> Result<usize, String> {
    let (tracks, folders) = {
        let conn = db.lock();
        let tracks: Vec<(i64, Option<i64>, String)> = conn
            .prepare("SELECT id, folder_id, path FROM tracks")
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

#[tauri::command]
pub fn list_tracks(db: tauri::State<'_, DbState>) -> Result<Vec<Track>, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, folder_id, path, title, artist, album, composer, duration_secs,
                    format, sample_rate, bit_depth, channels, lossless, added_at,
                    track_number, track_total, disc_number, disc_total,
                    year, genre, album_artist,
                    rg_track_gain_db, rg_track_peak, loudness_lufs, loudness_peak
             FROM tracks ORDER BY added_at DESC, id DESC",
        )
        .map_err(|e| format!("prepare list_tracks: {}", e))?;
    let tracks = stmt
        .query_map([], |row| {
            Ok(Track {
                id: row.get(0)?,
                folder_id: row.get(1)?,
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

#[tauri::command]
pub fn list_watched_folders(db: tauri::State<'_, DbState>) -> Result<Vec<WatchedFolder>, String> {
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

/// Registers a folder and scans it. The scan is a long walk + tag reads, so it
/// runs on the blocking pool — the UI stays live and shows the report when it
/// lands.
#[tauri::command]
pub async fn add_watched_folder(app: AppHandle, path: String) -> Result<ScanReport, String> {
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err(format!("not a directory: {}", path));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
        // The lock covers only the registration; the walk itself runs unlocked
        // (see `scan_folder`).
        let folder_id: i64 = {
            let conn = db.lock();
            conn.execute(
                "INSERT OR IGNORE INTO watched_folders (path) VALUES (?1)",
                [&path],
            )
            .map_err(|e| format!("register folder: {}", e))?;
            conn.query_row(
                "SELECT id FROM watched_folders WHERE path = ?1",
                [&path],
                |r| r.get(0),
            )
            .map_err(|e| format!("resolve folder id: {}", e))?
        };
        Ok(scan_folder(db.inner(), folder_id, &folder))
    })
    .await
    .map_err(|e| format!("scan task: {}", e))?
}

#[tauri::command]
pub fn remove_watched_folder(db: tauri::State<'_, DbState>, folder_id: i64) -> Result<(), String> {
    let conn = db.lock();
    conn.execute("DELETE FROM watched_folders WHERE id = ?1", [folder_id])
        .map_err(|e| format!("remove folder: {}", e))?;
    Ok(())
}

/// Ad-hoc single-file import (the Add-music dialog and OS drag-and-drop). Files
/// land with no watched folder; a rescan keeps them as long as the file still
/// exists.
#[tauri::command]
pub async fn import_files(app: AppHandle, paths: Vec<String>) -> Result<ScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
        let mut report = ScanReport::default();
        // Same lock discipline as a folder scan: probe files unlocked, write
        // in bursts.
        let mut batch: Vec<(String, Metadata)> = Vec::with_capacity(SCAN_BATCH);
        for p in paths {
            match read_metadata(Path::new(&p)) {
                Ok(meta) => {
                    batch.push((p, meta));
                    if batch.len() >= SCAN_BATCH {
                        flush_batch(db.inner(), None, &mut batch, &mut report);
                    }
                }
                Err(e) => {
                    log::warn!("{}", e);
                    report.skipped += 1;
                }
            }
        }
        flush_batch(db.inner(), None, &mut batch, &mut report);
        Ok(report)
    })
    .await
    .map_err(|e| format!("import task: {}", e))?
}

/// Re-walks every watched folder (picking up new + changed files) and prunes
/// rows whose file is gone.
#[tauri::command]
pub async fn rescan_library(app: AppHandle) -> Result<ScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
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
            let sub = scan_folder(db.inner(), id, root);
            report.added += sub.added;
            report.updated += sub.updated;
            report.skipped += sub.skipped;
        }
        report.removed = prune_missing(db.inner())?;
        Ok(report)
    })
    .await
    .map_err(|e| format!("rescan task: {}", e))?
}

/// The engine's route to the loudness columns.
pub struct LoudnessStore(pub AppHandle);

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
pub fn needs_loudness(app: &AppHandle, track_id: i64) -> bool {
    let Some(db) = app.try_state::<DbState>() else {
        return false;
    };
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
pub fn store_loudness(app: &AppHandle, track_id: i64, lufs: f64, peak: f64) {
    let Some(db) = app.try_state::<DbState>() else {
        return;
    };
    let conn = db.lock();
    if let Err(e) = conn.execute(
        "UPDATE tracks SET loudness_lufs = ?2, loudness_peak = ?3 WHERE id = ?1",
        rusqlite::params![track_id, lufs, peak],
    ) {
        log::warn!("store loudness for track {}: {}", track_id, e);
    }
}

/// Cover art for one track, base64-encoded for the IPC hop.
///
/// Falls back to the rest of the album: a rip where only the first file carries
/// the picture is common, and every track on that record should still show the
/// sleeve. `None` only when nothing in the album has art — the frontend then
/// draws its generated gradient.
#[tauri::command]
pub async fn get_track_cover(app: AppHandle, track_id: i64) -> Result<Option<CoverArt>, String> {
    let paths = cover_candidates(&app, track_id);
    if paths.is_empty() {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || {
        for path in paths {
            if let Some(cover) = read_cover(&path) {
                return Ok(Some(cover));
            }
        }
        Ok(None)
    })
    .await
    .map_err(|e| format!("cover task: {}", e))?
}

/// How many album siblings to open before giving up. A record whose first dozen
/// files carry no art almost certainly has none.
const COVER_SIBLING_LIMIT: usize = 12;

/// The track's own file first, then its album siblings in playing order.
fn cover_candidates(app: &AppHandle, track_id: i64) -> Vec<String> {
    let db = app.state::<DbState>();
    let conn = db.lock();

    let Ok(own) = conn.query_row("SELECT path FROM tracks WHERE id = ?1", [track_id], |r| {
        r.get::<_, String>(0)
    }) else {
        return Vec::new();
    };

    // `IS` rather than `=` so two tracks with no album artist still match.
    let siblings = conn
        .prepare(
            "SELECT t.path FROM tracks t, tracks cur
             WHERE cur.id = ?1
               AND t.id != cur.id
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
            .and_then(|mut f| f.write_all(&wav))
            .expect("write test wav");
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
            "the unreachable volume's track must survive — ejecting a drive \
             and pressing Rescan used to silently delete its whole library"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
