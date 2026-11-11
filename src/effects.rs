//! What the state modules ask the application to do on their behalf.
//!
//! The player, the equalizer and the playback preferences hold state and
//! decide; they never touch the engine, the database or a timer themselves.
//! Each of their methods returns the [`Effect`]s its decision implies, and the
//! application controller carries them out — which is what lets every decision
//! be tested without an audio device or a database.

use crate::library::Track;
use crate::persistence::PlaybackOption;
use crate::radio::Station;

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    // ── Engine: transport ────────────────────────────────────────────────────
    /// Replace the engine's queue and start at `index`.
    LoadQueue {
        tracks: Vec<Track>,
        index: usize,
    },
    /// Connect to a station off the user-interface thread, then report back
    /// through `Player::station_connection_finished`.
    ConnectStation(Station),
    Toggle,
    Next,
    Previous,
    JumpTo(usize),
    /// Seek to a position in seconds.
    Seek(f64),
    SetShuffle(bool),
    SetRepeat(bool),

    // ── Engine: parameters ───────────────────────────────────────────────────
    SetVolume(f64),
    SetEqualizerGains(Vec<f64>),
    SetLinearPhase(bool),
    SetGapless(bool),
    SetCrossfade(bool),
    SetNormalize(bool),

    // ── Persistence ──────────────────────────────────────────────────────────
    /// Save the volume once it has stopped moving (a drag emits dozens of
    /// values a second).
    PersistVolumeDebounced(f64),
    PersistEqualizer {
        gains: Vec<f64>,
        preset: String,
    },
    /// As `PersistEqualizer`, once the sliders have stopped moving.
    PersistEqualizerDebounced {
        gains: Vec<f64>,
        preset: String,
    },
    PersistLinearPhase(bool),
    PersistPlaybackOption {
        option: PlaybackOption,
        enabled: bool,
    },

    // ── Presentation ─────────────────────────────────────────────────────────
    /// Show this track's cover art as the current art; `None` clears it.
    LoadCover(Option<i64>),
    /// Show the art a station announced (a `data:` URL); `None` clears it.
    ShowStreamCover(Option<String>),
    /// The engine reported its linear-phase mode and what it costs.
    LinearPhaseReported {
        enabled: bool,
        latency_seconds: f32,
    },
    /// Tell the user something failed.
    NotifyError {
        /// Translation key of the message.
        key: &'static str,
        parameters: Vec<(&'static str, String)>,
        /// Collapses a burst of the same failure into one notification.
        dedupe_key: Option<&'static str>,
    },
}
