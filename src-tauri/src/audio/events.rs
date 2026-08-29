//! What the engine tells the frontend.
//!
//! The Svelte `PlayerStore` is a mirror of this: Rust owns the queue and the
//! transport, and every change arrives here rather than being computed twice.
//!
//! Visualiser frames do **not** travel as one of these. They go over a second
//! channel as raw bytes — see [`super::analyser::FRAME_BYTES`] — because at
//! 60 Hz the JSON of 170 numbers would cost far more than the 170 bytes do.

use serde::Serialize;

/// What the engine is playing, if anything.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Idle,
    Local,
    Radio,
}

/// Note the two levels of `rename_all`: the one on the enum renames the
/// *variant tags*, and each variant needs its own to camel-case its fields.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
pub enum EngineEvent {
    /// Transport state. Sent on every change, and once on subscribe so a
    /// reloaded webview catches up with audio that never stopped.
    #[serde(rename_all = "camelCase")]
    State {
        playing: bool,
        mode: Mode,
        index: usize,
        queue_len: usize,
        shuffle: bool,
        repeat: bool,
        station_id: Option<String>,
    },
    /// Roughly 10 Hz. The frontend interpolates between these with
    /// `performance.now()`, so the playhead stays smooth without paying for
    /// 60 Hz of IPC.
    #[serde(rename_all = "camelCase")]
    Position {
        position_secs: f64,
        /// Zero for radio, which has no end.
        duration_secs: f64,
    },
    /// The queue moved on — including a gapless transition, which is emitted
    /// when the boundary actually reaches the device rather than when the
    /// decoder crossed it.
    #[serde(rename_all = "camelCase")]
    TrackChanged { index: usize },
    /// The format of the source now playing, for the Now Playing badges.
    #[serde(rename_all = "camelCase")]
    Format {
        sample_rate: u32,
        channels: u16,
        codec: String,
    },
    /// What a station is currently playing. Every field is optional: ICY
    /// carries one free-form string, and what can be pulled out of it varies
    /// by station. All-`None` means the station said nothing useful.
    #[serde(rename_all = "camelCase")]
    StreamMetadata {
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        /// Cover art as a `data:` URL. Fetched and encoded in Rust, because
        /// the webview's CSP allows no remote images.
        cover: Option<String>,
    },
    /// The output device actually in use — what the Settings screen shows
    /// instead of the hard-coded "System default" it used to claim.
    #[serde(rename_all = "camelCase")]
    Device {
        name: String,
        sample_rate: u32,
        channels: u16,
    },
    /// Playback failed. Non-fatal: the engine stays alive and idle.
    #[serde(rename_all = "camelCase")]
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(event: &EngineEvent) -> String {
        serde_json::to_string(event).expect("engine events must serialise")
    }

    #[test]
    fn events_are_tagged_and_camel_cased() {
        let event = EngineEvent::Position {
            position_secs: 12.5,
            duration_secs: 300.0,
        };
        let encoded = json(&event);
        assert!(encoded.contains(r#""event":"position""#), "{encoded}");
        assert!(encoded.contains(r#""positionSecs":12.5"#), "{encoded}");
        assert!(encoded.contains(r#""durationSecs":300.0"#), "{encoded}");
    }

    #[test]
    fn state_fields_are_camel_cased_too() {
        // The enum-level rename_all only touches variant tags, so a missing
        // per-variant attribute would silently ship snake_case to the UI.
        let encoded = json(&EngineEvent::State {
            playing: true,
            mode: Mode::Local,
            index: 3,
            queue_len: 10,
            shuffle: false,
            repeat: true,
            station_id: None,
        });
        assert!(encoded.contains(r#""queueLen":10"#), "{encoded}");
        assert!(encoded.contains(r#""stationId":null"#), "{encoded}");
        assert!(encoded.contains(r#""mode":"local""#), "{encoded}");
    }

    #[test]
    fn every_variant_carries_its_tag() {
        let events = [
            EngineEvent::TrackChanged { index: 1 },
            EngineEvent::Format {
                sample_rate: 44_100,
                channels: 2,
                codec: "flac".into(),
            },
            EngineEvent::Device {
                name: "Speakers".into(),
                sample_rate: 48_000,
                channels: 2,
            },
            EngineEvent::Error {
                message: "boom".into(),
            },
        ];
        for event in &events {
            let encoded = json(event);
            assert!(encoded.contains(r#""event":"#), "{encoded}");
            assert!(encoded.contains(r#""data":"#), "{encoded}");
        }
    }
}
