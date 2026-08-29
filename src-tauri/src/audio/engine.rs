//! The engine thread: owns the output stream, the decoders and the queue.
//!
//! Everything that is `!Sync` or expensive to rebuild lives here, reached only
//! by [`EngineCommand`]. The thread loop is deliberately simple — drain
//! commands, top the ring up, emit whatever the UI is owed, sleep if there was
//! nothing to do — because the hard realtime work happens in the cpal callback
//! and the hard correctness work lives in [`super::queue`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};

use super::analyser::{Analyser, FRAME_BYTES};
use super::decode::Decoder;
use super::dsp::remap_channels;
use super::events::{EngineEvent, Mode};
use super::icy;
use super::loudness::{self, Loudness, Store};
use super::nowplaying::Update;
use super::output::{self, Output};
use super::params::Params;
use super::queue::{Queue, QueueEntry};
use super::resample::Resampler;
use super::stream::{NowPlaying, RadioStream};
use super::Subscribers;

/// Half a second of stereo audio at 48 kHz. Deep enough to ride out a disk
/// stall, shallow enough that a seek throws away little work.
const RING_SECONDS: f32 = 0.5;
/// Frames pulled from the decoder per pump. Small enough to stay responsive
/// to a seek, large enough that the loop is not all overhead.
const PUMP_FRAMES: usize = 2048;
/// Start opening the next track this long before the current one ends.
const PRELOAD_SECS: f64 = 5.0;
/// Cadence of `Position` events. The frontend interpolates between them.
const POSITION_INTERVAL: Duration = Duration::from_millis(100);

pub enum EngineCommand {
    LoadQueue {
        entries: Vec<QueueEntry>,
        index: usize,
    },
    /// Plays an already-connected station. The HTTP round trip happens in the
    /// command layer on Tauri's async runtime, so the engine thread only ever
    /// receives a reader that is ready to decode.
    PlayStream {
        station_id: String,
        stream: Box<RadioStream>,
        /// True when a now-playing poller was started for this station.
        has_provider: bool,
    },
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Previous,
    JumpTo(usize),
    Seek(f64),
    SetShuffle(bool),
    SetRepeat(bool),
    SetNormalize(bool),
    SetDevice(Option<String>),
    /// A now-playing poller reporting what a station is playing. Carries the
    /// epoch it started under so a late answer about a station the listener
    /// has already left is discarded.
    StationMetadata {
        epoch: u64,
        update: Update,
    },
    /// Re-emit everything the UI needs to render current state. Sent on
    /// subscribe, so a reloaded webview catches up with audio that never
    /// stopped.
    Describe,
    Shutdown,
}

/// Where a track begins, measured in frames written to the ring. The UI is
/// told a track changed when this many frames have actually reached the
/// device, not when the decoder crossed the boundary — with gapless those are
/// up to half a second apart.
struct Boundary {
    frame: u64,
    index: usize,
    duration_secs: f64,
    /// The gain this track wants, in dB. Applied when the boundary reaches
    /// the device rather than when the decoder crossed it — at a gapless
    /// join the two are up to a ring-buffer apart, and applying it early
    /// would play the tail of the outgoing track at the next track's volume.
    gain_db: f64,
}

pub struct Engine {
    loudness_store: Arc<dyn Store>,
    commands: Receiver<EngineCommand>,
    params: Arc<Params>,
    subscribers: Arc<Subscribers>,

    queue: Queue,
    mode: Mode,
    playing: bool,
    station_id: Option<String>,

    device_id: Option<String>,
    output: Option<Output>,
    producer: Option<rtrb::Producer<f32>>,
    analyser: Option<Analyser>,

    decoder: Option<Decoder>,
    /// Opened early so a gapless transition costs no probe or allocation.
    preloaded: Option<Decoder>,
    /// Written by the ICY reader as the station announces tracks.
    now_playing: Option<NowPlaying>,
    reported_title: Option<String>,
    /// Whether the normalization gain is applied at all. Consulted at the
    /// moment a gain is set, so the Settings toggle takes effect mid-track
    /// rather than at the next track.
    normalize: bool,
    /// The gain of the track currently being decoded, in dB.
    current_gain_db: f64,
    /// Loudness being measured for the playing track, when it has no gain of
    /// its own. Abandoned on a seek — a partial listen measures the wrong
    /// thing.
    measuring: Option<(i64, Loudness)>,
    /// Set when the station has a now-playing provider. The provider is then
    /// the only source: merging it with ICY, which often disagrees about
    /// timing, produces worse answers than either feed alone.
    has_provider: bool,
    station_epoch: Arc<AtomicU64>,
    resampler: Option<Resampler>,

    /// Total frames handed to the ring since the stream was built.
    frames_written: u64,
    boundaries: VecDeque<Boundary>,
    current_duration: f64,
    reported_index: Option<usize>,

    decode_buf: Vec<f32>,
    mapped_buf: Vec<f32>,
    /// Resampled samples that did not fit in the ring last turn.
    pending_out: Vec<f32>,

    last_position: Instant,
    last_frame_push: Instant,
}

impl Engine {
    pub fn new(
        loudness_store: Arc<dyn Store>,
        commands: Receiver<EngineCommand>,
        params: Arc<Params>,
        subscribers: Arc<Subscribers>,
        station_epoch: Arc<AtomicU64>,
        device_id: Option<String>,
    ) -> Self {
        Self {
            loudness_store,
            commands,
            params,
            subscribers,
            queue: Queue::new(),
            mode: Mode::Idle,
            playing: false,
            station_id: None,
            device_id,
            output: None,
            producer: None,
            analyser: None,
            decoder: None,
            preloaded: None,
            now_playing: None,
            reported_title: None,
            normalize: true,
            current_gain_db: 0.0,
            measuring: None,
            has_provider: false,
            station_epoch,
            resampler: None,
            frames_written: 0,
            boundaries: VecDeque::new(),
            current_duration: 0.0,
            reported_index: None,
            decode_buf: Vec::new(),
            mapped_buf: Vec::new(),
            pending_out: Vec::new(),
            last_position: Instant::now(),
            last_frame_push: Instant::now(),
        }
    }

    pub fn run(mut self) {
        loop {
            let mut worked = false;
            loop {
                match self.commands.try_recv() {
                    Ok(EngineCommand::Shutdown) | Err(TryRecvError::Disconnected) => return,
                    Ok(command) => {
                        worked = true;
                        self.handle(command);
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            worked |= self.pump();
            self.emit_progress();
            self.push_visual_frame();

            if !worked {
                // Either idle or the ring is full; either way there is nothing
                // useful to do until the callback drains some of it.
                std::thread::sleep(Duration::from_millis(4));
            }
        }
    }

    // ── commands ────────────────────────────────────────────────────────

    fn handle(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::LoadQueue { entries, index } => {
                self.finish_measuring(false);
                self.station_id = None;
                self.now_playing = None;
                self.reported_title = None;
                self.has_provider = false;
                self.station_epoch.fetch_add(1, Ordering::Relaxed);
                self.queue.load(entries, index);
                if self.queue.is_empty() {
                    self.stop();
                } else {
                    self.mode = Mode::Local;
                    self.start_current();
                }
            }
            EngineCommand::PlayStream {
                station_id,
                stream,
                has_provider,
            } => {
                self.has_provider = has_provider;
                self.start_stream(station_id, *stream);
            }
            EngineCommand::StationMetadata { epoch, update } => {
                // A poller for a station already left behind must not
                // overwrite what is playing now.
                if epoch == self.station_epoch.load(Ordering::Relaxed) {
                    self.emit(EngineEvent::StreamMetadata {
                        title: update.info.as_ref().map(|i| i.title.clone()),
                        artist: update.info.as_ref().and_then(|i| i.artist.clone()),
                        album: update.info.as_ref().and_then(|i| i.album.clone()),
                        cover: update.cover,
                    });
                }
            }
            EngineCommand::Play => self.set_playing(true),
            EngineCommand::Pause => self.set_playing(false),
            EngineCommand::Toggle => self.set_playing(!self.playing),
            EngineCommand::Stop => self.stop(),
            EngineCommand::Next => {
                if self.mode == Mode::Local && self.queue.advance().is_some() {
                    self.start_current();
                }
            }
            EngineCommand::Previous => {
                if self.mode == Mode::Local && self.queue.back().is_some() {
                    self.start_current();
                }
            }
            EngineCommand::JumpTo(index) => {
                if self.mode == Mode::Local && self.queue.jump_to(index).is_some() {
                    self.start_current();
                }
            }
            EngineCommand::Seek(secs) => self.seek(secs),
            EngineCommand::SetShuffle(enabled) => {
                self.queue.set_shuffle(enabled);
                self.refresh_preload();
                self.emit_state();
            }
            EngineCommand::SetRepeat(enabled) => {
                self.queue.set_repeat(enabled);
                self.refresh_preload();
                self.emit_state();
            }
            EngineCommand::SetNormalize(enabled) => {
                self.normalize = enabled;
                // Re-apply what is playing so the switch is heard now rather
                // than at the next track.
                let gain = self.current_gain_db;
                self.apply_gain(gain);
            }
            EngineCommand::SetDevice(id) => {
                self.device_id = id;
                // Rebuilding the stream is the only way to change device, and
                // it invalidates the ring, so re-seek to where we were.
                let position = self.position_secs();
                self.output = None;
                self.producer = None;
                if self.ensure_output().is_ok() && self.decoder.is_some() {
                    self.seek(position);
                }
            }
            EngineCommand::Describe => self.describe(),
            EngineCommand::Shutdown => {}
        }
    }

    /// Begins measuring the playing track's loudness, when it is a local
    /// track that nothing has measured and no tag describes.
    ///
    /// Free: the engine is decoding these samples anyway. A track listened to
    /// end to end fills in its own gain for next time.
    fn start_measuring(&mut self, format: &super::decode::SourceFormat) {
        self.measuring = None;
        if self.mode != Mode::Local {
            return;
        }
        let Some(entry) = self.queue.current() else {
            return;
        };
        let track_id = entry.track_id;
        if !self.loudness_store.needs_measurement(track_id) {
            return;
        }
        if let Some(meter) = Loudness::new(format.sample_rate, format.channels) {
            self.measuring = Some((track_id, meter));
        }
    }

    /// Stores the measurement if the track was heard all the way through.
    ///
    /// Anything that skipped audio — a seek, a manual skip — abandons it
    /// instead: a partial listen measures the wrong thing, and a wrong gain
    /// is worse than none.
    fn finish_measuring(&mut self, complete: bool) {
        let Some((track_id, meter)) = self.measuring.take() else {
            return;
        };
        if !complete {
            return;
        }
        if let Some(measured) = meter.finish() {
            self.loudness_store.record(track_id, measured);
        }
    }

    /// Sets the mixer's gain for the track now audible, honouring the
    /// normalization switch. The clamp caps a boost at +12 dB so a broken tag
    /// cannot blow the output up.
    fn apply_gain(&mut self, gain_db: f64) {
        self.current_gain_db = gain_db;
        let linear = if self.normalize {
            loudness::db_to_linear(gain_db).clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.params.set_track_gain(linear);
    }

    /// Replays current state onto a freshly attached subscriber.
    fn describe(&mut self) {
        if let Some(output) = self.output.as_ref() {
            self.subscribers.send_event(EngineEvent::Device {
                name: output.config.device_name.clone(),
                sample_rate: output.config.sample_rate,
                channels: output.config.channels,
            });
        }
        if let Some(format) = self.decoder.as_ref().map(|d| d.format().clone()) {
            self.emit(EngineEvent::Format {
                sample_rate: format.sample_rate,
                channels: format.channels,
                codec: format.codec,
            });
        }
        self.emit_state();
        self.emit_progress_now();
    }

    fn set_playing(&mut self, playing: bool) {
        if self.mode == Mode::Idle || self.playing == playing {
            return;
        }
        self.playing = playing;
        if let Some(output) = self.output.as_ref() {
            let result = if playing {
                output.play()
            } else {
                output.pause()
            };
            if let Err(message) = result {
                self.emit(EngineEvent::Error { message });
            }
        }
        self.emit_state();
    }

    fn stop(&mut self) {
        self.finish_measuring(false);
        self.playing = false;
        self.mode = Mode::Idle;
        self.station_id = None;
        self.now_playing = None;
        self.reported_title = None;
        self.has_provider = false;
        self.station_epoch.fetch_add(1, Ordering::Relaxed);
        self.decoder = None;
        self.preloaded = None;
        self.pending_out.clear();
        self.boundaries.clear();
        self.params.request_flush();
        if let Some(output) = self.output.as_ref() {
            let _ = output.pause();
        }
        self.emit_state();
    }

    fn seek(&mut self, secs: f64) {
        if self.mode != Mode::Local {
            return;
        }
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        // Whatever has been measured so far no longer describes a full
        // listen.
        self.measuring = None;
        match decoder.seek(secs) {
            Ok(landed) => {
                // Drop everything already decoded past the old position, and
                // rebase the frame counter so the playhead jumps rather than
                // counting up from where it was.
                self.params.request_flush();
                self.pending_out.clear();
                self.boundaries.clear();
                let rate = self.device_rate().max(1) as f64;
                self.frames_written = (landed * rate) as u64;
                self.params.reset_frames(self.frames_written);
                self.boundaries.push_back(Boundary {
                    frame: self.frames_written,
                    index: self.queue.index(),
                    duration_secs: self.current_duration,
                    gain_db: self.current_gain_db,
                });
                self.emit_progress_now();
            }
            Err(message) => self.emit(EngineEvent::Error { message }),
        }
    }

    // ── track lifecycle ─────────────────────────────────────────────────

    fn start_current(&mut self) {
        let Some(entry) = self.queue.current().cloned() else {
            self.stop();
            return;
        };
        if let Err(message) = self.ensure_output() {
            self.emit(EngineEvent::Error { message });
            return;
        }

        match Decoder::open_file(&entry.path) {
            Ok(decoder) => {
                self.current_duration = decoder
                    .format()
                    .duration_secs
                    .unwrap_or(entry.duration_secs);
                self.install_decoder(decoder, entry.gain_db);
                self.mode = Mode::Local;
                self.begin_playback();
            }
            Err(message) => {
                self.emit(EngineEvent::Error { message });
                // A missing or corrupt file should not strand the queue.
                if self.queue.advance().is_some() {
                    self.start_current();
                } else {
                    self.stop();
                }
            }
        }
    }

    fn start_stream(&mut self, station_id: String, stream: RadioStream) {
        if let Err(message) = self.ensure_output() {
            self.emit(EngineEvent::Error { message });
            return;
        }
        match Decoder::open(stream.source, stream.hint) {
            Ok(decoder) => {
                // A station has no length and no normalization gain.
                self.current_duration = 0.0;
                self.install_decoder(decoder, 0.0);
                self.mode = Mode::Radio;
                self.station_id = Some(station_id);
                self.now_playing = Some(stream.now_playing);
                self.reported_title = None;
                self.begin_playback();
            }
            Err(message) => self.emit(EngineEvent::Error { message }),
        }
    }

    /// Swaps in a freshly opened decoder and resets everything derived from
    /// the old one.
    fn install_decoder(&mut self, decoder: Decoder, gain_db: f64) {
        let format = decoder.format().clone();
        self.start_measuring(&format);
        self.apply_gain(gain_db);
        self.rebuild_resampler(format.sample_rate);
        self.decoder = Some(decoder);
        self.preloaded = None;

        self.params.request_flush();
        self.pending_out.clear();
        self.boundaries.clear();
        self.frames_written = 0;
        self.params.reset_frames(0);
        self.reported_index = None;
        self.boundaries.push_back(Boundary {
            frame: 0,
            index: self.queue.index(),
            duration_secs: self.current_duration,
            gain_db: self.current_gain_db,
        });

        self.emit(EngineEvent::Format {
            sample_rate: format.sample_rate,
            channels: format.channels,
            codec: format.codec,
        });
    }

    fn begin_playback(&mut self) {
        self.playing = true;
        if let Some(output) = self.output.as_ref() {
            if let Err(message) = output.play() {
                self.emit(EngineEvent::Error { message });
            }
        }
        self.refresh_preload();
        self.emit_state();
    }

    /// Opens the next track early so a gapless join costs no probe.
    fn refresh_preload(&mut self) {
        if self.mode != Mode::Local {
            self.preloaded = None;
            return;
        }
        let Some(next) = self.queue.peek_next().cloned() else {
            self.preloaded = None;
            return;
        };
        let remaining = self.current_duration - self.position_secs();
        if self.preloaded.is_none() && remaining <= PRELOAD_SECS {
            match Decoder::open_file(&next.path) {
                Ok(decoder) => self.preloaded = Some(decoder),
                // Not fatal: the transition just falls back to opening late.
                Err(message) => log::warn!("preload failed: {}", message),
            }
        }
    }

    // ── the pump ────────────────────────────────────────────────────────

    /// Moves audio from the decoder into the ring. Returns whether it made
    /// progress, so the caller knows whether to sleep.
    fn pump(&mut self) -> bool {
        if !self.playing || self.decoder.is_none() {
            return false;
        }
        let channels = self.device_channels() as usize;
        let Some(producer) = self.producer.as_ref() else {
            return false;
        };
        if producer.slots() < PUMP_FRAMES * channels {
            return false;
        }

        let source_channels = self
            .decoder
            .as_ref()
            .map(|d| d.format().channels as usize)
            .unwrap_or(channels)
            .max(1);

        self.decode_buf.resize(PUMP_FRAMES * source_channels, 0.0);
        let read = match self.decoder.as_mut().unwrap().read(&mut self.decode_buf) {
            Ok(read) => read,
            Err(message) => {
                self.emit(EngineEvent::Error { message });
                self.advance_or_stop();
                return true;
            }
        };

        if read > 0 {
            // Measured at the source's own rate and channel count: the answer
            // should describe the file, not whatever device it happens to be
            // playing through.
            if let Some((_, meter)) = self.measuring.as_mut() {
                meter.feed(&self.decode_buf[..read]);
            }

            self.mapped_buf.clear();
            remap_channels(
                &self.decode_buf[..read],
                source_channels,
                channels,
                &mut self.mapped_buf,
            );

            // `pending_out` is appended to, never cleared here: upsampling
            // produces more frames than were read, so the ring may not have
            // room for all of them this turn. The remainder waits rather than
            // being dropped, which would be an audible gap.
            let mapped = std::mem::take(&mut self.mapped_buf);
            if let Some(resampler) = self.resampler.as_mut() {
                if let Err(message) = resampler.process(&mapped, &mut self.pending_out) {
                    self.emit(EngineEvent::Error { message });
                }
            } else {
                self.pending_out.extend_from_slice(&mapped);
            }
            self.mapped_buf = mapped;
        }

        let pushed = self.push_pending(channels);

        // Only move on once the tail of this track has actually been handed
        // over — otherwise the last fraction of a second is lost, which is
        // exactly the join gapless is supposed to make seamless.
        if self.decoder.as_ref().is_some_and(|d| d.is_exhausted()) && self.pending_out.is_empty() {
            self.advance_or_stop();
        } else {
            self.refresh_preload();
        }

        read > 0 || pushed
    }

    /// Moves as much of `pending_out` into the ring as fits, keeping the rest.
    ///
    /// Always writes a whole number of frames: the ring is a flat stream of
    /// interleaved samples, so committing a partial frame would shift every
    /// later frame's channel alignment and swap the stereo image.
    fn push_pending(&mut self, channels: usize) -> bool {
        if self.pending_out.is_empty() || channels == 0 {
            return false;
        }
        let Some(producer) = self.producer.as_mut() else {
            return false;
        };
        let room = (producer.slots().min(self.pending_out.len()) / channels) * channels;
        if room == 0 {
            return false;
        }
        let Ok(chunk) = producer.write_chunk_uninit(room) else {
            return false;
        };
        chunk.fill_from_iter(self.pending_out[..room].iter().copied());
        self.pending_out.drain(..room);
        self.frames_written += (room / channels) as u64;
        true
    }

    /// End of a track: hand over to the preloaded decoder without flushing —
    /// that continuity is what makes the join gapless.
    fn advance_or_stop(&mut self) {
        // Reaching here from an exhausted decoder means the whole track was
        // decoded, which is the only case worth recording.
        let complete = self.decoder.as_ref().is_some_and(|d| d.is_exhausted());
        self.finish_measuring(complete);

        if self.mode != Mode::Local {
            self.stop();
            return;
        }
        let Some(next) = self.queue.peek_next().cloned() else {
            self.decoder = None;
            self.playing = false;
            self.emit_state();
            return;
        };
        self.queue.advance();

        let decoder = match self.preloaded.take() {
            Some(decoder) => Some(decoder),
            None => match Decoder::open_file(&next.path) {
                Ok(decoder) => Some(decoder),
                Err(message) => {
                    self.emit(EngineEvent::Error { message });
                    None
                }
            },
        };

        let Some(decoder) = decoder else {
            self.advance_or_stop();
            return;
        };

        let format = decoder.format().clone();
        self.current_duration = format.duration_secs.unwrap_or(next.duration_secs);
        // Deliberately *not* applied here: the outgoing track is still in the
        // ring. The boundary below carries it, and it lands when the join
        // actually reaches the device.
        self.rebuild_resampler(format.sample_rate);
        self.decoder = Some(decoder);
        self.preloaded = None;

        // No flush: whatever is still in the ring belongs to the previous
        // track and must be heard. The boundary tells us when the new track
        // actually starts coming out of the speakers.
        self.boundaries.push_back(Boundary {
            frame: self.frames_written,
            index: self.queue.index(),
            duration_secs: self.current_duration,
            gain_db: next.gain_db,
        });

        self.emit(EngineEvent::Format {
            sample_rate: format.sample_rate,
            channels: format.channels,
            codec: format.codec,
        });
        self.refresh_preload();
    }

    // ── output plumbing ─────────────────────────────────────────────────

    fn ensure_output(&mut self) -> Result<(), String> {
        if self.output.is_some() {
            return Ok(());
        }
        let rate_hint = 48_000;
        let ring = (rate_hint as f32 * RING_SECONDS) as usize * 2;
        let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring.max(PUMP_FRAMES * 4));
        // Roughly 170 ms of mono at 48 kHz: the FFT window plus headroom.
        let (tap_producer, tap_consumer) = rtrb::RingBuffer::<f32>::new(8192);

        let output = output::open(
            self.device_id.as_deref(),
            Arc::clone(&self.params),
            consumer,
            tap_producer,
        )?;

        self.emit(EngineEvent::Device {
            name: output.config.device_name.clone(),
            sample_rate: output.config.sample_rate,
            channels: output.config.channels,
        });

        self.producer = Some(producer);
        self.analyser = Some(Analyser::new(tap_consumer));
        self.output = Some(output);
        Ok(())
    }

    fn rebuild_resampler(&mut self, source_rate: u32) {
        let device_rate = self.device_rate();
        let channels = self.device_channels() as usize;
        if source_rate == 0 || device_rate == 0 {
            self.resampler = None;
            return;
        }
        match Resampler::new(source_rate, device_rate, channels) {
            Ok(resampler) => {
                self.resampler = if resampler.is_passthrough() {
                    None
                } else {
                    Some(resampler)
                }
            }
            Err(message) => {
                self.emit(EngineEvent::Error { message });
                self.resampler = None;
            }
        }
    }

    fn device_rate(&self) -> u32 {
        self.output
            .as_ref()
            .map(|o| o.config.sample_rate)
            .unwrap_or(0)
    }

    fn device_channels(&self) -> u16 {
        self.output.as_ref().map(|o| o.config.channels).unwrap_or(2)
    }

    // ── reporting ───────────────────────────────────────────────────────

    /// Seconds into the current track, derived from what the callback has
    /// actually played rather than from what the decoder has run ahead to.
    fn position_secs(&self) -> f64 {
        let rate = self.device_rate();
        if rate == 0 {
            return 0.0;
        }
        let played = self.params.frames_played();
        let base = self
            .boundaries
            .iter()
            .rev()
            .find(|b| b.frame <= played)
            .map(|b| b.frame)
            .unwrap_or(0);
        (played.saturating_sub(base)) as f64 / rate as f64
    }

    fn emit_progress(&mut self) {
        if self.last_position.elapsed() < POSITION_INTERVAL {
            return;
        }
        self.emit_progress_now();
    }

    fn emit_progress_now(&mut self) {
        self.last_position = Instant::now();

        // Drop every boundary the device has already passed except the most
        // recent, which stays as the origin for `position_secs`. Announcing
        // the crossing here rather than when the decoder reached it is what
        // makes the UI flip at the moment the listener hears the new track.
        let played = self.params.frames_played();
        while self.boundaries.len() > 1 && self.boundaries[1].frame <= played {
            self.boundaries.pop_front();
        }

        if let Some(boundary) = self.boundaries.front().filter(|b| b.frame <= played) {
            let (index, duration, gain_db) =
                (boundary.index, boundary.duration_secs, boundary.gain_db);
            self.current_duration = duration;
            if self.reported_index != Some(index) {
                self.reported_index = Some(index);
                // The new track is only now reaching the speakers, so this is
                // the moment its gain becomes the right one. `Gain` ramps
                // across the buffer, so the change is inaudible.
                self.apply_gain(gain_db);
                self.emit(EngineEvent::TrackChanged { index });
            }
        }

        self.emit_stream_metadata();

        if self.mode != Mode::Idle {
            self.emit(EngineEvent::Position {
                position_secs: self.position_secs(),
                duration_secs: if self.mode == Mode::Radio {
                    0.0
                } else {
                    self.current_duration
                },
            });
        }
    }

    /// Announces a station's current track when it changes.
    ///
    /// Skipped entirely when a now-playing provider is polling: it reports the
    /// same track with more detail, and two sources racing would flicker.
    fn emit_stream_metadata(&mut self) {
        if self.has_provider {
            return;
        }
        let Some(source) = self.now_playing.as_ref() else {
            return;
        };
        // Clone out and release the lock before emitting — the ICY reader
        // takes this same lock from inside a decode read.
        let title = match source.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return,
        };
        if title == self.reported_title {
            return;
        }
        self.reported_title = title.clone();

        let parsed = title.as_deref().and_then(icy::parse);
        self.emit(EngineEvent::StreamMetadata {
            title: parsed.as_ref().map(|p| p.title.clone()),
            artist: parsed.as_ref().and_then(|p| p.artist.clone()),
            album: parsed.as_ref().and_then(|p| p.album.clone()),
            cover: None,
        });
    }

    /// Builds and sends one visualiser frame, at roughly 60 Hz.
    fn push_visual_frame(&mut self) {
        let Some(analyser) = self.analyser.as_mut() else {
            return;
        };
        analyser.drain();
        if self.last_frame_push.elapsed() < Duration::from_millis(16) {
            return;
        }
        self.last_frame_push = Instant::now();
        let frame = analyser.frame();
        debug_assert_eq!(frame.len(), FRAME_BYTES);
        self.subscribers.send_frame(&frame);
    }

    fn emit_state(&self) {
        self.emit(EngineEvent::State {
            playing: self.playing,
            mode: self.mode,
            index: self.queue.index(),
            queue_len: self.queue.len(),
            shuffle: self.queue.shuffle(),
            repeat: self.queue.repeat(),
            station_id: self.station_id.clone(),
        });
    }

    fn emit(&self, event: EngineEvent) {
        self.subscribers.send_event(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// These tests are about audio reaching the device, not about remembering
    /// how loud it was.
    fn no_store() -> Arc<dyn Store> {
        Arc::new(super::super::loudness::NoStore)
    }

    /// End-to-end through a real output device: decode a generated file,
    /// resample it, push it through the ring, and confirm the callback
    /// consumed frames.
    ///
    /// Ignored by default because it needs an audio device — `just test-rust`
    /// has to pass on a headless machine. Run it deliberately with
    /// `cargo test -- --ignored play_a_file_end_to_end --nocapture`.
    #[test]
    #[ignore = "requires a real audio output device"]
    fn play_a_file_end_to_end() {
        let dir = std::env::temp_dir().join("janis-engine-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("tone.wav");

        // Two seconds of a 440 Hz tone, 16-bit stereo at 44.1 kHz — a rate
        // that differs from most devices' 48 kHz, so the resampler is on the
        // path too.
        let (rate, channels, frames) = (44_100u32, 2u16, 88_200usize);
        let mut wav = Vec::new();
        let block_align = channels * 2;
        let data_len = frames as u32 * block_align as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for n in 0..frames {
            let t = n as f32 / rate as f32;
            let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16;
            for _ in 0..channels {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
        }
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(&wav))
            .expect("write test tone");

        let engine = crate::audio::init(no_store(), None);
        // Silent: the callback still runs and counts frames, and nobody in
        // the room has to listen to a test tone.
        engine.params().set_volume(0.0);
        engine
            .send(EngineCommand::LoadQueue {
                entries: vec![QueueEntry {
                    track_id: 1,
                    path: path.clone(),
                    duration_secs: 2.0,
                    gain_db: 0.0,
                }],
                index: 0,
            })
            .expect("engine accepts the queue");

        std::thread::sleep(Duration::from_millis(700));
        let played = engine.params().frames_played();
        crate::audio::shutdown(&engine);
        let _ = std::fs::remove_file(&path);

        assert!(
            played > 0,
            "the output callback consumed no frames — nothing reached the device"
        );
        // Roughly half a second of audio should have gone out by now; allow a
        // wide margin for a slow device opening.
        assert!(
            played > 4_000,
            "only {played} frames played, expected the stream to be running"
        );
    }

    /// The same path, fed by a live Icecast station instead of a file.
    ///
    /// Ignored by default: it needs both an audio device and the network.
    /// Run with `cargo test -- --ignored play_a_radio_station --nocapture`.
    #[test]
    #[ignore = "requires an audio device and network access"]
    fn play_a_radio_station() {
        // SomaFM is in the curated list, allows direct connections, and sends
        // ICY metadata — so this exercises titles as well as audio.
        const URL: &str = "https://ice1.somafm.com/groovesalad-128-mp3";

        let stream = tauri::async_runtime::block_on(super::super::stream::open(URL))
            .expect("station should connect");
        let now_playing = Arc::clone(&stream.now_playing);

        let engine = crate::audio::init(no_store(), None);
        engine.params().set_volume(0.0);
        engine
            .send(EngineCommand::PlayStream {
                station_id: "soma-groovesalad".to_string(),
                stream: Box::new(stream),
                // No poller here: this test is about audio reaching the
                // device, so ICY stays the metadata source.
                has_provider: false,
            })
            .expect("engine accepts the station");

        std::thread::sleep(Duration::from_millis(2500));
        let played = engine.params().frames_played();
        let title = now_playing.lock().ok().and_then(|t| t.clone());
        crate::audio::shutdown(&engine);

        assert!(
            played > 4_000,
            "only {played} frames played — the station did not reach the device"
        );
        println!("radio played {played} frames; icy title: {title:?}");
    }
}
