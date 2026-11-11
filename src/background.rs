//! Work that must not run on the user-interface thread: file reads, image
//! decoding, library scans.

use std::sync::mpsc::{channel, Sender};
use std::thread;

type Job = Box<dyn FnOnce() + Send>;

/// One named thread that runs the jobs it is handed, in order. A job reports
/// back by posting to the user-interface thread itself.
pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    pub fn new(name: &str) -> Result<Self, String> {
        let (jobs, queue) = channel::<Job>();
        thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                // Ends when the worker, and with it the sender, is dropped.
                for job in queue {
                    job();
                }
            })
            .map_err(|error| format!("start the {name} thread: {error}"))?;
        Ok(Self { jobs })
    }

    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        if self.jobs.send(Box::new(job)).is_err() {
            log::error!("a background thread ended early; its job was dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn jobs_run_in_the_order_they_were_handed_over() {
        let worker = Worker::new("test worker").expect("the thread starts");
        let (results, received) = channel();
        for number in 0..5 {
            let results = results.clone();
            worker.run(move || results.send(number).expect("the test is listening"));
        }
        let order: Vec<i32> = (0..5)
            .map(|_| {
                received
                    .recv_timeout(Duration::from_secs(5))
                    .expect("every job runs")
            })
            .collect();
        assert_eq!(order, [0, 1, 2, 3, 4]);
    }
}
