//! The controller's side of the track overview: the shape of the whole
//! track the mini player draws, DAW-style, in place of the oscilloscope while
//! a track plays (a station keeps the oscilloscope).
//!
//! The engine reads the track on a thread of its own — a file from disk, a
//! server's song from the server — and the slices arrive while it reads, so
//! the shape grows from left to right. `overview_reads` decides which reads
//! start, which results count and what is kept; this side starts the threads
//! and draws.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image};

use super::{post, Application};
use crate::overview;
use crate::overview_reads::{self, ReadJob, Start};
use crate::player::Player;
use crate::user_interface;

/// How often a read in progress redraws the shape.
const REDRAW_INTERVAL: Duration = Duration::from_millis(150);

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
                let changed = application.track_overview.borrow_mut().resize(size);
                if changed {
                    application.draw_overview();
                }
            }
        });
    }

    /// Starts reading the overview of `track_id`, or, with `None`, puts the
    /// oscilloscope back.
    pub(super) fn load_overview(&self, track_id: Option<i64>) {
        let start = self.track_overview.borrow_mut().start(track_id);
        let Start::Shown { job } = start else {
            return;
        };
        self.draw_overview();
        if let Some(job) = job {
            self.read_overview(job);
        }
    }

    /// Reads the overview on a thread of its own, trying each place the
    /// track can be read from until one answers.
    fn read_overview(&self, job: ReadJob) {
        let track = self
            .player
            .borrow()
            .queue()
            .iter()
            .find(|track| track.id == job.track_id)
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
                let (epoch, track_id) = (job.epoch, job.track_id);
                let started = Instant::now();
                let mut last_redraw = started;
                let outcome = overview_reads::read_first_answering(
                    locations,
                    &|| job.is_current(),
                    &mut |location, on_batch| engine.read_overview(location, on_batch),
                    &mut |batch| {
                        overview_reads::lock(&job.slices).extend_from_slice(batch);
                        if last_redraw.elapsed() >= REDRAW_INTERVAL {
                            last_redraw = Instant::now();
                            post(move |application| application.overview_progressed(epoch));
                        }
                    },
                );
                let Some(outcome) = outcome else {
                    return;
                };
                log::debug!(
                    "track overview: {} slices read in {:?}",
                    overview_reads::lock(&job.slices).len(),
                    started.elapsed()
                );
                post(move |application| application.overview_finished(epoch, track_id, outcome));
            });
        if let Err(error) = spawned {
            log::warn!("start a track overview read: {error}");
        }
    }

    fn overview_progressed(&self, epoch: u64) {
        if self.track_overview.borrow().is_current(epoch) {
            self.draw_overview();
        }
    }

    fn overview_finished(&self, epoch: u64, track_id: i64, outcome: Result<bool, String>) {
        let redraw = self
            .track_overview
            .borrow_mut()
            .finish(epoch, track_id, outcome);
        if redraw {
            self.draw_overview();
            self.refresh_player();
        }
    }

    /// Rasterises the slices read so far at the view's size.
    fn draw_overview(&self) {
        let duration = self.player.borrow().duration_seconds();
        let image = {
            let mut reads = self.track_overview.borrow_mut();
            let (width, height) = reads.pixel_size();
            let slices = reads.slices_drawn_at(duration);
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
        let width = self.track_overview.borrow().pixel_size().0;
        self.window
            .global::<user_interface::Player>()
            .set_overview_playhead(overview::pixel_progress(progress, width) as f32);
    }

    /// Pushes whether the overview stands in for the oscilloscope, and
    /// redraws it when the engine corrected the track's length.
    pub(super) fn refresh_track_overview(&self, player: &Player) {
        let current = player.current().map(|track| track.id);
        let (shown, redraw) = {
            let reads = self.track_overview.borrow();
            (
                reads.shown_for(current),
                reads.redraw_needed(current, player.duration_seconds()),
            )
        };
        self.window
            .global::<user_interface::Player>()
            .set_overview_shown(shown);
        if redraw {
            self.draw_overview();
        }
    }
}
