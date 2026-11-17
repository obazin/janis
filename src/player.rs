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

/// What the Now Playing lists depend on; see [`Player::lists_basis`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListsBasis {
    mode: Mode,
    index: Option<usize>,
    shuffle: bool,
}

pub struct Player {
    queue: Vec<Track>,
    index: usize,
    playing: bool,
    /// A local stop not requested by the listener means Play replays the track.
    replay_on_resume: bool,
    /// A stopped source is reopened through the same queue/station decisions.
    stopped_queue_index: Option<usize>,
    stopped_station: Option<Station>,
    /// Stop's Idle acknowledgement supplies the final native queue selection.
    stop_pending: bool,
    play_after_stop: bool,
    /// A buffered station still awaits native playback or an explicit failure.
    pending_station: Option<Station>,
    navigation_pending: bool,
    seek_serial: u64,
    /// Navigation must not turn a paused queue into audible playback.
    navigation_paused: bool,
    mode: Mode,
    stream: StreamInfo,
    connecting: bool,
    /// Bumped each time the listener starts something else, so the outcome
    /// of a connection they have moved on from is recognised and dropped.
    connection_epoch: u64,
    station: Option<Station>,
    volume: f64,
    shuffle: bool,
    repeat: bool,
    queue_has_next: bool,
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
            replay_on_resume: false,
            stopped_queue_index: None,
            stopped_station: None,
            stop_pending: false,
            play_after_stop: false,
            pending_station: None,
            navigation_pending: false,
            seek_serial: 0,
            navigation_paused: false,
            mode: Mode::Idle,
            stream: StreamInfo::default(),
            connecting: false,
            connection_epoch: 0,
            station: None,
            volume: 0.8,
            shuffle: false,
            repeat: false,
            queue_has_next: false,
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

    pub fn stopped(&self) -> bool {
        self.mode == Mode::Idle || self.replay_on_resume
    }

    pub fn can_play(&self) -> bool {
        self.current().is_some()
            || self.station.is_some()
            || self.stopped_queue_index.is_some()
            || self.pending_station.is_some()
            || self.stopped_station.is_some()
    }

    pub fn can_seek(&self) -> bool {
        !self.navigation_pending
            && !self.stopped()
            && self.current().is_some()
            && self.duration_seconds > 0.0
    }

    pub fn can_go_next(&self) -> bool {
        !self.stopped()
            && self.current().is_some()
            && (self.repeat
                || if self.shuffle {
                    // Only the engine knows the shuffled playback cursor.
                    self.queue_has_next
                } else {
                    self.index + 1 < self.queue.len()
                })
    }

    pub fn can_go_previous(&self) -> bool {
        // The engine's Previous wraps even when playlist repeat is off.
        !self.stopped() && self.current().is_some()
    }

    pub fn seek_serial(&self) -> u64 {
        self.seek_serial
    }

    pub fn live_position(&self, now: Instant) -> f64 {
        if self.mode != Mode::Local {
            return 0.0;
        }
        let elapsed = if self.playing {
            now.saturating_duration_since(self.position_at)
                .as_secs_f64()
        } else {
            0.0
        };
        let position = self.position_seconds + elapsed;
        if self.duration_seconds > 0.0 {
            position.min(self.duration_seconds)
        } else {
            position
        }
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

    /// What the Now Playing lists are built from: they change with the
    /// current track and the shuffle switch, and nothing else the engine
    /// reports — position reports ten times a second among it.
    pub fn lists_basis(&self) -> ListsBasis {
        ListsBasis {
            mode: self.mode,
            index: self.current_index(),
            shuffle: self.shuffle,
        }
    }

    /// Where the current local track sits in the queue.
    pub fn current_index(&self) -> Option<usize> {
        self.current().map(|_| self.index)
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
        if !volume.is_nan() {
            self.volume = volume.clamp(0.0, 1.0);
        }
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
                has_next,
            } => {
                self.shuffle = shuffle;
                self.repeat = repeat;
                self.queue_has_next = has_next;
                if self.stop_pending {
                    if self.stopped_queue_index.is_some()
                        && matches!(mode, Mode::Local | Mode::Idle)
                        && index < self.queue.len()
                    {
                        self.stopped_queue_index = Some(index);
                    }
                    if mode == Mode::Idle {
                        self.stop_pending = false;
                        if std::mem::take(&mut self.play_after_stop) {
                            return self.play(now);
                        }
                    }
                    return Vec::new();
                }
                let playing = playing && !(self.navigation_paused && mode == Mode::Local);
                if mode != self.mode || index != self.index {
                    self.navigation_pending = false;
                }
                if playing || mode != Mode::Local {
                    self.replay_on_resume = false;
                } else if self.mode == Mode::Local && self.playing {
                    self.replay_on_resume = true;
                }
                self.mode = mode;
                self.index = index;
                self.playing = playing;
                if mode == Mode::Radio {
                    if self
                        .pending_station
                        .as_ref()
                        .is_some_and(|station| Some(station.id.as_str()) == station_id.as_deref())
                    {
                        self.pending_station = None;
                    }
                    self.stopped_station = None;
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
                if self.stop_pending {
                    return Vec::new();
                }
                self.position_seconds = position_secs;
                self.duration_seconds = duration_secs;
                self.position_at = now;
                Vec::new()
            }
            EngineEvent::Seeked {
                track_id,
                position_secs,
            } => {
                if self.mode == Mode::Local
                    && self.current().is_some_and(|track| track.id == track_id)
                {
                    self.position_seconds = position_secs;
                    self.position_at = now;
                    self.navigation_pending = false;
                    self.seek_serial = self.seek_serial.wrapping_add(1);
                }
                Vec::new()
            }
            EngineEvent::StreamMetadata {
                title,
                artist,
                album,
                cover,
            } => {
                if self.stop_pending {
                    return Vec::new();
                }
                self.stream = StreamInfo {
                    title,
                    artist,
                    album,
                };
                vec![Effect::ShowStreamCover(cover)]
            }
            EngineEvent::TrackChanged { index } => {
                if self.stop_pending {
                    if self.stopped_queue_index.is_some() && index < self.queue.len() {
                        self.stopped_queue_index = Some(index);
                    }
                    return Vec::new();
                }
                self.navigation_pending = false;
                // A delayed audible boundary does not supersede a terminal
                // State: only native playback confirmation clears Stopped.
                self.index = index;
                // The new track starts from its beginning; the engine's next
                // position report confirms its exact length.
                if let Some(track) = self.queue.get(index) {
                    self.position_seconds = 0.0;
                    self.duration_seconds = track.duration_secs;
                    self.position_at = now;
                }
                let track_id = self.queue.get(index).map(|track| track.id);
                vec![Effect::LoadCover(track_id), Effect::LoadOverview(track_id)]
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
                self.navigation_pending = false;
                if let Some(station) = self.pending_station.take() {
                    self.stopped_station = Some(station);
                    self.station = None;
                    self.connecting = false;
                }
                log::error!("audio engine: {message}");
                // When a source goes away — a server, a drive — the engine
                // fails each queued track in turn; the first toast says it.
                vec![Effect::NotifyError {
                    key: "error.playback",
                    parameters: vec![("message", message)],
                    dedupe_key: Some("playback"),
                }]
            }
            // The queue replay serves a host whose mirror can restart while
            // the engine keeps playing. This mirror lives exactly as long as
            // the engine, and `play_queue` already holds the complete queue.
            EngineEvent::Queue { .. } | EngineEvent::Format { .. } => Vec::new(),
            // Janis sets no A–B loop, so the engine only ever reports none.
            EngineEvent::Loop { .. } => Vec::new(),
        }
    }

    /// Replaces the queue and starts at `index`.
    pub fn play_queue(&mut self, tracks: Vec<Track>, index: usize, now: Instant) -> Vec<Effect> {
        if tracks.is_empty() {
            return Vec::new();
        }
        let mut effects = self.clear_stream_info();
        self.stopped_queue_index = None;
        self.stopped_station = None;
        self.stop_pending = false;
        self.play_after_stop = false;
        self.pending_station = None;
        self.navigation_pending = false;
        self.navigation_paused = false;
        self.replay_on_resume = false;
        self.station = None;
        // A station still connecting is abandoned: its outcome is not awaited.
        self.connecting = false;
        self.connection_epoch += 1;
        self.index = index.min(tracks.len() - 1);
        self.queue = tracks;
        // Loading rebuilds shuffled order with the selected track first.
        self.queue_has_next = self.queue.len() > 1;
        self.mode = Mode::Local;
        self.playing = true;
        self.position_seconds = 0.0;
        self.duration_seconds = self.queue[self.index].duration_secs;
        self.position_at = now;
        let track_id = self.queue[self.index].id;
        effects.push(Effect::LoadCover(Some(track_id)));
        effects.push(Effect::LoadOverview(Some(track_id)));
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
        self.stopped_queue_index = None;
        self.stopped_station = None;
        self.stop_pending = false;
        self.play_after_stop = false;
        self.pending_station = Some(station.clone());
        self.navigation_pending = false;
        self.navigation_paused = false;
        self.replay_on_resume = false;
        self.station = Some(station.clone());
        self.position_seconds = 0.0;
        self.duration_seconds = 0.0;
        self.connecting = true;
        self.connection_epoch += 1;
        effects.push(Effect::LoadCover(None));
        effects.push(Effect::LoadOverview(None));
        effects.push(Effect::ConnectStation {
            station,
            epoch: self.connection_epoch,
        });
        effects
    }

    /// The outcome of the connection `Effect::ConnectStation` asked for:
    /// success once the station is connected and buffered. `epoch` is the one
    /// the effect carried; an outcome from a connection the listener has
    /// moved on from changes nothing.
    pub fn station_connection_finished(
        &mut self,
        station: &Station,
        epoch: u64,
        outcome: Result<(), String>,
    ) -> Vec<Effect> {
        if epoch != self.connection_epoch {
            log::debug!("an abandoned connection to {} finished", station.name);
            return Vec::new();
        }
        self.connecting = false;
        match outcome {
            Ok(()) => Vec::new(),
            Err(error) => {
                log::error!("radio play failed: {error}");
                self.stopped_station = self.pending_station.take();
                self.station = None;
                vec![Effect::NotifyError {
                    key: "error.radio",
                    parameters: vec![("name", station.name.clone())],
                    dedupe_key: None,
                }]
            }
        }
    }

    /// Explicit transport requests are idempotent, unlike a button toggle.
    pub fn play(&mut self, now: Instant) -> Vec<Effect> {
        if self.stop_pending {
            self.play_after_stop = true;
            return Vec::new();
        }
        if self.playing || self.connecting || self.pending_station.is_some() {
            return Vec::new();
        }
        if let Some(index) = self.stopped_queue_index.take() {
            let tracks = std::mem::take(&mut self.queue);
            return self.play_queue(tracks, index, now);
        }
        if let Some(station) = self.stopped_station.take() {
            return self.play_station(station);
        }
        if self.mode == Mode::Idle {
            return Vec::new();
        }
        self.navigation_paused = false;
        if self.replay_on_resume {
            self.position_seconds = 0.0;
            self.replay_on_resume = false;
        }
        self.playing = true;
        self.position_at = now;
        vec![Effect::Play]
    }

    pub fn pause(&mut self, now: Instant) -> Vec<Effect> {
        self.play_after_stop = false;
        if !self.playing {
            return Vec::new();
        }
        let mut effects = self.toggle(now);
        for effect in &mut effects {
            if *effect == Effect::Toggle {
                *effect = Effect::Pause;
            }
        }
        effects
    }

    pub fn stop(&mut self, now: Instant) -> Vec<Effect> {
        self.play_after_stop = false;
        if self.stop_pending
            || (self.mode == Mode::Idle
                && !self.connecting
                && self.station.is_none()
                && self.pending_station.is_none())
        {
            return Vec::new();
        }
        self.stop_pending = true;
        self.stopped_station = self.station.take().or(self.pending_station.take());
        self.stopped_queue_index = if self.stopped_station.is_none() {
            self.current_index()
        } else {
            None
        };
        self.connection_epoch += 1;
        self.connecting = false;
        self.navigation_pending = false;
        self.navigation_paused = false;
        self.replay_on_resume = false;
        self.mode = Mode::Idle;
        self.playing = false;
        self.position_seconds = 0.0;
        self.duration_seconds = 0.0;
        self.position_at = now;
        let mut effects = self.clear_stream_info();
        effects.extend([
            Effect::Stop,
            Effect::LoadCover(None),
            Effect::LoadOverview(None),
        ]);
        effects
    }

    pub fn toggle(&mut self, now: Instant) -> Vec<Effect> {
        if self.stop_pending {
            self.play_after_stop = !self.play_after_stop;
            return Vec::new();
        }
        if self.mode == Mode::Idle {
            return self.play(now);
        }
        self.navigation_paused = false;
        if !self.playing && self.replay_on_resume {
            self.position_seconds = 0.0;
            self.replay_on_resume = false;
        }
        if self.playing && self.mode == Mode::Local {
            // The playhead stays where it was last drawn, not at the last
            // position the engine reported, up to a tenth of a second back.
            let elapsed = now
                .saturating_duration_since(self.position_at)
                .as_secs_f64();
            self.position_seconds = (self.position_seconds + elapsed).min(self.duration_seconds);
        }
        self.playing = !self.playing;
        self.position_at = now;
        vec![Effect::Toggle]
    }

    pub fn next(&mut self) -> Vec<Effect> {
        if !self.can_go_next() {
            return Vec::new();
        }
        self.navigation_pending = true;
        if self.playing {
            vec![Effect::Next]
        } else {
            self.navigation_paused = true;
            vec![Effect::Next, Effect::Pause]
        }
    }

    pub fn previous(&mut self) -> Vec<Effect> {
        if !self.can_go_previous() {
            return Vec::new();
        }
        self.navigation_pending = true;
        if self.playing {
            vec![Effect::Previous]
        } else {
            self.navigation_paused = true;
            vec![Effect::Previous, Effect::Pause]
        }
    }

    /// Plays a specific position in the current queue.
    pub fn jump_to(&mut self, index: usize) -> Vec<Effect> {
        if self.mode != Mode::Local || index >= self.queue.len() {
            return Vec::new();
        }
        self.navigation_paused = false;
        self.navigation_pending = true;
        self.replay_on_resume = false;
        vec![Effect::JumpTo(index)]
    }

    /// Seeks to a fraction of the current local track.
    pub fn seek_to(&mut self, fraction: f64, now: Instant) -> Vec<Effect> {
        if !self.can_seek() || fraction.is_nan() {
            return Vec::new();
        }
        let track_id = self
            .current()
            .expect("a seekable source has a current track")
            .id;
        self.replay_on_resume = false;
        self.position_seconds = fraction.clamp(0.0, 1.0) * self.duration_seconds;
        self.position_at = now;
        vec![Effect::Seek {
            track_id,
            position_seconds: self.position_seconds,
        }]
    }

    /// Absolute desktop positions outside the current track are ignored.
    pub fn seek_position(&mut self, position_seconds: f64, now: Instant) -> Vec<Effect> {
        if !self.can_seek()
            || !position_seconds.is_finite()
            || position_seconds < 0.0
            || position_seconds > self.duration_seconds
        {
            return Vec::new();
        }
        self.seek_to(position_seconds / self.duration_seconds, now)
    }

    /// Relative seeks use the interpolated anchor, including while paused.
    pub fn seek_relative(&mut self, offset_seconds: f64, now: Instant) -> Vec<Effect> {
        if !self.can_seek() || !offset_seconds.is_finite() {
            return Vec::new();
        }
        let position = self.live_position(now) + offset_seconds;
        if position > self.duration_seconds {
            return self.next();
        }
        self.seek_position(position.max(0.0), now)
    }

    pub fn seek_track(
        &mut self,
        track_id: i64,
        position_seconds: f64,
        now: Instant,
    ) -> Vec<Effect> {
        if self.current().is_none_or(|track| track.id != track_id) {
            return Vec::new();
        }
        self.seek_position(position_seconds, now)
    }

    pub fn set_volume(&mut self, volume: f64) -> Vec<Effect> {
        if volume.is_nan() {
            return Vec::new();
        }
        self.volume = volume.clamp(0.0, 1.0);
        vec![
            Effect::SetVolume(self.volume),
            Effect::PersistVolumeDebounced(self.volume),
        ]
    }

    pub fn toggle_shuffle(&mut self) -> Vec<Effect> {
        self.set_shuffle(!self.shuffle)
    }

    pub fn set_shuffle(&mut self, enabled: bool) -> Vec<Effect> {
        if enabled && !self.shuffle {
            self.queue_has_next = self.queue.len() > 1;
        }
        self.shuffle = enabled;
        vec![Effect::SetShuffle(self.shuffle)]
    }

    pub fn toggle_repeat(&mut self) -> Vec<Effect> {
        self.set_repeat(!self.repeat)
    }

    pub fn set_repeat(&mut self, enabled: bool) -> Vec<Effect> {
        self.repeat = enabled;
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

    #[test]
    fn explicit_play_and_pause_are_idempotent_and_empty_play_stays_idle() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(player.play(now).is_empty());
        assert!(player.pause(now).is_empty());
        assert!(!player.playing());
        player.play_queue(vec![timed_track(1, 100.0)], 0, now);
        assert!(player.play(now).is_empty());
        assert_eq!(player.pause(now), [Effect::Pause]);
        assert!(player.pause(now).is_empty());
        assert_eq!(player.play(now), [Effect::Play]);
        assert!(player.play(now).is_empty());
    }

    #[test]
    fn paused_navigation_keeps_the_transport_paused_through_engine_confirmations() {
        for forward in [true, false] {
            let now = Instant::now();
            let mut player = playing(
                vec![timed_track(1, 100.0), timed_track(2, 100.0)],
                20.0,
                now,
            );
            player.pause(now);
            let effects = if forward {
                player.next()
            } else {
                player.previous()
            };
            assert_eq!(
                effects,
                [
                    if forward {
                        Effect::Next
                    } else {
                        Effect::Previous
                    },
                    Effect::Pause
                ]
            );
            player.apply(state(Mode::Local, true, 1, None), now, &no_stations);
            assert!(
                !player.playing(),
                "an intermediate navigation state stays paused"
            );
            player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);
            player.apply(state(Mode::Local, false, 1, None), now, &no_stations);
            assert!(!player.playing());
            assert!(
                !player.stopped(),
                "navigation preserves Paused rather than Stopped"
            );
            assert_eq!(player.play(now), [Effect::Play]);
            player.apply(state(Mode::Local, true, 1, None), now, &no_stations);
            assert!(player.playing(), "explicit Play retires the pause intent");
        }
    }

    #[test]
    fn queue_row_playback_retires_a_navigation_pause() {
        let now = Instant::now();
        let mut player = playing(
            vec![timed_track(1, 100.0), timed_track(2, 100.0)],
            20.0,
            now,
        );
        player.pause(now);
        player.next();
        player.apply(state(Mode::Local, false, 1, None), now, &no_stations);
        assert_eq!(player.jump_to(0), [Effect::JumpTo(0)]);
        player.apply(state(Mode::Local, true, 0, None), now, &no_stations);
        assert!(player.playing());
        assert_eq!(player.pause(now), [Effect::Pause]);
    }

    #[test]
    fn navigation_fences_old_absolute_positions_before_engine_confirmation() {
        let now = Instant::now();
        let mut player = playing(
            vec![timed_track(1, 100.0), timed_track(2, 100.0)],
            20.0,
            now,
        );
        assert_eq!(player.next(), [Effect::Next]);
        assert!(!player.can_seek());
        assert!(player.seek_track(1, 50.0, now).is_empty());
        player.apply(state(Mode::Local, true, 1, None), now, &no_stations);
        assert!(player.seek_track(1, 50.0, now).is_empty());
        assert_eq!(
            player.seek_track(2, 50.0, now),
            [Effect::Seek {
                track_id: 2,
                position_seconds: 50.0
            }]
        );
    }

    #[test]
    fn stopped_navigation_and_seeking_are_honestly_unavailable() {
        let now = Instant::now();
        let mut player = playing(
            vec![timed_track(1, 100.0), timed_track(2, 100.0)],
            20.0,
            now,
        );
        player.stop(now);
        assert!(player.can_play());
        assert!(!player.can_seek());
        assert!(!player.can_go_next());
        assert!(!player.can_go_previous());
        assert!(player.next().is_empty());
        assert!(player.previous().is_empty());
        assert!(player.seek_to(0.5, now).is_empty());
        assert!(player.seek_relative(10.0, now).is_empty());
    }

    #[test]
    fn stop_reopens_the_selected_track_at_zero_after_the_idle_confirmation() {
        let now = Instant::now();
        let song = timed_track(7, 100.0);
        let mut player = playing(vec![timed_track(1, 100.0), song.clone()], 40.0, now);
        player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);
        let effects = player.stop(now);
        assert!(effects.contains(&Effect::Stop));
        assert_eq!(player.mode(), Mode::Idle);
        assert!(!player.playing());
        assert_eq!(player.position_seconds(), 0.0);
        assert!(player.can_play());
        assert!(!player.can_seek());
        assert!(player.pause(now).is_empty());
        assert!(player.stop(now).is_empty());
        player.apply(state(Mode::Idle, false, 1, None), now, &no_stations);
        assert!(player.can_play());
        let effects = player.play(now);
        assert!(effects.contains(&Effect::LoadQueue {
            tracks: vec![timed_track(1, 100.0), song],
            index: 1,
        }));
        assert!(!effects.contains(&Effect::Toggle));
        assert_eq!(player.current().map(|track| track.id), Some(7));
        assert_eq!(player.position_seconds(), 0.0);
        assert!(player.playing());
    }

    #[test]
    fn stop_retains_radio_resume_intent_and_retires_a_pending_connection() {
        for connected in [false, true] {
            let now = Instant::now();
            let mut player = playing(vec![timed_track(1, 100.0)], 40.0, now);
            let selected = station("fip");
            player.play_station(selected.clone());
            let abandoned_epoch = player.connection_epoch;
            if connected {
                player.apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
                    Some(selected.clone())
                });
            }
            assert!(player.stop(now).contains(&Effect::Stop));
            player.apply(state(Mode::Idle, false, 0, None), now, &no_stations);
            assert!(player
                .station_connection_finished(&selected, abandoned_epoch, Ok(()))
                .is_empty());
            assert!(!player.playing());
            assert!(player.pause(now).is_empty());
            let effects = player.play(now);
            assert!(effects.iter().any(|effect| matches!(
                effect,
                Effect::ConnectStation { station, .. } if station.id == "fip"
            )));
            assert!(!effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadQueue { .. })));
            assert!(
                player.play(now).is_empty(),
                "a connecting Play is idempotent"
            );
        }
    }

    #[test]
    fn desktop_seeks_use_signed_live_seconds_and_reject_stale_or_invalid_positions() {
        let now = Instant::now();
        let mut player = playing(
            vec![timed_track(1, 100.0), timed_track(2, 100.0)],
            20.0,
            now,
        );
        assert_eq!(
            player.seek_relative(-5.0, now + Duration::from_secs(2)),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 17.0
            }]
        );
        player.pause(now + Duration::from_secs(2));
        assert_eq!(
            player.seek_relative(-100.0, now + Duration::from_secs(10)),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 0.0
            }]
        );
        for target in [-1.0, 100.1, f64::NAN, f64::INFINITY] {
            assert!(player.seek_track(1, target, now).is_empty());
        }
        assert!(player.seek_track(2, 50.0, now).is_empty());
        assert_eq!(
            player.seek_track(1, 50.0, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 50.0
            }]
        );
        assert_eq!(
            player.seek_relative(60.0, now),
            [Effect::Next, Effect::Pause]
        );
        assert!(player.seek_relative(f64::NAN, now).is_empty());
        player.play_station(station("fip"));
        player.apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
            Some(station("fip"))
        });
        assert!(player.seek_relative(10.0, now).is_empty());
        assert!(player.seek_track(1, 10.0, now).is_empty());
        assert!(!player.can_go_next());
        assert!(!player.can_go_previous());
    }

    #[test]
    fn only_engine_confirmations_publish_position_discontinuities() {
        let now = Instant::now();
        let mut player = playing(
            vec![timed_track(1, 100.0), timed_track(2, 100.0)],
            20.0,
            now,
        );
        player.seek_to(0.5, now);
        player.apply(
            EngineEvent::Position {
                position_secs: 20.1,
                duration_secs: 100.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(
            player.seek_serial(),
            0,
            "queued progress is not a seek acknowledgement"
        );
        player.apply(
            EngineEvent::Seeked {
                track_id: 1,
                position_secs: 49.9,
            },
            now,
            &no_stations,
        );
        assert_eq!(player.seek_serial(), 1);
        assert_eq!(player.position_seconds(), 49.9);
        player.apply(
            EngineEvent::Position {
                position_secs: 50.1,
                duration_secs: 100.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(player.seek_serial(), 1);
        player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);
        player.apply(
            EngineEvent::Seeked {
                track_id: 1,
                position_secs: 80.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(
            player.seek_serial(),
            1,
            "a previous track cannot move this playhead"
        );
        assert_eq!(player.position_seconds(), 0.0);
        player.apply(
            EngineEvent::Seeked {
                track_id: 2,
                position_secs: 0.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(
            player.seek_serial(),
            2,
            "same-track restarts notify desktop clients"
        );
        player.stop(now);
        player.apply(
            EngineEvent::Seeked {
                track_id: 2,
                position_secs: 25.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(
            player.seek_serial(),
            2,
            "a stopped source cannot confirm a seek"
        );
    }

    #[test]
    fn queue_capabilities_follow_the_selection_and_playlist_repeat() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(!player.can_play());
        assert!(!player.can_go_next());
        assert!(!player.can_go_previous());
        player.play_queue(vec![timed_track(1, 100.0), timed_track(2, 100.0)], 1, now);
        assert!(!player.can_go_next());
        assert!(player.can_go_previous());
        assert!(player.next().is_empty());
        assert_eq!(player.set_repeat(true), [Effect::SetRepeat(true)]);
        assert!(player.can_go_next());
        player.set_repeat(false);
        assert!(!player.can_go_next());
        assert_eq!(player.set_shuffle(true), [Effect::SetShuffle(true)]);
        assert!(player.can_go_next());
        assert_eq!(player.next(), [Effect::Next]);
        player.apply(
            EngineEvent::State {
                playing: true,
                mode: Mode::Local,
                index: 0,
                queue_len: 2,
                shuffle: true,
                repeat: false,
                station_id: None,
                has_next: false,
            },
            now,
            &no_stations,
        );
        assert!(!player.can_go_next());
        assert!(player.next().is_empty());
    }

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
            has_next: false,
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
        track(id, "t").duration(seconds)
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
                Effect::LoadOverview(Some(2)),
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
    fn a_track_change_starts_the_new_track_and_asks_for_its_cover_and_overview() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_queue(vec![timed_track(1, 300.0), timed_track(2, 120.0)], 0, now);
        player.apply(
            EngineEvent::Position {
                position_secs: 299.0,
                duration_secs: 300.0,
            },
            now,
            &no_stations,
        );

        let effects = player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);

        assert_eq!(player.current().map(|track| track.id), Some(2));
        assert_eq!(player.current_index(), Some(1));
        // The gapless join is drawn at once against the new track's length,
        // not the old one's until the engine next reports a position.
        assert_eq!(player.position_seconds(), 0.0);
        assert_eq!(player.duration_seconds(), 120.0);
        assert_eq!(
            effects,
            [Effect::LoadCover(Some(2)), Effect::LoadOverview(Some(2))]
        );
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
        assert!((player.live_progress(later + Duration::from_secs(5)) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn pausing_freezes_the_playhead_where_it_was_drawn_and_resuming_goes_on_from_there() {
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

        // 80 ms after the last report, between two of them.
        let paused_at = start + Duration::from_millis(80);
        let drawn = player.live_progress(paused_at);
        player.toggle(paused_at);
        assert_eq!(player.live_progress(paused_at), drawn);
        assert_eq!(
            player.live_progress(paused_at + Duration::from_secs(9)),
            drawn
        );

        let resumed_at = paused_at + Duration::from_secs(10);
        player.toggle(resumed_at);
        let one_second_on = player.live_progress(resumed_at + Duration::from_secs(1));
        assert!((one_second_on - (drawn + 0.01)).abs() < 1e-9);
    }

    proptest::proptest! {
        #[test]
        fn the_playhead_never_moves_backwards_between_position_reports(
            position in 0.0f64..100.0,
            steps in proptest::collection::vec((0u64..3_000, proptest::bool::ANY), 1..40),
        ) {
            let start = Instant::now();
            let mut player = Player::new(start);
            player.play_queue(vec![timed_track(1, 100.0)], 0, start);
            player.apply(
                EngineEvent::Position {
                    position_secs: position,
                    duration_secs: 100.0,
                },
                start,
                &no_stations,
            );
            let mut now = start;
            let mut last = player.live_progress(now);
            for (milliseconds, toggles) in steps {
                now += Duration::from_millis(milliseconds);
                if toggles {
                    player.toggle(now);
                }
                let progress = player.live_progress(now);
                proptest::prop_assert!(progress >= last && progress <= 1.0);
                last = progress;
            }
        }
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
        assert_eq!(
            player.seek_to(0.25, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 50.0
            }]
        );
        assert_eq!(
            player.seek_to(7.0, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 200.0
            }]
        );
        assert_eq!(
            player.seek_to(-1.0, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 0.0
            }]
        );
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
        assert_eq!(
            effects.last(),
            Some(&Effect::ConnectStation {
                station: jazz.clone(),
                epoch: 1,
            })
        );

        assert!(player
            .station_connection_finished(&jazz, 1, Ok(()))
            .is_empty());
        assert!(!player.connecting());
        assert_eq!(player.station(), Some(&jazz));

        player.play_station(jazz.clone());
        let effects = player.station_connection_finished(&jazz, 2, Err("refused".to_owned()));
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
    fn the_outcome_of_a_station_the_listener_left_for_another_is_ignored() {
        let mut player = Player::new(Instant::now());
        let (jazz, rock) = (station("jazz"), station("rock"));
        player.play_station(jazz.clone());
        player.play_station(rock.clone());

        // The first station fails after the second was chosen.
        let effects = player.station_connection_finished(&jazz, 1, Err("refused".to_owned()));

        assert!(effects.is_empty());
        assert!(player.connecting());
        assert_eq!(player.station(), Some(&rock));

        assert!(player
            .station_connection_finished(&rock, 2, Ok(()))
            .is_empty());
        assert!(!player.connecting());
        assert_eq!(player.station(), Some(&rock));
    }

    #[test]
    fn playing_a_queue_abandons_a_station_that_was_connecting() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let jazz = station("jazz");
        player.play_station(jazz.clone());

        player.play_queue(vec![timed_track(1, 10.0)], 0, now);
        assert!(!player.connecting());

        let effects = player.station_connection_finished(&jazz, 1, Err("refused".to_owned()));
        assert!(effects.is_empty());
        assert_eq!(player.current().map(|track| track.id), Some(1));
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
                dedupe_key: Some("playback"),
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

    #[test]
    fn a_volume_that_is_not_a_number_is_refused() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.set_volume(0.5);
        assert!(player.set_volume(f64::NAN).is_empty());
        assert_eq!(player.volume(), 0.5);
        assert_eq!(
            player.restore_volume(f64::NAN),
            [Effect::SetVolume(0.5)],
            "the engine keeps the volume it had"
        );
    }

    #[test]
    fn the_now_playing_lists_follow_the_track_and_shuffle_but_not_the_position() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_queue(vec![timed_track(1, 100.0), timed_track(2, 100.0)], 0, now);
        let basis = player.lists_basis();

        player.apply(
            EngineEvent::Position {
                position_secs: 10.0,
                duration_secs: 100.0,
            },
            now,
            &no_stations,
        );
        assert_eq!(player.lists_basis(), basis);

        player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);
        let next = player.lists_basis();
        assert_ne!(next, basis);

        player.toggle_shuffle();
        assert_ne!(player.lists_basis(), next);
    }

    /// A player playing `tracks` from the first, its position reported at
    /// `position` seconds.
    fn playing(tracks: Vec<Track>, position: f64, now: Instant) -> Player {
        let mut player = Player::new(now);
        let duration = tracks[0].duration_secs;
        player.play_queue(tracks, 0, now);
        player.apply(
            EngineEvent::Position {
                position_secs: position,
                duration_secs: duration,
            },
            now,
            &no_stations,
        );
        player
    }

    #[test]
    fn a_seek_while_paused_moves_the_playhead_at_once_and_it_stays_there() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 200.0)], 20.0, now);
        player.toggle(now);

        assert_eq!(
            player.seek_to(0.5, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 100.0
            }]
        );

        assert_eq!(player.live_progress(now), 0.5);
        assert_eq!(player.live_progress(now + Duration::from_secs(30)), 0.5);
    }

    #[test]
    fn progress_is_zero_for_a_station_nothing_or_a_track_of_unknown_length() {
        let now = Instant::now();
        assert_eq!(Player::new(now).live_progress(now), 0.0);
        let unknown = playing(vec![timed_track(1, 0.0)], 0.0, now);
        assert_eq!(unknown.live_progress(now + Duration::from_secs(5)), 0.0);
        let mut radio = Player::new(now);
        radio.play_station(station("fip"));
        radio.apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
            Some(station("fip"))
        });
        assert_eq!(radio.live_progress(now + Duration::from_secs(5)), 0.0);
    }

    #[test]
    fn seeking_is_refused_without_a_length_or_a_fraction() {
        let now = Instant::now();
        let mut unknown = playing(vec![timed_track(1, 0.0)], 0.0, now);
        assert!(unknown.seek_to(0.5, now).is_empty());
        let mut known = playing(vec![timed_track(1, 100.0)], 0.0, now);
        assert!(known.seek_to(f64::NAN, now).is_empty());
        assert_eq!(
            known.seek_to(7.0, now),
            [Effect::Seek {
                track_id: 1,
                position_seconds: 100.0
            }],
            "held to the end"
        );
    }

    #[test]
    fn the_engine_state_corrects_the_shuffle_and_repeat_switches() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 100.0)], 0.0, now);
        player.toggle_shuffle();
        assert!(player.shuffle());
        player.apply(
            EngineEvent::State {
                has_next: false,
                playing: true,
                mode: Mode::Local,
                index: 0,
                queue_len: 1,
                shuffle: false,
                repeat: true,
                station_id: None,
            },
            now,
            &no_stations,
        );
        assert!(!player.shuffle());
        assert!(player.repeat());
    }

    #[test]
    fn a_radio_state_naming_the_station_shown_keeps_it_and_one_naming_none_clears_it() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_station(station("fip"));
        let looked_up = std::cell::Cell::new(0);
        let lookup = |id: &str| {
            looked_up.set(looked_up.get() + 1);
            Some(station(id))
        };
        player.apply(state(Mode::Radio, true, 0, Some("fip")), now, &lookup);
        assert_eq!(looked_up.get(), 0, "the station shown is kept as it is");
        assert_eq!(
            player.station().map(|station| station.id.as_str()),
            Some("fip")
        );

        player.apply(state(Mode::Radio, true, 0, None), now, &lookup);
        assert_eq!(player.station(), None);
    }

    #[test]
    fn a_track_change_beyond_the_queue_clears_the_cover_and_the_overview() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 100.0)], 30.0, now);
        assert_eq!(
            player.apply(EngineEvent::TrackChanged { index: 5 }, now, &no_stations),
            [Effect::LoadCover(None), Effect::LoadOverview(None)]
        );
        assert_eq!(player.position_seconds(), 30.0, "the position stays");
    }

    #[test]
    fn playing_a_queue_over_a_station_clears_the_station_and_what_it_announced() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_station(station("fip"));
        player.apply(
            EngineEvent::StreamMetadata {
                title: Some("Song".to_owned()),
                artist: None,
                album: None,
                cover: None,
            },
            now,
            &no_stations,
        );
        let tracks = vec![timed_track(1, 100.0)];

        let effects = player.play_queue(tracks.clone(), 0, now);

        assert_eq!(
            effects,
            [
                Effect::ShowStreamCover(None),
                Effect::LoadCover(Some(1)),
                Effect::LoadOverview(Some(1)),
                Effect::LoadQueue { tracks, index: 0 },
            ]
        );
        assert_eq!(player.station(), None);
        assert_eq!(player.stream().title, None);
        assert!(!player.connecting());
    }

    #[test]
    fn a_station_connects_with_the_previous_art_and_overview_cleared() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 100.0)], 30.0, now);
        let fip = station("fip");

        let effects = player.play_station(fip.clone());

        assert_eq!(
            effects,
            [
                Effect::ShowStreamCover(None),
                Effect::LoadCover(None),
                Effect::LoadOverview(None),
                Effect::ConnectStation {
                    station: fip,
                    epoch: 2,
                },
            ]
        );
        assert!(player.connecting());
        assert_eq!(player.position_seconds(), 0.0);
    }

    #[test]
    fn a_station_pauses_and_resumes() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.play_station(station("fip"));
        player.apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
            Some(station("fip"))
        });
        assert_eq!(player.toggle(now), [Effect::Toggle]);
        assert!(!player.playing());
        assert_eq!(player.toggle(now), [Effect::Toggle]);
        assert!(player.playing());
    }

    #[test]
    fn the_transport_buttons_reach_the_engine() {
        let now = Instant::now();
        let mut player = Player::new(now);
        assert!(player.next().is_empty());
        assert!(player.previous().is_empty());
        player.play_queue(vec![timed_track(1, 100.0), timed_track(2, 100.0)], 0, now);
        assert_eq!(player.next(), [Effect::Next]);
        assert_eq!(player.previous(), [Effect::Previous]);
        assert_eq!(player.jump_to(1), [Effect::JumpTo(1)]);
    }

    #[test]
    fn an_engine_finished_queue_replays_from_zero_and_retires_the_stopped_state() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 100.0)], 99.9, now);
        player.apply(state(Mode::Local, false, 0, None), now, &no_stations);
        assert!(player.stopped());
        assert!(!player.can_seek());
        assert_eq!(player.play(now + Duration::from_secs(60)), [Effect::Play]);
        assert_eq!(player.position_seconds(), 0.0);
        assert!(player.playing());
        assert!(!player.stopped());
        assert!(player.play(now).is_empty());
    }

    #[test]
    fn a_new_queue_clears_a_finished_tracks_replay_intent() {
        let now = Instant::now();
        let mut player = playing(vec![timed_track(1, 100.0)], 99.9, now);
        player.apply(state(Mode::Local, false, 0, None), now, &no_stations);
        player.play_queue(vec![timed_track(2, 100.0)], 0, now);
        assert!(!player.stopped());
        assert!(player.can_seek());
        assert_eq!(player.pause(now), [Effect::Pause]);
        assert_eq!(player.play(now), [Effect::Play]);
    }

    #[test]
    fn a_new_shuffled_queue_has_a_next_track_before_the_engine_reports_its_cursor() {
        let now = Instant::now();
        let mut player = Player::new(now);
        player.set_shuffle(true);
        player.play_queue(vec![timed_track(1, 100.0), timed_track(2, 100.0)], 1, now);
        assert!(player.can_go_next());
        player.apply(
            EngineEvent::State {
                playing: true,
                mode: Mode::Local,
                index: 1,
                queue_len: 2,
                shuffle: true,
                repeat: false,
                station_id: None,
                has_next: false,
            },
            now,
            &no_stations,
        );
        assert!(!player.can_go_next());
    }

    #[test]
    fn buffered_radio_play_is_idempotent_until_native_playback_is_confirmed() {
        for connection_finished_first in [false, true] {
            let now = Instant::now();
            let mut player = Player::new(now);
            let selected = station("fip");
            player.play_station(selected.clone());
            let epoch = player.connection_epoch;
            if connection_finished_first {
                player.station_connection_finished(&selected, epoch, Ok(()));
            }
            assert!(player.play(now).is_empty());
            assert!(player.toggle(now).is_empty());
            assert!(!player.playing());
            player.apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
                Some(selected.clone())
            });
            if !connection_finished_first {
                player.station_connection_finished(&selected, epoch, Ok(()));
            }
            assert!(player.playing());
            assert!(player.play(now).is_empty());
            assert_eq!(player.pause(now), [Effect::Pause]);
        }
    }

    #[test]
    fn a_native_radio_start_failure_waits_for_an_explicit_retry() {
        for connection_finished_first in [false, true] {
            let now = Instant::now();
            let mut player = Player::new(now);
            let selected = station("fip");
            player.play_station(selected.clone());
            let epoch = player.connection_epoch;
            if connection_finished_first {
                player.station_connection_finished(&selected, epoch, Ok(()));
            }
            let effects = player.apply(
                EngineEvent::Error {
                    message: "output unavailable".to_owned(),
                },
                now,
                &no_stations,
            );
            assert!(matches!(effects.as_slice(), [Effect::NotifyError { .. }]));
            if !connection_finished_first {
                assert!(player
                    .station_connection_finished(&selected, epoch, Ok(()))
                    .is_empty());
            }
            assert!(!player.playing());
            assert!(!player.connecting());
            assert!(player.can_play());
            let effects = player.play(now);
            assert!(effects.iter().any(|effect| matches!(
                effect,
                Effect::ConnectStation { station, .. } if station.id == selected.id
            )));
            assert!(player.play(now).is_empty());
        }
    }

    #[test]
    fn a_radio_connection_failure_can_be_retried_explicitly() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let selected = station("fip");
        player.play_station(selected.clone());
        let effects = player.station_connection_finished(
            &selected,
            player.connection_epoch,
            Err("connection refused".to_owned()),
        );
        assert!(matches!(effects.as_slice(), [Effect::NotifyError { .. }]));
        assert!(player.can_play());
        assert!(player.play(now).iter().any(|effect| matches!(
            effect,
            Effect::ConnectStation { station, .. } if station.id == selected.id
        )));
    }

    #[test]
    fn stop_cancels_buffered_radio_before_its_native_state_arrives() {
        let now = Instant::now();
        let mut player = Player::new(now);
        let selected = station("fip");
        player.play_station(selected.clone());
        let epoch = player.connection_epoch;
        player.station_connection_finished(&selected, epoch, Ok(()));
        assert!(!player.connecting());
        assert_eq!(player.mode(), Mode::Idle);
        assert!(player.stop(now).contains(&Effect::Stop));
        assert!(player.stop(now).is_empty());
        assert!(player.play(now).is_empty());
        assert!(player
            .apply(state(Mode::Radio, true, 0, Some("fip")), now, &|_| {
                Some(selected.clone())
            })
            .is_empty());
        assert_eq!(player.mode(), Mode::Idle);
        assert!(!player.playing());
        assert!(player.station().is_none());
        assert!(
            player.stop(now).is_empty(),
            "Stop retires the deferred Play"
        );
        assert!(player
            .apply(state(Mode::Idle, false, 0, None), now, &no_stations)
            .is_empty());
        assert!(!player.playing());
        assert!(player.play(now).iter().any(|effect| matches!(
            effect,
            Effect::ConnectStation { station, epoch: new_epoch }
                if station.id == selected.id && *new_epoch != epoch
        )));
    }

    #[test]
    fn stop_reconciles_navigation_acknowledgements_without_resurrecting_playback() {
        let now = Instant::now();
        for navigation_event in [
            state(Mode::Local, true, 1, None),
            EngineEvent::TrackChanged { index: 1 },
        ] {
            let tracks = vec![timed_track(1, 100.0), timed_track(2, 100.0)];
            let mut player = playing(tracks.clone(), 20.0, now);
            assert_eq!(player.next(), [Effect::Next]);
            assert!(player.stop(now).contains(&Effect::Stop));
            assert!(player.apply(navigation_event, now, &no_stations).is_empty());
            assert_eq!(player.mode(), Mode::Idle);
            assert!(!player.playing());
            assert!(!player.can_seek());
            assert!(player
                .apply(state(Mode::Idle, false, 1, None), now, &no_stations)
                .is_empty());
            assert!(player
                .play(now)
                .contains(&Effect::LoadQueue { tracks, index: 1 }));
            assert_eq!(player.current().map(|track| track.id), Some(2));
        }
    }

    #[test]
    fn play_after_navigation_and_stop_waits_for_the_final_stopped_selection() {
        let now = Instant::now();
        for cancel_replay in [false, true] {
            for navigation_event in [
                Some(state(Mode::Local, true, 1, None)),
                Some(EngineEvent::TrackChanged { index: 1 }),
                None,
            ] {
                let tracks = vec![timed_track(1, 100.0), timed_track(2, 100.0)];
                let mut player = playing(tracks.clone(), 20.0, now);
                player.next();
                player.stop(now);
                assert!(player.play(now).is_empty());
                assert!(player.play(now).is_empty(), "deferred Play is idempotent");
                assert!(player
                    .apply(state(Mode::Local, true, 0, None), now, &no_stations)
                    .is_empty());
                if let Some(event) = navigation_event {
                    assert!(player.apply(event, now, &no_stations).is_empty());
                }
                assert_eq!(player.mode(), Mode::Idle);
                assert!(!player.playing());
                if cancel_replay {
                    assert!(player.stop(now).is_empty());
                }
                let effects = player.apply(state(Mode::Idle, false, 1, None), now, &no_stations);
                if cancel_replay {
                    assert!(effects.is_empty());
                    assert!(!player.playing());
                    assert!(player
                        .play(now)
                        .contains(&Effect::LoadQueue { tracks, index: 1 }));
                } else {
                    assert!(effects.contains(&Effect::LoadQueue { tracks, index: 1 }));
                }
                assert_eq!(player.current().map(|track| track.id), Some(2));
            }
        }
    }

    #[test]
    fn a_delayed_final_track_boundary_cannot_turn_a_finished_queue_into_paused() {
        let now = Instant::now();
        for terminal_index in [0, 1] {
            let mut player = playing(vec![timed_track(1, 100.0), timed_track(2, 0.01)], 99.9, now);
            player.apply(
                state(Mode::Local, false, terminal_index, None),
                now,
                &no_stations,
            );
            assert!(player.stopped());
            player.apply(EngineEvent::TrackChanged { index: 1 }, now, &no_stations);
            assert!(!player.playing());
            assert!(player.stopped());
            assert!(!player.can_seek());
            assert!(!player.can_go_next());
            assert!(!player.can_go_previous());
            player.apply(state(Mode::Local, false, 1, None), now, &no_stations);
            assert!(player.stopped());
            player.apply(state(Mode::Local, true, 1, None), now, &no_stations);
            assert!(!player.stopped());
            assert!(player.can_seek());
        }
    }
}
