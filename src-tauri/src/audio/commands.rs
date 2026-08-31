//! The Tauri IPC surface for playback.
//!
//! Every command is a thin wrapper over an [`AudioEngine`](super::AudioEngine)
//! facade method: deserialize the frontend's parameters, call the engine, map
//! any error to a `String`. No audio logic lives here — it is all in
//! `audio-stack-rs`. Commands follow the crate convention: sync, taking
//! `tauri::State<'_, _>` fully qualified, returning `Result<T, String>`.
//!
//! Volume and EQ still write straight into the engine's realtime atomics (via
//! the facade), so a slider is audible on the next callback; their persistence
//! stays on `persistence::set_volume` / `set_eq`.

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use super::{AudioDevice, AudioEngine, EngineEvent, QueueEntry, Source, Subscribers};

/// Attaches the webview's two channels and replays current state, so a
/// reloaded page catches up with audio that never stopped.
#[tauri::command]
pub fn audio_subscribe(
    engine: tauri::State<'_, AudioEngine>,
    subscribers: tauri::State<'_, Arc<Subscribers>>,
    events: Channel<EngineEvent>,
    frames: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    subscribers.subscribe(events, frames);
    engine.describe();
    Ok(())
}

#[tauri::command]
pub fn audio_devices(engine: tauri::State<'_, AudioEngine>) -> Result<Vec<AudioDevice>, String> {
    engine.devices()
}

#[tauri::command]
pub fn audio_set_device(
    engine: tauri::State<'_, AudioEngine>,
    device_id: Option<String>,
) -> Result<(), String> {
    engine.set_device(device_id);
    Ok(())
}

#[tauri::command]
pub fn audio_load_queue(
    engine: tauri::State<'_, AudioEngine>,
    tracks: Vec<QueueEntry>,
    index: usize,
) -> Result<(), String> {
    engine.load_queue(tracks, index);
    Ok(())
}

/// Connects to a station and hands the buffered reader to the engine.
///
/// The only async command: it awaits the HTTP connect and prefetch so the
/// engine thread never blocks on the network, resolving once the station is
/// buffered so the frontend can tell connecting from playing. `now_playing`
/// names the station's metadata provider when it has one — the frontend owns
/// the station list, so it is the frontend that knows.
///
/// Takes the `AppHandle` rather than a `State` because a `State` borrow cannot
/// be held across the `.await`.
#[tauri::command]
pub async fn audio_play_stream(
    app: tauri::AppHandle,
    station_id: String,
    url: String,
    now_playing: Option<Source>,
) -> Result<(), String> {
    let engine = app.state::<AudioEngine>();
    engine.play_stream(station_id, url, now_playing).await
}

#[tauri::command]
pub fn audio_play(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.play();
    Ok(())
}

#[tauri::command]
pub fn audio_pause(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.pause();
    Ok(())
}

#[tauri::command]
pub fn audio_toggle(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.toggle();
    Ok(())
}

#[tauri::command]
pub fn audio_stop(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.stop();
    Ok(())
}

#[tauri::command]
pub fn audio_next(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.next();
    Ok(())
}

#[tauri::command]
pub fn audio_previous(engine: tauri::State<'_, AudioEngine>) -> Result<(), String> {
    engine.previous();
    Ok(())
}

#[tauri::command]
pub fn audio_jump_to(engine: tauri::State<'_, AudioEngine>, index: usize) -> Result<(), String> {
    engine.jump_to(index);
    Ok(())
}

#[tauri::command]
pub fn audio_seek(engine: tauri::State<'_, AudioEngine>, position_secs: f64) -> Result<(), String> {
    engine.seek(position_secs);
    Ok(())
}

#[tauri::command]
pub fn audio_set_shuffle(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_shuffle(enabled);
    Ok(())
}

#[tauri::command]
pub fn audio_set_repeat(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_repeat(enabled);
    Ok(())
}

#[tauri::command]
pub fn audio_set_normalize(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_normalize(enabled);
    Ok(())
}

#[tauri::command]
pub fn audio_set_gapless(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_gapless(enabled);
    Ok(())
}

#[tauri::command]
pub fn audio_set_crossfade(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_crossfade(enabled);
    Ok(())
}

#[tauri::command]
pub fn audio_set_volume(engine: tauri::State<'_, AudioEngine>, volume: f64) -> Result<(), String> {
    engine.set_volume(volume);
    Ok(())
}

#[tauri::command]
pub fn audio_set_eq(engine: tauri::State<'_, AudioEngine>, gains: Vec<f64>) -> Result<(), String> {
    engine.set_eq(gains);
    Ok(())
}

/// Switches the equalizer between the realtime biquad filters and the
/// linear-phase FIR mode. The bands are the same either way — the FIR mode
/// removes the inter-band phase distortion and takes over from the callback
/// EQ so the two never stack, at the cost of a constant latency the engine
/// echoes back in its `firEq` event. Unlike the gains this does not reach the
/// realtime atomics directly: the effect lives in the decode chain, so a
/// change is heard once the already-buffered audio has played.
#[tauri::command]
pub fn audio_set_fir_eq(
    engine: tauri::State<'_, AudioEngine>,
    enabled: bool,
) -> Result<(), String> {
    engine.set_fir_eq(enabled);
    Ok(())
}
