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
use crate::library::{self, Gains, Track};

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
        Ok(Self {
            worker: Worker::new("janis loudness")?,
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
    /// Measures, one after another on the loudness thread, every file in the
    /// library that has no gain yet. Called at start-up and after each scan;
    /// a call made while a measurement runs waits behind it and then finds
    /// only what is still missing.
    pub(super) fn measure_loudness(&self) {
        let database = Arc::clone(&self.database);
        let tried = Arc::clone(&self.loudness.tried);
        let stopping = Arc::clone(&self.loudness.stopping);
        self.loudness.worker.run(move || {
            let pending = match library::tracks_needing_loudness(&database) {
                Ok(pending) => pending,
                Err(error) => {
                    log::warn!("loudness measurement: {error}");
                    return;
                }
            };
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
                        // The database lock is held only for this one write.
                        library::store_loudness(
                            &database,
                            track.track_id,
                            measured.lufs,
                            measured.peak,
                        );
                        let gain = library::resolved_gain(
                            Gains::default(),
                            Some(measured.lufs),
                            Some(measured.peak),
                        );
                        let track_id = track.track_id;
                        post(move |application| {
                            application
                                .loudness
                                .measured_gains
                                .borrow_mut()
                                .insert(track_id, gain);
                        });
                    }
                    Err(error) => log::warn!("loudness measurement: {error}"),
                }
            }
        });
    }
}
