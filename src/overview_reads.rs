//! The track overview's reads: which track the shape stands for, whether a
//! read is still wanted, what a finished read leaves behind, and the order the
//! places a track can be read from are tried in. The controller starts the
//! threads and draws; this decides.
//!
//! A read carries the epoch it started under and stops as soon as the epoch
//! moves on, which every change of track does. Finished overviews are kept
//! for a few tracks, so going back to one draws it at once.

use std::fmt::Display;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::audio::OverviewSlice;
use crate::covers::BoundedCache;

/// How many finished overviews are kept: a few hundred kilobytes each.
const FINISHED_LIMIT: usize = 8;
/// How far the track's length may move before the shape is redrawn to it.
const DURATION_TOLERANCE_SECONDS: f64 = 0.05;

/// What a read hands each batch of slices to; breaking stops the read.
pub type OnBatch<'callback> = dyn FnMut(&[OverviewSlice]) -> ControlFlow<()> + 'callback;

/// The slices of one read, shared with the thread that reads them.
pub type SharedSlices = Arc<Mutex<Vec<OverviewSlice>>>;

/// A read to start on a thread of its own.
pub struct ReadJob {
    pub epoch: u64,
    pub track_id: i64,
    /// The epoch as it moves on, for the thread to notice it is stale.
    pub shared_epoch: Arc<AtomicU64>,
    pub slices: SharedSlices,
}

impl ReadJob {
    /// Whether the read is still the one wanted.
    pub fn is_current(&self) -> bool {
        self.shared_epoch.load(Ordering::Relaxed) == self.epoch
    }
}

/// What asking for a track's overview leads to.
pub enum Start {
    /// The overview already stands for that track: nothing to do.
    Unchanged,
    /// The overview now stands for the track asked for: drawn from what is
    /// kept, and read on `job` when nothing is.
    Shown { job: Option<ReadJob> },
}

pub struct OverviewReads {
    epoch: Arc<AtomicU64>,
    /// The track whose overview is shown or being read.
    track_id: Option<i64>,
    slices: SharedSlices,
    /// Nothing could be read: the oscilloscope stands in.
    failed: bool,
    /// The view's size in physical pixels, as it last reported it.
    pixel_size: (usize, usize),
    /// The track length the shape was last drawn to.
    drawn_duration: f64,
    finished: BoundedCache<i64, Arc<Vec<OverviewSlice>>>,
}

impl Default for OverviewReads {
    fn default() -> Self {
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
}

impl OverviewReads {
    /// The overview of `track_id`, or, with `None`, the oscilloscope back.
    pub fn start(&mut self, track_id: Option<i64>) -> Start {
        // The engine confirms a track the click already showed: its overview
        // is not read a second time.
        if track_id.is_some() && self.track_id == track_id && !self.failed {
            return Start::Unchanged;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        self.track_id = track_id;
        self.failed = false;
        let kept = track_id.and_then(|track_id| self.finished.get(&track_id).cloned());
        self.slices = Arc::new(Mutex::new(kept.as_deref().cloned().unwrap_or_default()));
        let job = match track_id {
            Some(track_id) if kept.is_none() => Some(ReadJob {
                epoch,
                track_id,
                shared_epoch: Arc::clone(&self.epoch),
                slices: Arc::clone(&self.slices),
            }),
            _ => None,
        };
        Start::Shown { job }
    }

    /// Whether a read started under `epoch` is still the one wanted.
    pub fn is_current(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::Relaxed) == epoch
    }

    /// A read ended. Returns whether the overview is to be drawn again: not
    /// for a read the listener moved on from.
    pub fn finish(&mut self, epoch: u64, track_id: i64, outcome: Result<bool, String>) -> bool {
        if !self.is_current(epoch) {
            return false;
        }
        match outcome {
            Ok(true) => {
                let slices = Arc::new(lock(&self.slices).clone());
                self.finished.insert(track_id, slices);
                true
            }
            // Only a read the listener moved on from stops early.
            Ok(false) => false,
            Err(error) => {
                // What was read stays drawn; with nothing read, the
                // oscilloscope stands in.
                log::warn!("track overview: {error}");
                self.failed = lock(&self.slices).is_empty();
                true
            }
        }
    }

    /// The view reported its size. Returns whether it changed.
    pub fn resize(&mut self, size: (usize, usize)) -> bool {
        std::mem::replace(&mut self.pixel_size, size) != size
    }

    pub fn pixel_size(&self) -> (usize, usize) {
        self.pixel_size
    }

    /// Whether the overview stands for the track `current_track_id` names.
    pub fn shown_for(&self, current_track_id: Option<i64>) -> bool {
        !self.failed && self.track_id.is_some() && self.track_id == current_track_id
    }

    /// Whether the overview shown was drawn to a length the engine has since
    /// corrected.
    pub fn redraw_needed(&self, current_track_id: Option<i64>, duration: f64) -> bool {
        self.shown_for(current_track_id)
            && (duration - self.drawn_duration).abs() > DURATION_TOLERANCE_SECONDS
    }

    /// The slices to draw, at the length they are drawn to.
    pub fn slices_drawn_at(&mut self, duration: f64) -> MutexGuard<'_, Vec<OverviewSlice>> {
        self.drawn_duration = duration;
        lock(&self.slices)
    }

    /// Stops the read in progress, at shut-down.
    pub fn stop(&self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn lock(slices: &Mutex<Vec<OverviewSlice>>) -> MutexGuard<'_, Vec<OverviewSlice>> {
    slices.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tries each place a track can be read from, in order, until one answers.
/// `read` reads one place, handing each batch to the callback it is given;
/// `take_batch` receives what was read. A place that answers but yields no
/// audio (a transcode the server could not make) counts as one that failed;
/// one that fails after yielding some keeps what it gave, and nothing else is
/// tried. `None` when the read went stale before a place was tried.
pub fn read_first_answering<Location: Display>(
    locations: Vec<Location>,
    is_current: &dyn Fn() -> bool,
    read: &mut dyn FnMut(&Location, &mut OnBatch<'_>) -> Result<bool, String>,
    take_batch: &mut dyn FnMut(&[OverviewSlice]),
) -> Option<Result<bool, String>> {
    let mut outcome = Err(String::from("nowhere to read the track from"));
    for location in locations {
        if !is_current() {
            return None;
        }
        let mut received = false;
        outcome = read(&location, &mut |batch| {
            if !is_current() {
                return ControlFlow::Break(());
            }
            received = true;
            take_batch(batch);
            ControlFlow::Continue(())
        });
        if matches!(outcome, Ok(true)) && !received {
            outcome = Err(format!("{location}: no audio"));
        }
        match &outcome {
            Err(error) if !received => log::debug!("track overview: {error}"),
            _ => break,
        }
    }
    Some(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLICE: OverviewSlice = OverviewSlice {
        minimum: -0.5,
        maximum: 0.5,
        rms: 0.3,
    };

    fn job(start: Start) -> Option<ReadJob> {
        match start {
            Start::Unchanged => panic!("expected the overview to change"),
            Start::Shown { job } => job,
        }
    }

    #[test]
    fn a_new_track_is_read_and_the_engine_confirming_it_reads_nothing_more() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        assert_eq!(first.track_id, 1);
        assert!(first.is_current());
        assert!(matches!(reads.start(Some(1)), Start::Unchanged));
        assert!(reads.shown_for(Some(1)));
        assert!(!reads.shown_for(Some(2)));
    }

    #[test]
    fn a_finished_overview_is_drawn_at_once_when_the_track_comes_back() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        lock(&first.slices).push(SLICE);
        assert!(reads.finish(first.epoch, 1, Ok(true)));

        job(reads.start(Some(2))).expect("the next track is read");
        assert!(job(reads.start(Some(1))).is_none(), "nothing to read again");
        assert_eq!(*reads.slices_drawn_at(10.0), [SLICE]);
    }

    #[test]
    fn an_overview_that_ends_after_the_track_changed_is_dropped() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        job(reads.start(Some(2))).expect("the next track is read");
        assert!(!first.is_current());

        lock(&first.slices).push(SLICE);
        assert!(!reads.finish(first.epoch, 1, Ok(true)));
        assert!(
            job(reads.start(Some(1))).is_some(),
            "a stale read is not kept"
        );
    }

    #[test]
    fn a_read_that_found_nothing_lets_the_oscilloscope_stand_in_and_is_tried_again() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        assert!(reads.finish(first.epoch, 1, Err("unreachable".to_owned())));
        assert!(!reads.shown_for(Some(1)));
        assert!(job(reads.start(Some(1))).is_some());
    }

    #[test]
    fn a_read_that_failed_midway_keeps_its_shape() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        lock(&first.slices).push(SLICE);
        assert!(reads.finish(first.epoch, 1, Err("cut off".to_owned())));
        assert!(reads.shown_for(Some(1)));
    }

    #[test]
    fn a_read_the_listener_moved_on_from_ends_without_a_redraw() {
        let mut reads = OverviewReads::default();
        let first = job(reads.start(Some(1))).expect("a read");
        assert!(!reads.finish(first.epoch, 1, Ok(false)));
    }

    #[test]
    fn the_shape_is_redrawn_only_when_the_length_moved_beyond_the_tolerance() {
        let mut reads = OverviewReads::default();
        job(reads.start(Some(1))).expect("a read");
        drop(reads.slices_drawn_at(100.0));
        assert!(!reads.redraw_needed(Some(1), 100.01));
        assert!(reads.redraw_needed(Some(1), 101.0));
        assert!(!reads.redraw_needed(Some(2), 101.0), "not shown, not drawn");
    }

    #[test]
    fn a_resize_reports_only_a_change() {
        let mut reads = OverviewReads::default();
        assert!(reads.resize((300, 40)));
        assert!(!reads.resize((300, 40)));
        assert_eq!(reads.pixel_size(), (300, 40));
    }

    /// Reads that answer as `answers` says, place by place: how many batches
    /// each yields before it ends with its outcome.
    fn reading(
        answers: &[(&'static str, usize, Result<bool, String>)],
        is_current: &dyn Fn() -> bool,
    ) -> (Option<Result<bool, String>>, Vec<&'static str>, usize) {
        let mut tried = Vec::new();
        let mut taken = 0;
        let locations: Vec<&'static str> = answers.iter().map(|(name, ..)| *name).collect();
        let outcome = read_first_answering(
            locations,
            is_current,
            &mut |location, on_batch| {
                tried.push(*location);
                let (_, batches, outcome) = answers
                    .iter()
                    .find(|(name, ..)| name == location)
                    .expect("a known place");
                for _ in 0..*batches {
                    if on_batch(&[SLICE]).is_break() {
                        return Ok(false);
                    }
                }
                outcome.clone()
            },
            &mut |batch| taken += batch.len(),
        );
        (outcome, tried, taken)
    }

    #[test]
    fn a_transcode_that_yields_no_audio_falls_back_to_the_original() {
        let (outcome, tried, taken) = reading(
            &[("transcode", 0, Ok(true)), ("original", 2, Ok(true))],
            &|| true,
        );
        assert_eq!(outcome, Some(Ok(true)));
        assert_eq!(tried, ["transcode", "original"]);
        assert_eq!(taken, 2);
    }

    #[test]
    fn a_place_that_fails_after_yielding_keeps_what_it_gave_and_nothing_else_is_tried() {
        let (outcome, tried, taken) = reading(
            &[
                ("transcode", 3, Err("cut off".to_owned())),
                ("original", 2, Ok(true)),
            ],
            &|| true,
        );
        assert_eq!(outcome, Some(Err("cut off".to_owned())));
        assert_eq!(tried, ["transcode"]);
        assert_eq!(taken, 3);
    }

    #[test]
    fn nowhere_to_read_from_is_a_failure_and_a_stale_read_tries_nothing() {
        let (outcome, tried, _) = reading(&[], &|| true);
        assert!(matches!(outcome, Some(Err(_))));
        assert!(tried.is_empty());

        let (outcome, tried, _) = reading(&[("original", 1, Ok(true))], &|| false);
        assert_eq!(outcome, None);
        assert!(tried.is_empty());
    }
}
