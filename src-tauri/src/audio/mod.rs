//! The audio engine: decoding, DSP, output and transport.
//!
//! Janis decodes and plays audio in Rust rather than in the webview. One
//! signal path serves every source, so the EQ and the visualiser apply to
//! whatever is playing.
//!
//! Threading, and why it is shaped this way:
//!
//! - The **engine thread** owns the `cpal::Stream`, the decoders and the
//!   queue. The stream is `Send` in cpal 0.18, so this is a lifecycle choice
//!   rather than a trait one: the callback closure captures the ring consumer
//!   and the filter bank, so a device change has to rebuild all of them
//!   together, and dropping a stream blocks until the OS callback thread
//!   quiesces — not something to do on the UI thread.
//! - Commands reach that thread over a `crossbeam-channel`. The handle held
//!   in managed state is just the sender, the shared [`params::Params`] and
//!   the subscriber channels, so it stays `Send + Sync`.
//! - The **cpal callback** is the only realtime context. It pops frames from
//!   a lock-free ring, runs the EQ and gain, and taps a mono copy for the
//!   analyser. It never allocates, locks, blocks — or logs, since the log
//!   plugin takes a lock and writes to a file.

pub mod analyser;
pub mod codecs;
pub mod commands;
pub mod decode;
pub mod dsp;
pub mod engine;
pub mod events;
pub mod icy;
pub mod nowplaying;
pub mod opus;
pub mod output;
pub mod params;
pub mod queue;
pub mod resample;
pub mod stream;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use tauri::ipc::{Channel, InvokeResponseBody};

use engine::{Engine, EngineCommand};
use events::EngineEvent;
use params::Params;

/// The webview's two subscriptions: transport events, and visualiser frames.
///
/// Both are replaced rather than accumulated on re-subscribe, because a Vite
/// hot reload builds a fresh webview and the old channels then `eval` into a
/// dead page. A send failure is expected and never fatal.
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

    pub fn send_event(&self, event: EngineEvent) {
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
    pub fn send_frame(&self, bytes: &[u8]) {
        debug_assert!(bytes.len() < 1024, "frame must use the direct IPC path");
        if let Ok(guard) = self.frames.lock() {
            if let Some(channel) = guard.as_ref() {
                let _ = channel.send(InvokeResponseBody::Raw(bytes.to_vec()));
            }
        }
    }
}

/// The managed handle. Holds no audio state of its own — everything lives on
/// the engine thread, reachable only by command.
pub struct AudioEngine {
    commands: Sender<EngineCommand>,
    params: Arc<Params>,
    subscribers: Arc<Subscribers>,
    /// Bumped whenever what is playing changes. A now-playing poller carries
    /// the value it started with and stops as soon as it no longer matches,
    /// which is how a poller for a station the listener has left dies.
    station_epoch: Arc<AtomicU64>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AudioEngine {
    pub fn params(&self) -> &Params {
        &self.params
    }

    pub fn subscribers(&self) -> &Arc<Subscribers> {
        &self.subscribers
    }

    pub fn station_epoch(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.station_epoch)
    }

    /// Claims the next epoch, invalidating any poller still running.
    pub fn next_station_epoch(&self) -> u64 {
        self.station_epoch.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn commands(&self) -> Sender<EngineCommand> {
        self.commands.clone()
    }

    /// Queues a command for the engine thread. Never blocks: the channel is
    /// unbounded, so a `#[tauri::command]` cannot stall the UI thread.
    pub fn send(&self, command: EngineCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "audio engine is not running".to_string())
    }
}

/// Starts the engine thread. The output device is opened lazily, on first
/// play, so a machine with no sound card still boots.
pub fn init(device_id: Option<String>) -> AudioEngine {
    let (tx, rx) = crossbeam_channel::unbounded();
    let params = Arc::new(Params::default());
    let subscribers = Arc::new(Subscribers::default());

    let station_epoch = Arc::new(AtomicU64::new(0));
    let engine = Engine::new(
        rx,
        Arc::clone(&params),
        Arc::clone(&subscribers),
        Arc::clone(&station_epoch),
        device_id,
    );
    let join = std::thread::Builder::new()
        .name("janis-audio".into())
        .spawn(move || engine.run())
        .expect("spawn audio engine thread");

    AudioEngine {
        commands: tx,
        params,
        subscribers,
        station_epoch,
        join: Mutex::new(Some(join)),
    }
}

/// Stops the engine thread and waits for it, so the `cpal::Stream` is dropped
/// while the process is still alive. Without this, CoreAudio can call into
/// freed memory during teardown.
pub fn shutdown(engine: &AudioEngine) {
    let _ = engine.send(EngineCommand::Shutdown);
    if let Ok(mut guard) = engine.join.lock() {
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
    }
}
