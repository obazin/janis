//! The playback state the screens render.
//!
//! The engine owns the queue, the transport and the signal path; this is a
//! mirror of what it reports, plus the presentation the engine has no business
//! knowing about (the [`Track`] and [`Station`] values the screens show).
//! Radio goes through the same engine as local files, so a station gets the
//! equalizer and a real visualiser like anything else; the engine only knows
//! a station by its id.
//!
//! Every user action updates the mirror at once — the click is visible before
//! the engine confirms it — and returns the [`Effect`]s that make it real.

use std::time::Instant;

use crate::audio::{EngineEvent, Mode};
use crate::effects::Effect;
use crate::library::Track;
use crate::radio::Station;

/// What a station says it is playing, when it says anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

pub struct Player {
    queue: Vec<Track>,
    index: usize,
    playing: bool,
    mode: Mode,
    stream: StreamInfo,
    connecting: bool,
    station: Option<Station>,
    volume: f64,
    shuffle: bool,
    repeat: bool,
    position_seconds: f64,
    duration_seconds: f64,
    /// When `position_seconds` was last known exact, for interpolation.
    position_at: Instant,
    sample_rate: Option<u32>,
    device_name: Option<String>,
}

impl Player {
    pub fn new(now: Instant) -> Self {
        Self {
            queue: Vec::new(),
            index: 0,
            playing: false,
            mode: Mode::Idle,
            stream: StreamInfo::default(),
            connecting: false,
            station: None,
            volume: 0.8,
            shuffle: false,
            repeat: false,
            position_seconds: 0.0,
            duration_seconds: 0.0,
            position_at: now,
            sample_rate: None,
            device_name: None,
        }
    }

    pub fn queue(&self) -> &[Track] {
        &self.queue
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Local, radio, or idle when nothing is loaded.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn station(&self) -> Option<&Station> {
        self.station.as_ref()
    }

    pub fn stream(&self) -> &StreamInfo {
        &self.stream
    }

    /// True while a station is connecting and buffering.
    pub fn connecting(&self) -> bool {
        self.connecting
    }

    pub fn volume(&self) -> f64 {
        self.volume
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> bool {
        self.repeat
    }

    pub fn position_seconds(&self) -> f64 {
        self.position_seconds
    }

    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    /// Device sample rate, for the Settings readout.
    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    /// The local track playing or paused; `None` in radio and idle modes.
    pub fn current(&self) -> Option<&Track> {
        match self.mode {
            Mode::Local => self.queue.get(self.index),
            Mode::Idle | Mode::Radio => None,
        }
    }

    /// Fractional progress, interpolated between the engine's ~10 Hz position
    /// events so the playhead moves smoothly at the display's frame rate.
    pub fn live_progress(&self, now: Instant) -> f64 {
        if self.mode != Mode::Local || self.duration_seconds <= 0.0 {
            return 0.0;
        }
        let elapsed = if self.playing {
            now.saturating_duration_since(self.position_at)
                .as_secs_f64()
        } else {
            0.0
        };
        (self.position_seconds + elapsed).min(self.duration_seconds) / self.duration_seconds
    }

    /// Boot hydration: the volume comes from the preferences row.
    pub fn restore_volume(&mut self, volume: f64) -> Vec<Effect> {
        self.volume = volume.clamp(0.0, 1.0);
        vec![Effect::SetVolume(self.volume)]
    }

    /// Takes one engine event into the mirror.
    ///
    /// `station_by_id` resolves the station the engine names: the engine is
    /// the authority on which station plays, and recovering the value from the
    /// id is what keeps a mirror whose station was cleared by an interleaved
    /// local state event from showing "nothing playing" over live radio.
    pub fn apply(
        &mut self,
        event: EngineEvent,
        now: Instant,
        station_by_id: &dyn Fn(&str) -> Option<Station>,
    ) -> Vec<Effect> {
        match event {
            EngineEvent::State {
                playing,
                mode,
                index,
                shuffle,
                repeat,
                station_id,
                queue_len: _,
            } => {
                self.mode = mode;
                self.index = index;
                self.shuffle = shuffle;
                self.repeat = repeat;
                self.playing = playing;
                if mode == Mode::Radio {
                    let current_id = self.station.as_ref().map(|station| station.id.as_str());
                    if current_id != station_id.as_deref() {
                        self.station = station_id.as_deref().and_then(station_by_id);
                    }
                    Vec::new()
                } else {
                    self.station = None;
                    self.clear_stream_info()
                }
            }
            EngineEvent::Position {
                position_secs,
                duration_secs,
            } => {
                self.position_seconds = position_secs;
                self.duration_seconds = duration_secs;
                self.position_at = now;
                Vec::new()
            }
            EngineEvent::StreamMetadata {
                title,
                artist,
                album,
                cover,
            } => {
                self.stream = StreamInfo {
                    title,
                    artist,
                    album,
                };
                vec![Effect::ShowStreamCover(cover)]
            }
            EngineEvent::TrackChanged { index } => {
                self.index = index;
                vec![Effect::LoadCover(
                    self.queue.get(index).map(|track| track.id),
                )]
            }
            EngineEvent::Device {
                name, sample_rate, ..
            } => {
                self.device_name = Some(name);
                self.sample_rate = Some(sample_rate);
                Vec::new()
            }
            // The equalizer owns this state; the player only carries the
            // channel it arrives on.
            EngineEvent::FirEq {
                enabled,
                latency_secs,
            } => vec![Effect::LinearPhaseReported {
                enabled,
                latency_seconds: latency_secs,
            }],
            EngineEvent::Error { message } => {
                log::error!("audio engine: {message}");
                vec![Effect::NotifyError {
                    key: "error.playback",
                    parameters: vec![("message", message)],
                    dedupe_key: None,
                }]
            }
            // The queue replay serves a host whose mirror can restart while
            // the engine keeps playing. This mirror lives exactly as long as
            // the engine, and `play_queue` already holds the complete queue.
            EngineEvent::Queue { .. } | EngineEvent::Format { .. } => Vec::new(),
        }
    }

    /// Replaces the queue and starts at `index`.
    pub fn play_queue(&mut self, tracks: Vec<Track>, index: usize, now: Instant) -> Vec<Effect> {
        if tracks.is_empty() {
            return Vec::new();
        }
        let mut effects = self.clear_stream_info();
        self.station = None;
        self.index = index.min(tracks.len() - 1);
        self.queue = tracks;
        self.mode = Mode::Local;
        self.playing = true;
        self.position_seconds = 0.0;
        self.duration_seconds = self.queue[self.index].duration_secs;
        self.position_at = now;
        effects.push(Effect::LoadCover(Some(self.queue[self.index].id)));
        effects.push(Effect::LoadQueue {
            tracks: self.queue.clone(),
            index: self.index,
        });
        effects
    }

    /// Starts connecting to a station. `connecting` stays true until
    /// [`Player::station_connection_finished`], which is why a station card
    /// can show a connecting state rather than pretending.
    pub fn play_station(&mut self, station: Station) -> Vec<Effect> {
        let mut effects = self.clear_stream_info();
        self.station = Some(station.clone());
        self.position_seconds = 0.0;
        self.duration_seconds = 0.0;
        self.connecting = true;
        effects.push(Effect::LoadCover(None));
        effects.push(Effect::ConnectStation(station));
        effects
    }

    /// The outcome of the connection `Effect::ConnectStation` asked for:
    /// success once the station is connected and buffered.
    pub fn station_connection_finished(
        &mut self,
        station: &Station,
        outcome: Result<(), String>,
    ) -> Vec<Effect> {
        self.connecting = false;
        match outcome {
            Ok(()) => Vec::new(),
            Err(error) => {
                log::error!("radio play failed: {error}");
                self.station = None;
                vec![Effect::NotifyError {
                    key: "error.radio",
                    parameters: vec![("name", station.name.clone())],
                    dedupe_key: None,
                }]
            }
        }
    }

    pub fn toggle(&mut self, now: Instant) -> Vec<Effect> {
        if self.mode == Mode::Idle {
            return Vec::new();
        }
        self.playing = !self.playing;
        self.position_at = now;
        vec![Effect::Toggle]
    }

    pub fn next(&self) -> Vec<Effect> {
        vec![Effect::Next]
    }

    pub fn previous(&self) -> Vec<Effect> {
        vec![Effect::Previous]
    }

    /// Plays a specific position in the current queue.
    pub fn jump_to(&self, index: usize) -> Vec<Effect> {
        vec![Effect::JumpTo(index)]
    }

    /// Seeks to a fraction of the current local track.
    pub fn seek_to(&mut self, fraction: f64, now: Instant) -> Vec<Effect> {
        if self.mode != Mode::Local || self.duration_seconds <= 0.0 || fraction.is_nan() {
            return Vec::new();
        }
        self.position_seconds = fraction.clamp(0.0, 1.0) * self.duration_seconds;
        self.position_at = now;
        vec![Effect::Seek(self.position_seconds)]
    }

    pub fn set_volume(&mut self, volume: f64) -> Vec<Effect> {
        self.volume = volume.clamp(0.0, 1.0);
        vec![
            Effect::SetVolume(self.volume),
            Effect::PersistVolumeDebounced(self.volume),
        ]
    }

    pub fn toggle_shuffle(&mut self) -> Vec<Effect> {
        self.shuffle = !self.shuffle;
        vec![Effect::SetShuffle(self.shuffle)]
    }

    pub fn toggle_repeat(&mut self) -> Vec<Effect> {
        self.repeat = !self.repeat;
        vec![Effect::SetRepeat(self.repeat)]
    }

    fn clear_stream_info(&mut self) -> Vec<Effect> {
        self.stream = StreamInfo::default();
        vec![Effect::ShowStreamCover(None)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::test_support::track;
    use std::time::Duration;

    fn station(id: &str) -> Station {
        Station {
            id: id.to_owned(),
            name: format!("Station {id}"),
            country: "FR".to_owned(),
            genre_key: "radio.genre.jazz".to_owned(),
            kbps: 128,
            url: format!("https://example.invalid/{id}"),
            gradient_index: 0,
            now_playing: None,
        }
    }

    fn no_stations(_: &str) -> Option<Station> {
        None
    }

    fn state(mode: Mode, playing: bool, index: usize, station_id: Option<&str>) -> EngineEvent {
        EngineEvent::State {
            playing,
            mode,
            index,
            queue_len: 0,
            shuffle: false,
            repeat: false,
            station_id: station_id.map(str::to_owned),
        }
    }

    fn timed_track(id: i64, seconds: f64) -> Track {
        let mut track = track(id, "t");
        track.duration_secs = seconds;
        track
    }

    #[test]
    fn playing_a_queue_shows_it_at_once_and_loads_it_into_the_engine() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let tracks = vec![timed_track(1, 100.0), timed_track(2, 200.0)];

        // An index past the end lands on the last track.
        let effects = player.play_queue(tracks.clone(), 9, now);

        assert_eq!(player.mode(), Mode::Local);
        assert!(player.playing());
        assert_eq!(player.current().map(|track| track.id), Some(2));
        assert_eq!(player.duration_seconds(), 200.0);
        assert_eq!(
            effects,
            [
                Effect::ShowStreamCover(None),
                Effect::LoadCover(Some(2)),
                Effect::LoadQueue { tracks, index: 1 },
            ]
        );
    }

    #[test]
    fn an_empty_queue_changes_nothing() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(player.play_queue(Vec::new(), 0, now).is_empty());
        assert_eq!(player.mode(), Mode::Idle);
    }

    #[test]
    fn a_track_change_moves_the_index_and_asks_for_the_new_cover() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_queue(vec![timed_track(1, 10.0), timed_track(2, 10.0)], 0, now);

        let effects = player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);

        assert_eq!(player.current().map(|track| track.id), Some(2));
        assert_eq!(effects, [Effect::LoadCover(Some(2))]);
    }

    #[test]
    fn progress_is_interpolated_while_playing_and_frozen_while_paused() {
        let start = Instant::now();
        let mut player = Player::new(start);
        player.play_queue(vec![timed_track(1, 100.0)], 0, start);
        player.apply(
            EngineEvent::Position {
                position_secs: 10.0,
                duration_secs: 100.0,
            },
            start,
            &no_stations,
        );

        let later = start + Duration::from_secs(5);
        assert!((player.live_progress(later) - 0.15).abs() < 1e-9);
        // Never past the end, however stale the last position is.
        assert_eq!(player.live_progress(start + Duration::from_secs(500)), 1.0);

        player.toggle(later);
        assert!((player.live_progress(later + Duration::from_secs(5)) - 0.10).abs() < 1e-9);
    }

    #[test]
    fn toggling_does_nothing_while_idle() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(player.toggle(now).is_empty());
        assert!(!player.playing());
    }

    #[test]
    fn seeking_clamps_to_the_track_and_is_refused_outside_local_playback() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(player.seek_to(0.5, now).is_empty());

        player.play_queue(vec![timed_track(1, 200.0)], 0, now);
        assert_eq!(player.seek_to(0.25, now), [Effect::Seek(50.0)]);
        assert_eq!(player.seek_to(7.0, now), [Effect::Seek(200.0)]);
        assert_eq!(player.seek_to(-1.0, now), [Effect::Seek(0.0)]);
    }

    #[test]
    fn volume_is_clamped_applied_at_once_and_saved_once_it_settles() {
        let mut player = Player::new(Instant::now());
        assert_eq!(
            player.set_volume(1.7),
            [Effect::SetVolume(1.0), Effect::PersistVolumeDebounced(1.0)]
        );
        // Boot hydration applies without writing back what it just read.
        assert_eq!(player.restore_volume(-0.2), [Effect::SetVolume(0.0)]);
        assert_eq!(player.volume(), 0.0);
    }

    #[test]
    fn a_station_connects_then_plays_or_reports_its_failure() {
        let mut player = Player::new(Instant::now());
        let jazz = station("jazz");

        let effects = player.play_station(jazz.clone());
        assert!(player.connecting());
        assert_eq!(player.station(), Some(&jazz));
        assert_eq!(effects.last(), Some(&Effect::ConnectStation(jazz.clone())));

        assert!(player.station_connection_finished(&jazz, Ok(())).is_empty());
        assert!(!player.connecting());
        assert_eq!(player.station(), Some(&jazz));

        player.play_station(jazz.clone());
        let effects = player.station_connection_finished(&jazz, Err("refused".to_owned()));
        assert_eq!(player.station(), None);
        assert_eq!(
            effects,
            [Effect::NotifyError {
                key: "error.radio",
                parameters: vec![("name", "Station jazz".to_owned())],
                dedupe_key: None,
            }]
        );
    }

    #[test]
    fn the_station_is_recovered_from_the_engine_after_a_local_state_cleared_it() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let jazz = station("jazz");
        player.play_station(jazz.clone());

        // The previous local playback reports once more while the station connects…
        player.apply(state(Mode::Local, true, 0, None), now, &no_stations);
        assert_eq!(player.station(), None);

        // …then the engine names the station, and the mirror finds it again.
        let lookup = |id: &str| (id == "jazz").then(|| jazz.clone());
        player.apply(state(Mode::Radio, true, 0, Some("jazz")), now, &lookup);

        assert_eq!(player.station(), Some(&jazz));
        assert_eq!(player.mode(), Mode::Radio);
        assert_eq!(player.current(), None);
    }

    #[test]
    fn stream_metadata_is_shown_until_playback_leaves_radio() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_station(station("jazz"));

        let effects = player.apply(
            EngineEvent::StreamMetadata {
                title: Some("So What".to_owned()),
                artist: Some("Miles Davis".to_owned()),
                album: None,
                cover: Some("data:image/png;base64,AAAA".to_owned()),
            },
            now,
            &no_stations,
        );
        assert_eq!(player.stream().title.as_deref(), Some("So What"));
        assert_eq!(
            effects,
            [Effect::ShowStreamCover(Some(
                "data:image/png;base64,AAAA".to_owned()
            ))]
        );

        let effects = player.apply(state(Mode::Idle, false, 0, None), now, &no_stations);
        assert_eq!(player.stream(), &StreamInfo::default());
        assert_eq!(effects, [Effect::ShowStreamCover(None)]);
    }

    #[test]
    fn engine_reports_reach_the_settings_readout_the_equalizer_and_the_user() {
        let now = Instant::now();
        let mut player = Player::new(now);

        player.apply(
            EngineEvent::Device {
                name: "Speakers".to_owned(),
                sample_rate: 48_000,
                channels: 2,
            },
            now,
            &no_stations,
        );
        assert_eq!(player.device_name(), Some("Speakers"));
        assert_eq!(player.sample_rate(), Some(48_000));

        assert_eq!(
            player.apply(
                EngineEvent::FirEq {
                    enabled: true,
                    latency_secs: 0.043,
                },
                now,
                &no_stations,
            ),
            [Effect::LinearPhaseReported {
                enabled: true,
                latency_seconds: 0.043,
            }]
        );

        assert_eq!(
            player.apply(
                EngineEvent::Error {
                    message: "no device".to_owned(),
                },
                now,
                &no_stations,
            ),
            [Effect::NotifyError {
                key: "error.playback",
                parameters: vec![("message", "no device".to_owned())],
                dedupe_key: None,
            }]
        );
    }

    #[test]
    fn shuffle_and_repeat_flip_at_once() {
        let mut player = Player::new(Instant::now());
        assert_eq!(player.toggle_shuffle(), [Effect::SetShuffle(true)]);
        assert_eq!(player.toggle_repeat(), [Effect::SetRepeat(true)]);
        assert_eq!(player.toggle_shuffle(), [Effect::SetShuffle(false)]);
        assert!(player.repeat());
    }
}
