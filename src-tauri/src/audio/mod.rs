//! Janis's thin audio layer over the `audio-stack-rs` facade.
//!
//! The whole signal path — decode, DSP, output, transport, and web radio —
//! plus audio-file metadata parsing live in the `audio-stack-rs` crate. This
//! module is only the bridge to Tauri: the IPC command surface ([`commands`]),
//! the [`EventSink`](audio_stack_rs::EventSink) that forwards engine output
//! onto the webview's two channels, and the boot wiring.

pub mod commands;

use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};

// Re-export the facade items the rest of Janis (commands, library, main) uses,
// so nothing outside this module names `audio_stack_rs` directly.
pub use audio_stack_rs::{
    audio_extension, gain_db, read_cover, read_metadata, AudioDevice, AudioEngine, CoverArt,
    EngineEvent, Measured, Metadata, QueueEntry, Source, Store,
};

/// The webview's two subscriptions: transport events, and visualiser frames.
///
/// Implements the engine's `EventSink`, forwarding each onto a Tauri `Channel`.
/// Both channels are replaced rather than accumulated on re-subscribe, because
/// a Vite hot reload builds a fresh webview and the old channels then `eval`
/// into a dead page. A send failure is expected and never fatal.
#[derive(Default)]
pub struct Subscribers {
    events: Mutex<Option<Channel<EngineEvent>>>,
    frames: Mutex<Option<Channel<InvokeResponseBody>>>,
}

impl Subscribers {
    pub fn subscribe(&self, events: Channel<EngineEvent>, frames: Channel<InvokeResponseBody>) {
        *self.events.lock().expect("subscriber mutex poisoned") = Some(events);
        *self.frames.lock().expect("subscriber mutex poisoned") = Some(frames);
    }
}

impl audio_stack_rs::EventSink for Subscribers {
    fn send_event(&self, event: EngineEvent) {
        if let Ok(guard) = self.events.lock() {
            if let Some(channel) = guard.as_ref() {
                let _ = channel.send(event);
            }
        }
    }

    /// Sends one visualiser frame as raw bytes.
    ///
    /// Frames must stay under Tauri's 1 KB raw threshold: below it the payload
    /// is delivered by a direct `webview.eval`, at or above it Tauri parks the
    /// data and makes the webview fetch it with an extra IPC round trip —
    /// unaffordable sixty times a second.
    fn send_frame(&self, bytes: &[u8]) {
        debug_assert!(bytes.len() < 1024, "frame must use the direct IPC path");
        if let Ok(guard) = self.frames.lock() {
            if let Some(channel) = guard.as_ref() {
                let _ = channel.send(InvokeResponseBody::Raw(bytes.to_vec()));
            }
        }
    }
}

/// Boots the engine thread with Janis's loudness store and event sink. The
/// output device opens lazily on first play, so a machine with no sound card
/// still boots.
pub fn init(store: Arc<dyn Store>, subscribers: Arc<Subscribers>) -> AudioEngine {
    audio_stack_rs::init(store, subscribers, None)
}

/// Stops the engine and waits (bounded) for its thread, so the output stream is
/// dropped while the process is alive. Called on app exit.
pub fn shutdown(engine: &AudioEngine) {
    engine.shutdown();
}
