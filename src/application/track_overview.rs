//! The controller's side of the track overview: the shape of the whole
//! track the mini player draws, DAW-style, in place of the oscilloscope while
//! a track plays (a station keeps the oscilloscope).
//!
//! The engine reads the track on a thread of its own — a file from disk, a
//! server's song from the server — and the slices arrive while it reads, so
//! the shape grows from left to right. A read the listener has moved on from
//! stops at its next batch. Finished overviews are kept for a few tracks, so
//! going back to one draws it at once.

use std::ops::ControlFlow;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image};

use super::{post, Application};
use crate::audio::OverviewSlice;
use crate::covers::BoundedCache;
use crate::overview;
use crate::player::Player;
use crate::user_interface;

/// How often a read in progress redraws the shape.
const REDRAW_INTERVAL: Duration = Duration::from_millis(150);
/// How many finished overviews are kept: a few hundred kilobytes each.
const FINISHED_LIMIT: usize = 8;
/// How far the track's length may move before the shape is redrawn to it.
const DURATION_TOLERANCE_SECONDS: f64 = 0.05;

/// What the controller keeps for the overview.
pub(super) struct TrackOverview {
    /// Bumped on every change of track. A read carries the value it started
    /// under and stops as soon as it no longer matches.
    epoch: Arc<AtomicU64>,
    /// The track whose overview is shown or being read.
    track_id: Option<i64>,
    /// The slices read so far, filled by the overview thread.
    slices: Arc<Mutex<Vec<OverviewSlice>>>,
    /// Nothing could be read: the oscilloscope stands in.
    failed: bool,
    /// The view's size in physical pixels, as it last reported it.
    pixel_size: (usize, usize),
    /// The track length the shape was last drawn to.
    drawn_duration: f64,
    finished: BoundedCache<i64, Arc<Vec<OverviewSlice>>>,
}

impl TrackOverview {
    pub fn new() -> Self {
        Self {
            epoch: Default::default(),
            track_id: None,
            slices: Default::default(),
            failed: false,
            pixel_size: (0, 0),
            drawn_duration: 0.0,
            finished: BoundedCache::new(FINISHED_LIMIT),
        }
    }

    /// Whether the overview stands for what `player` plays.
    fn shown_for(&self, player: &Player) -> bool {
        !self.failed
            && self.track_id.is_some()
            && self.track_id == player.current().map(|track| track.id)
    }

    /// Stops the read in progress, at shut-down.
    pub fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

fn lock(slices: &Mutex<Vec<OverviewSlice>>) -> MutexGuard<'_, Vec<OverviewSlice>> {
    slices.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Application {
    /// Wires the overview view: it reports its size in physical pixels.
    pub(super) fn connect_track_overview(self: &Rc<Self>) {
        let view = self.window.global::<user_interface::Player>();
        let weak = Rc::downgrade(self);
        view.on_overview_resized(move |width, height| {
            if let Some(application) = weak.upgrade() {
                let size = (
                    usize::try_from(width).unwrap_or_default(),
                    usize::try_from(height).unwrap_or_default(),
                );
                let changed = {
                    let mut state = application.track_overview.borrow_mut();
                    std::mem::replace(&mut state.pixel_size, size) != size
                };
                if changed {
                    application.draw_overview();
                }
            }
        });
    }

    /// Starts reading the overview of `track_id`, or, with `None`, puts the
    /// oscilloscope back.
    pub(super) fn load_overview(&self, track_id: Option<i64>) {
        let job = {
            let mut state = self.track_overview.borrow_mut();
            // The engine confirms a track the click already showed: its
            // overview is not read a second time.
            if track_id.is_some() && state.track_id == track_id && !state.failed {
                return;
            }
            let epoch = state.epoch.fetch_add(1, Ordering::Relaxed) + 1;
            state.track_id = track_id;
            state.failed = false;
            let cached = track_id.and_then(|track_id| state.finished.get(&track_id).cloned());
            state.slices = Arc::new(Mutex::new(cached.as_deref().cloned().unwrap_or_default()));
            match track_id {
                Some(track_id) if cached.is_none() => Some((
                    epoch,
                    track_id,
                    Arc::clone(&state.epoch),
                    Arc::clone(&state.slices),
                )),
                _ => None,
            }
        };
        self.draw_overview();
        if let Some((epoch, track_id, shared_epoch, slices)) = job {
            self.read_overview(epoch, track_id, shared_epoch, slices);
        }
    }

    /// Reads the overview on the overview thread, trying each place the
    /// track can be read from until one answers.
    fn read_overview(
        &self,
        epoch: u64,
        track_id: i64,
        shared_epoch: Arc<AtomicU64>,
        slices: Arc<Mutex<Vec<OverviewSlice>>>,
    ) {
        let track = self
            .player
            .borrow()
            .queue()
            .iter()
            .find(|track| track.id == track_id)
            .cloned();
        let Some(track) = track else {
            return;
        };
        let locations = self.overview_locations(&track);
        let engine = Arc::clone(&self.engine);
        // A thread per read: one stuck connecting to a server, or waiting on
        // a stalled share, notices it is stale only at its next batch, and
        // must not hold up the next track's read meanwhile.
        let spawned = std::thread::Builder::new()
            .name("janis overview".to_owned())
            .spawn(move || {
                let current = || shared_epoch.load(Ordering::Relaxed) == epoch;
                let started = Instant::now();
                let mut last_redraw = started;
                let mut outcome = Err(String::from("nowhere to read the track from"));
                for location in locations {
                    if !current() {
                        return;
                    }
                    let mut received = false;
                    outcome = engine.read_overview(&location, &mut |batch| {
                        if !current() {
                            return ControlFlow::Break(());
                        }
                        received = true;
                        lock(&slices).extend_from_slice(batch);
                        if last_redraw.elapsed() >= REDRAW_INTERVAL {
                            last_redraw = Instant::now();
                            post(move |application| application.overview_progressed(epoch));
                        }
                        ControlFlow::Continue(())
                    });
                    // A source that answers but yields no audio (a server's
                    // transcode it could not make) is as good as none.
                    if matches!(outcome, Ok(true)) && !received {
                        outcome = Err(format!("{location}: no audio"));
                    }
                    match &outcome {
                        Err(error) if !received => {
                            log::debug!("track overview: {error}");
                        }
                        _ => break,
                    }
                }
                log::debug!(
                    "track overview: {} slices read in {:?}",
                    lock(&slices).len(),
                    started.elapsed()
                );
                post(move |application| application.overview_finished(epoch, track_id, outcome));
            });
        if let Err(error) = spawned {
            log::warn!("start a track overview read: {error}");
        }
    }

    fn overview_progressed(&self, epoch: u64) {
        if self.track_overview.borrow().epoch.load(Ordering::Relaxed) == epoch {
            self.draw_overview();
        }
    }

    fn overview_finished(&self, epoch: u64, track_id: i64, outcome: Result<bool, String>) {
        {
            let mut state = self.track_overview.borrow_mut();
            if state.epoch.load(Ordering::Relaxed) != epoch {
                return;
            }
            match outcome {
                Ok(true) => {
                    let slices = Arc::new(lock(&state.slices).clone());
                    state.finished.insert(track_id, slices);
                }
                // Only a read the listener moved on from stops early.
                Ok(false) => return,
                Err(error) => {
                    // The track itself plays or fails on its own; what was
                    // read stays drawn, and with nothing read the
                    // oscilloscope stands in.
                    log::warn!("track overview: {error}");
                    let nothing_read = lock(&state.slices).is_empty();
                    state.failed = nothing_read;
                }
            }
        }
        self.draw_overview();
        self.refresh_player();
    }

    /// Rasterises the slices read so far at the view's size.
    fn draw_overview(&self) {
        let duration = self.player.borrow().duration_seconds();
        let mut state = self.track_overview.borrow_mut();
        state.drawn_duration = duration;
        let (width, height) = state.pixel_size;
        let image = {
            let slices = lock(&state.slices);
            if slices.is_empty() || width == 0 || height == 0 {
                Image::default()
            } else {
                Image::from_rgba8_premultiplied(overview::mask(&slices, duration, width, height))
            }
        };
        self.window
            .global::<user_interface::Player>()
            .set_overview(image);
    }

    /// Moves the overview's playhead to `progress`, in whole pixels of the
    /// view: the playhead of a long track crosses a pixel every few frames,
    /// and a frame that changes nothing costs nothing to draw.
    pub(super) fn show_overview_playhead(&self, progress: f64) {
        let width = self.track_overview.borrow().pixel_size.0;
        self.window
            .global::<user_interface::Player>()
            .set_overview_playhead(overview::pixel_progress(progress, width) as f32);
    }

    /// Pushes whether the overview stands in for the oscilloscope, and
    /// redraws it when the engine corrected the track's length.
    pub(super) fn refresh_track_overview(&self, player: &Player) {
        let (shown, stale) = {
            let state = self.track_overview.borrow();
            let shown = state.shown_for(player);
            let stale = (player.duration_seconds() - state.drawn_duration).abs()
                > DURATION_TOLERANCE_SECONDS;
            (shown, shown && stale)
        };
        self.window
            .global::<user_interface::Player>()
            .set_overview_shown(shown);
        if stale {
            self.draw_overview();
        }
    }
}
