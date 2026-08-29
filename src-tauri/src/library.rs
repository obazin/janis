//! The track library: watched folders, the metadata scanner, and cover art.
//!
//! Scanning walks a folder for audio files and reads tags + audio properties
//! via `lofty`, then upserts rows into `tracks` keyed by absolute path — so a
//! rescan updates metadata in place and never duplicates. This module only
//! ever produces metadata: the `audio` engine reads the files themselves, and
//! the webview never touches the filesystem at all — cover art reaches it as
//! base64 over IPC.

use base64::Engine;
use lofty::picture::PictureType;
use lofty::prelude::*;
use lofty::probe::Probe;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use walkdir::WalkDir;

use crate::persistence::DbState;

/// Extensions the scanner picks up. This list tracks what the `audio` engine
/// can decode, not what any browser accepts.
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "aif", "aiff",
];

/// Container formats that are always lossless. `m4a` is deliberately absent:
/// it can hold either AAC (lossy) or ALAC (lossless) and we don't inspect the
/// codec, so it is reported lossy — the conservative claim.
const LOSSLESS_EXTENSIONS: &[&str] = &["flac", "wav", "aif", "aiff"];

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverArt {
    pub mime: String,
    pub data_base64: String,
}

struct ScannedFile {
    title: String,
    artist: Option<String>,
    album: Option<String>,
    composer: Option<String>,
    duration_secs: f64,
    format: String,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    lossless: bool,
    track_number: Option<u32>,
    track_total: Option<u32>,
    disc_number: Option<u32>,
    disc_total: Option<u32>,
    year: Option<u32>,
    genre: Option<String>,
    album_artist: Option<String>,
}

fn audio_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    AUDIO_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Reads tags + properties for one file. Tag-less files still scan: the
/// filename stem becomes the title, properties come from the decoder.
fn read_metadata(path: &Path) -> Result<ScannedFile, String> {
    let ext = audio_extension(path).ok_or_else(|| "not an audio file".to_string())?;
    let tagged = Probe::open(path)
        .map_err(|e| format!("probe {}: {}", path.display(), e))?
        .read()
        .map_err(|e| format!("read {}: {}", path.display(), e))?;

    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let stem_title = || {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".to_string())
    };
    let non_empty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());

    let (title, artist, album, composer) = match tag {
        Some(tag) => (
            non_empty(tag.title().map(|c| c.into_owned())).unwrap_or_else(stem_title),
            non_empty(tag.artist().map(|c| c.into_owned())),
            non_empty(tag.album().map(|c| c.into_owned())),
            non_empty(tag.get_string(&ItemKey::Composer).map(|s| s.to_string())),
        ),
        None => (stem_title(), None, None, None),
    };

    // `get_string` reaches the same item whatever the underlying format wrote
    // it as — ID3v2.2/2.3/2.4 frames, Vorbis comments, MP4 atoms or APE keys
    // all normalise onto these keys, so this needs no per-format branching.
    let number = |key: ItemKey| -> Option<u32> {
        tag.and_then(|t| t.get_string(&key))
            // ID3 writes "3/12" into a single frame; lofty usually splits it,
            // but a tagger that wrote the pair verbatim must not parse to None.
            .and_then(|raw| {
                raw.split('/')
                    .next()
                    .map(str::trim)
                    .and_then(|n| n.parse().ok())
            })
    };
    let text = |key: ItemKey| non_empty(tag.and_then(|t| t.get_string(&key)).map(str::to_string));

    // A file with no track number still belongs somewhere in the album, and
    // the position is usually sitting in its filename.
    let from_name = track_number_from_filename(path);
    let track_number = number(ItemKey::TrackNumber).or(from_name.track);
    let disc_number = number(ItemKey::DiscNumber).or(from_name.disc);

    Ok(ScannedFile {
        title,
        artist,
        album,
        composer,
        duration_secs: props.duration().as_secs_f64(),
        format: ext.to_ascii_uppercase(),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        lossless: LOSSLESS_EXTENSIONS.contains(&ext.as_str()),
        track_number,
        track_total: number(ItemKey::TrackTotal),
        disc_number,
        disc_total: number(ItemKey::DiscTotal),
        year: number(ItemKey::Year).or_else(|| {
            // Vorbis and MP4 usually carry a full date; the leading four
            // digits are the year.
            text(ItemKey::RecordingDate).and_then(|d| d.get(..4).and_then(|y| y.parse().ok()))
        }),
        genre: text(ItemKey::Genre),
        album_artist: text(ItemKey::AlbumArtist),
    })
}

/// A disc and track position read out of a filename.
#[derive(Debug, Default, PartialEq, Eq)]
struct FilenamePosition {
    disc: Option<u32>,
    track: Option<u32>,
}

/// Recovers a track's position from its filename when the tags do not carry
/// one — `07 - Alive.flac`, `2-03 Reprise.mp3`, `104 Title.m4a`.
///
/// Only leading digits count, and only when something separates them from the
/// title. That is what keeps `1984 - Track.mp3` from reading as track 198 and
/// `2001 A Space Odyssey.mp3` from reading as a track at all.
fn track_number_from_filename(path: &Path) -> FilenamePosition {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return FilenamePosition::default();
    };
    let stem = stem.trim();

    let digits: String = stem.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 3 {
        return FilenamePosition::default();
    }
    let rest = &stem[digits.len()..];

    // `2-03 Title` / `2_03 Title`: a single leading digit, a separator, then
    // the real track number. Checked before the plain form so the disc is not
    // mistaken for the track.
    if digits.len() == 1 {
        if let Some(after) = rest.strip_prefix(['-', '_']) {
            let track_digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            let tail = &after[track_digits.len()..];
            if (1..=3).contains(&track_digits.len()) && starts_with_separator(tail) {
                return FilenamePosition {
                    disc: digits.parse().ok().filter(|d| *d > 0),
                    track: track_digits.parse().ok().filter(|t| *t > 0),
                };
            }
        }
    }

    if !starts_with_separator(rest) {
        return FilenamePosition::default();
    }
    FilenamePosition {
        disc: None,
        track: digits.parse().ok().filter(|t| *t > 0),
    }
}

/// Whether what follows the digits marks them as a standalone number rather
/// than the first part of a word or a longer figure.
fn starts_with_separator(rest: &str) -> bool {
    match rest.chars().next() {
        // Nothing after the digits at all — the whole stem is a number.
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '-' | '.' | '_'),
    }
}

/// Upserts one scanned file. Returns `true` when the row was newly inserted.
fn upsert_track(
    conn: &rusqlite::Connection,
    folder_id: Option<i64>,
    path: &str,
    meta: &ScannedFile,
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
                             year, genre, album_artist)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19)
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
             album_artist = excluded.album_artist",
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
        ],
    )
    .map_err(|e| format!("upsert track {}: {}", path, e))?;
    Ok(!existed)
}

/// Walks `folder` and upserts every audio file found under it.
fn scan_folder_into(conn: &rusqlite::Connection, folder_id: i64, folder: &Path) -> ScanReport {
    let mut report = ScanReport::default();
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
            Ok(meta) => match upsert_track(conn, Some(folder_id), &path.to_string_lossy(), &meta) {
                Ok(true) => report.added += 1,
                Ok(false) => report.updated += 1,
                Err(e) => {
                    log::warn!("{}", e);
                    report.skipped += 1;
                }
            },
            Err(e) => {
                log::warn!("{}", e);
                report.skipped += 1;
            }
        }
    }
    report
}

/// Deletes rows whose file vanished from disk. Returns the number removed.
fn prune_missing(conn: &rusqlite::Connection) -> Result<usize, String> {
    let paths: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM tracks")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .map_err(|e| format!("list track paths: {}", e))?;
    let mut removed = 0;
    for (id, path) in paths {
        if !Path::new(&path).exists() {
            conn.execute("DELETE FROM tracks WHERE id = ?1", [id])
                .map_err(|e| format!("prune track {}: {}", path, e))?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_extension_accepts_known_formats_case_insensitively() {
        assert_eq!(
            audio_extension(Path::new("/m/a.FLAC")).as_deref(),
            Some("flac")
        );
        assert_eq!(
            audio_extension(Path::new("/m/b.mp3")).as_deref(),
            Some("mp3")
        );
        assert_eq!(audio_extension(Path::new("/m/c.txt")), None);
        assert_eq!(audio_extension(Path::new("/m/noext")), None);
    }

    #[test]
    fn lossless_extensions_are_a_subset_of_audio_extensions() {
        for ext in LOSSLESS_EXTENSIONS {
            assert!(
                AUDIO_EXTENSIONS.contains(ext),
                "{ext} missing from AUDIO_EXTENSIONS"
            );
        }
        // m4a stays out deliberately: AAC-or-ALAC, reported lossy.
        assert!(!LOSSLESS_EXTENSIONS.contains(&"m4a"));
    }

    fn position(name: &str) -> FilenamePosition {
        track_number_from_filename(Path::new(name))
    }

    #[test]
    fn reads_a_leading_track_number() {
        for name in [
            "07 - Alive.flac",
            "07 Alive.flac",
            "07. Alive.flac",
            "07.Alive.flac",
            "07_Alive.flac",
        ] {
            assert_eq!(position(name).track, Some(7), "{name} should give track 7");
        }
    }

    #[test]
    fn reads_a_disc_and_track_pair() {
        assert_eq!(
            position("2-03 Reprise.mp3"),
            FilenamePosition {
                disc: Some(2),
                track: Some(3)
            }
        );
        assert_eq!(
            position("1_11 Closing Time.mp3"),
            FilenamePosition {
                disc: Some(1),
                track: Some(11)
            }
        );
    }

    #[test]
    fn a_three_digit_number_is_still_a_track() {
        // Some rips number across discs: 104 = disc 1, track 4.
        assert_eq!(position("104 - Title.m4a").track, Some(104));
    }

    #[test]
    fn a_year_is_not_a_track_number() {
        // The guard that matters: four digits are never a position, so this
        // must not read as track 198.
        assert_eq!(position("1984 - Track.mp3"), FilenamePosition::default());
        assert_eq!(
            position("2001 A Space Odyssey.mp3"),
            FilenamePosition::default()
        );
    }

    #[test]
    fn digits_running_into_the_title_are_not_a_track_number() {
        assert_eq!(position("07Alive.flac"), FilenamePosition::default());
        assert_eq!(position("99Luftballons.mp3"), FilenamePosition::default());
    }

    #[test]
    fn a_filename_with_no_leading_digits_yields_nothing() {
        assert_eq!(position("Alive.flac"), FilenamePosition::default());
        assert_eq!(
            position("Pearl Jam - Alive.flac"),
            FilenamePosition::default()
        );
    }

    #[test]
    fn track_zero_is_treated_as_absent() {
        // "00 - Intro" is a hidden-track convention, not position zero.
        assert_eq!(position("00 - Intro.mp3").track, None);
    }

    #[test]
    fn a_bare_number_is_the_whole_name() {
        assert_eq!(position("03.mp3").track, Some(3));
    }

    /// A minimal untagged WAV, so the scanner can be exercised without
    /// shipping a binary fixture.
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
    fn an_untagged_file_still_gets_its_position_from_its_name() {
        // The end-to-end path: no tags at all, so everything shown about this
        // track has to come from the filename and the decoder.
        let dir = std::env::temp_dir().join("janis-scan-test");
        let path = dir.join("04 - Blue in Green.wav");
        write_wav(&path);

        let meta = read_metadata(&path).expect("an untagged wav should still scan");

        assert_eq!(meta.track_number, Some(4), "recovered from the filename");
        assert_eq!(meta.title, "04 - Blue in Green", "the stem is the title");
        assert_eq!(meta.disc_number, None);
        assert_eq!(meta.year, None);
        assert_eq!(meta.format, "WAV");
        assert!(meta.lossless);

        let _ = std::fs::remove_file(&path);
    }

    /// Writes a cover picture into a file's ID3v2 tag.
    fn tag_with_cover(path: &Path, pic_type: PictureType) {
        use lofty::config::WriteOptions;
        use lofty::picture::{MimeType, Picture};
        use lofty::tag::{Tag, TagType};

        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(Picture::new_unchecked(
            pic_type,
            Some(MimeType::Jpeg),
            None,
            // Not a real JPEG; `read_cover` never decodes it, it only
            // re-encodes the bytes for the IPC hop.
            vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4],
        ));
        tag.save_to_path(path, WriteOptions::default())
            .expect("write cover tag");
    }

    #[test]
    fn cover_art_is_read_back_from_the_file() {
        let dir = std::env::temp_dir().join("janis-cover-test");
        let path = dir.join("with-art.wav");
        write_wav(&path);
        tag_with_cover(&path, PictureType::CoverFront);

        let cover = read_cover(path.to_str().expect("utf-8 path"))
            .expect("the picture just written should come back");
        assert_eq!(cover.mime, "image/jpeg");
        assert!(!cover.data_base64.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_with_no_picture_yields_none() {
        let dir = std::env::temp_dir().join("janis-cover-test");
        let path = dir.join("no-art.wav");
        write_wav(&path);

        assert!(read_cover(path.to_str().expect("utf-8 path")).is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_back_cover_is_used_when_there_is_no_front() {
        // Better the back of the sleeve than the generated gradient.
        let dir = std::env::temp_dir().join("janis-cover-test");
        let path = dir.join("back-art.wav");
        write_wav(&path);
        tag_with_cover(&path, PictureType::CoverBack);

        assert!(read_cover(path.to_str().expect("utf-8 path")).is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_disc_prefixed_name_survives_the_scanner() {
        let dir = std::env::temp_dir().join("janis-scan-test");
        let path = dir.join("2-11 Reprise.wav");
        write_wav(&path);

        let meta = read_metadata(&path).expect("should scan");
        assert_eq!(meta.disc_number, Some(2));
        assert_eq!(meta.track_number, Some(11));

        let _ = std::fs::remove_file(&path);
    }
}

#[tauri::command]
pub fn list_tracks(db: tauri::State<'_, DbState>) -> Result<Vec<Track>, String> {
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, folder_id, path, title, artist, album, composer, duration_secs,
                    format, sample_rate, bit_depth, channels, lossless, added_at,
                    track_number, track_total, disc_number, disc_total,
                    year, genre, album_artist
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

/// Registers a folder and scans it. The scan is a long walk + tag reads, so
/// it runs on the blocking pool — the UI stays live and shows the report
/// when it lands.
#[tauri::command]
pub async fn add_watched_folder(app: AppHandle, path: String) -> Result<ScanReport, String> {
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err(format!("not a directory: {}", path));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
        let conn = db.lock();
        conn.execute(
            "INSERT OR IGNORE INTO watched_folders (path) VALUES (?1)",
            [&path],
        )
        .map_err(|e| format!("register folder: {}", e))?;
        let folder_id: i64 = conn
            .query_row(
                "SELECT id FROM watched_folders WHERE path = ?1",
                [&path],
                |r| r.get(0),
            )
            .map_err(|e| format!("resolve folder id: {}", e))?;
        Ok(scan_folder_into(&conn, folder_id, &folder))
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

/// Ad-hoc single-file import (the Add-music dialog and OS drag-and-drop).
/// Files land with no watched folder; a rescan keeps them as long as the
/// file still exists.
#[tauri::command]
pub async fn import_files(app: AppHandle, paths: Vec<String>) -> Result<ScanReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
        let conn = db.lock();
        let mut report = ScanReport::default();
        for p in paths {
            let path = Path::new(&p);
            match read_metadata(path) {
                Ok(meta) => match upsert_track(&conn, None, &p, &meta) {
                    Ok(true) => report.added += 1,
                    Ok(false) => report.updated += 1,
                    Err(e) => {
                        log::warn!("{}", e);
                        report.skipped += 1;
                    }
                },
                Err(e) => {
                    log::warn!("{}", e);
                    report.skipped += 1;
                }
            }
        }
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
        let conn = db.lock();
        let folders: Vec<(i64, String)> = conn
            .prepare("SELECT id, path FROM watched_folders")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(|e| format!("list folders: {}", e))?;
        let mut report = ScanReport::default();
        for (id, path) in folders {
            let sub = scan_folder_into(&conn, id, Path::new(&path));
            report.added += sub.added;
            report.updated += sub.updated;
            report.skipped += sub.skipped;
        }
        report.removed = prune_missing(&conn)?;
        Ok(report)
    })
    .await
    .map_err(|e| format!("rescan task: {}", e))?
}

/// Cover art for one track, base64-encoded for the IPC hop.
///
/// Falls back to the rest of the album: a rip where only the first file
/// carries the picture is common, and every track on that record should still
/// show the sleeve. `None` only when nothing in the album has art — the
/// frontend then draws its generated gradient.
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

/// How many album siblings to open before giving up. A record whose first
/// dozen files carry no art almost certainly has none.
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

/// The first embedded picture in a file, preferring the front cover.
///
/// Searches every tag block, not just the primary one: a file can carry both
/// ID3v2 and APE, and the artwork is not always in the tag lofty considers
/// primary.
fn read_cover(path: &str) -> Option<CoverArt> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let pictures = || tagged.tags().iter().flat_map(|tag| tag.pictures());

    let picture = pictures()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())?;

    Some(CoverArt {
        mime: picture
            .mime_type()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "image/jpeg".to_string()),
        data_base64: base64::engine::general_purpose::STANDARD.encode(picture.data()),
    })
}
