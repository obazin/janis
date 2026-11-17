//! The desktop media-control boundary, independent of its platform protocol.

use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    Seek(f64),
    SetPosition {
        track_id: String,
        position_seconds: f64,
    },
    SetVolume(f64),
    SetShuffle(bool),
    SetRepeat(bool),
    Raise,
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    /// Opaque host identity; the platform encodes its own wire identifier.
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: Option<f64>,
    /// Local/public addresses only, never a signed provider or server URL.
    pub source_url: Option<String>,
}

#[derive(Debug)]
pub struct Artwork {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub playback: PlaybackStatus,
    pub metadata: Option<Metadata>,
    pub artwork: Option<Arc<Artwork>>,
    pub position_seconds: f64,
    pub observed_at: Instant,
    pub volume: f64,
    pub shuffle: bool,
    pub repeat: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    /// Changes only for seek discontinuities, not ordinary position reports.
    pub seek_serial: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            playback: PlaybackStatus::Stopped,
            metadata: None,
            artwork: None,
            position_seconds: 0.0,
            observed_at: Instant::now(),
            volume: 0.8,
            shuffle: false,
            repeat: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
            can_go_next: false,
            can_go_previous: false,
            seek_serial: 0,
        }
    }
}

pub struct Controls {
    pub(crate) updates: Option<tokio::sync::watch::Sender<Snapshot>>,
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,
}

impl Controls {
    /// Replaces an obsolete report rather than queueing position updates.
    pub fn update(&self, snapshot: Snapshot) {
        if let Some(updates) = &self.updates {
            updates.send_replace(snapshot);
        }
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        // Closing the reports lets the worker release its service and artwork.
        drop(self.updates.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
