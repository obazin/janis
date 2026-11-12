//! The controller's side of loudness normalization: measuring, in the
//! background, the files that carry no gain, and playing each at its new gain
//! as soon as it is known.
//!
//! The library holds every gain the database knew when it was read; a gain
//! measured since is kept in `measured_gains` for the rest of the session, at
//! most one number per local file, and a queue takes it from there.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::{post, Application};
use crate::audio;
use crate::background::Worker;
use crate::library::{self, Gains, PendingLoudness, Track};
use crate::platform;

/// What the controller keeps for the background measurement.
pub(super) struct LoudnessMeasurement {
    /// One thread of its own, so a long measurement never holds up a scan,
    /// and a scan never waits behind one.
    worker: Worker,
    /// The gains measured this session, by track id.
    measured_gains: std::cell::RefCell<HashMap<i64, f64>>,
    /// The tracks tried this session, measured or not, so a file that cannot
    /// be measured is not read again at every scan.
    tried: Arc<Mutex<HashSet<i64>>>,
    /// Raised at shut-down: the measurement stops between two files.
    stopping: Arc<AtomicBool>,
}

impl LoudnessMeasurement {
    pub fn new() -> Result<Self, String> {
        let worker = Worker::new("janis loudness")?;
        // Measuring can take a whole processor core for minutes after music is
        // added; at the lowest priority it yields to playback, the interface
        // and everything else the computer is doing.
        worker.run(platform::lower_current_thread_priority);
        Ok(Self {
            worker,
            measured_gains: Default::default(),
            tried: Default::default(),
            stopping: Default::default(),
        })
    }

    /// The gain `track` plays at: the one measured this session, when there
    /// is one, else the one the library read.
    pub fn gain_of(&self, track: &Track) -> f64 {
        self.measured_gains
            .borrow()
            .get(&track.id)
            .copied()
            .unwrap_or(track.gain_db)
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }
}

impl Application {
    /// Measures every file in the library that has no gain yet. Called at
    /// start-up and after each scan.
    ///
    /// The loudness thread runs at the lowest priority, where its disk access
    /// gives way to every other thread's, so it never takes the database lock
    /// the engine also waits on: the library thread finds the files and
    /// stores each result. A call made while a measurement runs queues behind
    /// it, and skips the files that one has already tried.
    pub(super) fn measure_loudness(&self) {
        let database = Arc::clone(&self.database);
        self.library_worker
            .run(move || match library::tracks_needing_loudness(&database) {
                Ok(pending) => post(move |application| application.measure_files(pending)),
                Err(error) => log::warn!("loudness measurement: {error}"),
            });
    }

    /// Measures `pending`, one file after another, on the loudness thread.
    fn measure_files(&self, pending: Vec<PendingLoudness>) {
        let tried = Arc::clone(&self.loudness.tried);
        let stopping = Arc::clone(&self.loudness.stopping);
        self.loudness.worker.run(move || {
            for track in pending {
                if stopping.load(Ordering::Relaxed) {
                    return;
                }
                // Each track is tried once a session, whatever comes of it.
                let first_try = tried
                    .lock()
                    .map(|mut tried| tried.insert(track.track_id))
                    .unwrap_or(false);
                if !first_try {
                    continue;
                }
                match audio::measure_file(Path::new(&track.path)) {
                    Ok(measured) => {
                        let track_id = track.track_id;
                        post(move |application| application.loudness_measured(track_id, measured));
                    }
                    Err(error) => log::warn!("loudness measurement: {error}"),
                }
            }
        });
    }

    /// Plays `track_id` at its measured gain from the next queue on, and
    /// stores the measurement from the library thread.
    fn loudness_measured(&self, track_id: i64, measured: audio::Measured) {
        let gain =
            library::resolved_gain(Gains::default(), Some(measured.lufs), Some(measured.peak));
        self.loudness
            .measured_gains
            .borrow_mut()
            .insert(track_id, gain);
        let database = Arc::clone(&self.database);
        self.library_worker.run(move || {
            library::store_loudness(&database, track_id, measured.lufs, measured.peak);
        });
    }
}
