//! Janis's thin audio layer over the `audio-stack-rs` facade.
//!
//! The whole signal path — decode, DSP, output, transport, and web radio —
//! plus audio-file metadata parsing live in the `audio-stack-rs` crate. This
//! module is only the bridge to the application: the re-exported facade, the
//! [`EventSink`](audio_stack_rs::EventSink) that hands engine output to the
//! user interface, and the boot wiring.

use std::sync::{Arc, Mutex};

// Re-export the facade items the rest of Janis uses, so nothing outside this
// module names `audio_stack_rs` directly.
pub use audio_stack_rs::{
    audio_extension, gain_db, measure_file, read_cover, read_metadata, AudioEngine, CoverArt,
    EngineEvent, Location, Measured, Metadata, Mode, OverviewSlice, QueueEntry, RemoteFile, Source,
    Store, AUDIO_EXTENSIONS, CENTER_FREQS, EQ_BAND_COUNT, FRAME_BYTES, LOSSLESS_EXTENSIONS,
    OVERVIEW_SLICES_PER_SECOND,
};

use crate::library::Track;

/// One visualiser frame: the waveform points, then the band magnitudes.
pub type VisualFrame = [u8; FRAME_BYTES];

/// Where the engine's output lands on its way to the user interface.
///
/// Transport events go to `on_event`, which the application points at its
/// user-interface thread. Visualiser frames are not queued: only the latest
/// one is kept, for whoever draws next.
pub struct EngineBridge {
    on_event: Box<dyn Fn(EngineEvent) + Send + Sync>,
    frame: Mutex<Option<VisualFrame>>,
}

impl EngineBridge {
    pub fn new(on_event: impl Fn(EngineEvent) + Send + Sync + 'static) -> Self {
        Self {
            on_event: Box::new(on_event),
            frame: Mutex::new(None),
        }
    }

    /// The most recent visualiser frame, or `None` before the first arrives.
    pub fn latest_frame(&self) -> Option<VisualFrame> {
        self.frame.lock().ok().and_then(|guard| *guard)
    }
}

impl audio_stack_rs::EventSink for EngineBridge {
    fn send_event(&self, event: EngineEvent) {
        (self.on_event)(event);
    }

    /// Called from the engine thread sixty times a second, so it never waits:
    /// a frame that finds the slot busy is dropped, which is invisible.
    fn send_frame(&self, bytes: &[u8]) {
        let Ok(frame) = VisualFrame::try_from(bytes) else {
            return;
        };
        if let Ok(mut guard) = self.frame.try_lock() {
            *guard = Some(frame);
        }
    }
}

/// What the engine needs to know about a library track to play it, given
/// where its audio is (the file at its path, or the address its server gives)
/// and the gain it plays at.
pub fn queue_entry(track: &Track, location: Location, gain_db: f64) -> QueueEntry {
    QueueEntry {
        track_id: track.id,
        location,
        duration_secs: track.duration_secs,
        gain_db,
    }
}

/// The queue of `tracks`, each with where its audio is and the gain it plays
/// at, in order.
pub fn queue_entries(
    tracks: &[Track],
    location_of: impl Fn(&Track) -> Location,
    gain_of: impl Fn(&Track) -> f64,
) -> Vec<QueueEntry> {
    tracks
        .iter()
        .map(|track| queue_entry(track, location_of(track), gain_of(track)))
        .collect()
}

/// Boots the engine thread with Janis's loudness store and event bridge. The
/// output device opens lazily on first play, so a machine with no sound card
/// still boots.
pub fn init(store: Arc<dyn Store>, bridge: Arc<EngineBridge>) -> AudioEngine {
    audio_stack_rs::init(store, bridge, None)
}

/// Stops the engine and waits (bounded) for its thread, so the output stream is
/// dropped while the process is alive. Called on application exit.
pub fn shutdown(engine: &AudioEngine) {
    engine.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_stack_rs::EventSink;

    #[test]
    fn the_bridge_keeps_only_the_latest_well_formed_frame() {
        let bridge = EngineBridge::new(|_| {});
        assert_eq!(bridge.latest_frame(), None);

        bridge.send_frame(&[7; FRAME_BYTES]);
        bridge.send_frame(&[9; FRAME_BYTES]);
        bridge.send_frame(&[1; 3]);

        assert_eq!(bridge.latest_frame(), Some([9; FRAME_BYTES]));
    }

    #[test]
    fn a_queue_carries_each_tracks_place_length_and_gain_in_order() {
        let tracks = [
            crate::library::test_support::track(1, "a").duration(100.0),
            crate::library::test_support::track(2, "b").duration(200.0),
        ];
        let entries = queue_entries(
            &tracks,
            |track| Location::File(std::path::PathBuf::from(&track.path)),
            |track| -(track.id as f64),
        );
        assert_eq!(
            entries,
            [
                QueueEntry {
                    track_id: 1,
                    location: Location::File("/music/1.flac".into()),
                    duration_secs: 100.0,
                    gain_db: -1.0,
                },
                QueueEntry {
                    track_id: 2,
                    location: Location::File("/music/2.flac".into()),
                    duration_secs: 200.0,
                    gain_db: -2.0,
                },
            ]
        );
    }
}
