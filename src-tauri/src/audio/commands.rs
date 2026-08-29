//! The IPC surface for playback.
//!
//! Every command here only queues work for the engine thread, so none of them
//! block the UI thread. They follow the crate convention: sync, taking
//! `tauri::State<'_, _>` fully qualified, returning `Result<T, String>`.
//!
//! Volume and EQ are the exception to "everything goes through the channel" —
//! they write straight into the shared atomic block, so a slider move is
//! audible on the next callback rather than after the engine thread's next
//! turn around its loop. Persistence of those values stays where it was, on
//! `persistence::set_volume` / `set_eq`.

use std::path::PathBuf;

use serde::Deserialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use super::engine::EngineCommand;
use super::events::EngineEvent;
use super::nowplaying;
use super::output::{self, AudioDevice};
use super::queue::QueueEntry;
use super::AudioEngine;

/// One track as the frontend sends it. Deliberately not the full `Track`:
/// playback needs a path, a length and a gain, and nothing else.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub track_id: i64,
    pub path: String,
    pub duration_secs: f64,
    /// Normalization gain in dB. Zero until the library records one.
    #[serde(default)]
    pub gain_db: f32,
}

impl From<QueueItem> for QueueEntry {
    fn from(item: QueueItem) -> Self {
        Self {
            track_id: item.track_id,
            path: PathBuf::from(item.path),
            duration_secs: item.duration_secs,
            gain_db: item.gain_db,
        }
    }
}

/// Attaches the webview's two channels and replays current state, so a
/// reloaded page catches up with audio that never stopped.
#[tauri::command]
pub fn audio_subscribe(
    engine: tauri::State<'_, AudioEngine>,
    events: Channel<EngineEvent>,
    frames: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    engine.subscribers().subscribe(events, frames);
    engine.send(EngineCommand::Describe)
}

#[tauri::command]
pub fn audio_devices() -> Result<Vec<AudioDevice>, String> {
    output::list_devices()
}

#[tauri::command]
pub fn audio_set_device(
    engine: tauri::State<'_, AudioEngine>,
    device_id: Option<String>,
) -> Result<(), String> {
    engine.send(EngineCommand::SetDevice(device_id))
}

#[tauri::command]
pub fn audio_load_queue(
    engine: tauri::State<'_, AudioEngine>,
    tracks: Vec<QueueItem>,
    index: usize,
) -> Result<(), String> {
    engine.send(EngineCommand::LoadQueue {
        entries: tracks.into_iter().map(QueueEntry::from).collect(),
        index,
    })
}

/// Connects to a station and hands the buffered reader to the engine.
///
/// The only async command here: it awaits the HTTP round trip and the initial
/// prefetch so the engine thread never blocks on the network. It resolves once
/// the station is buffered, so the frontend can tell connecting from playing.
///
/// `now_playing` names the station's metadata endpoint when it has one. The
/// frontend owns the station list, so it is the frontend that knows.
#[tauri::command]
pub async fn audio_play_stream(
    app: tauri::AppHandle,
    station_id: String,
    url: String,
    now_playing: Option<nowplaying::Source>,
) -> Result<(), String> {
    let stream = super::stream::open(&url).await?;
    let engine = app.state::<AudioEngine>();

    // Claim the epoch before starting anything: it retires the previous
    // station's poller and tags this one so late answers can be discarded.
    let epoch = engine.next_station_epoch();
    engine.send(EngineCommand::PlayStream {
        station_id,
        stream: Box::new(stream),
        has_provider: now_playing.is_some(),
    })?;

    if let Some(source) = now_playing {
        nowplaying::spawn(engine.commands(), engine.station_epoch(), epoch, source);
    }
    Ok(())
}

#[tauri::command]
pub fn audio_play(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Play)
}

#[tauri::command]
pub fn audio_pause(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Pause)
}

#[tauri::command]
pub fn audio_toggle(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Toggle)
}

#[tauri::command]
pub fn audio_stop(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Stop)
}

#[tauri::command]
pub fn audio_next(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Next)
}

#[tauri::command]
pub fn audio_previous(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.send(EngineCommand::Previous)
}

#[tauri::command]
pub fn audio_jump_to(engine: tauri::State<'_, AudioEngine>, index: usize) -> Result<(), String> {
    engine.send(EngineCommand::JumpTo(index))
}

#[tauri::command]
pub fn audio_seek(engine: tauri::State<'_, AudioEngine>, position_secs: f64) -> Result<(), String> {
    engine.send(EngineCommand::Seek(position_secs))
}

#[tauri::command]
pub fn audio_set_shuffle(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.send(EngineCommand::SetShuffle(enabled))
}

#[tauri::command]
pub fn audio_set_repeat(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.send(EngineCommand::SetRepeat(enabled))
}

/// Straight to the atomics — see the module note on why this skips the queue.
#[tauri::command]
pub fn audio_set_volume(engine: tauri::State<'_, AudioEngine>, volume: f64) -> Result<(), String> {
    engine.params().set_volume(volume as f32);
    Ok(())
}

#[tauri::command]
pub fn audio_set_eq(engine: tauri::State<'_, AudioEngine>, gains: Vec<f64>) -> Result<(), String> {
    let gains: Vec<f32> = gains.iter().map(|g| *g as f32).collect();
    engine.params().set_eq_gains(&gains);
    Ok(())
}
