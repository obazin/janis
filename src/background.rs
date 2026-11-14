//! Work that must not run on the user-interface thread: file reads, image
//! decoding, library scans, cover loads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
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
                    run_guarded(job);
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

/// Named threads that take the jobs handed to them from one shared queue:
/// at most as many run at once as there are threads.
pub struct WorkerPool {
    jobs: Sender<Job>,
}

impl WorkerPool {
    pub fn new(name: &str, count: usize) -> Result<Self, String> {
        let (jobs, queue) = channel::<Job>();
        let queue = Arc::new(Mutex::new(queue));
        for number in 1..=count {
            let queue = Arc::clone(&queue);
            thread::Builder::new()
                .name(format!("{name} {number}"))
                .spawn(move || {
                    // Ends when the pool, and with it the sender, is dropped.
                    while let Some(job) = next_job(&queue) {
                        run_guarded(job);
                    }
                })
                .map_err(|error| format!("start the {name} threads: {error}"))?;
        }
        Ok(Self { jobs })
    }

    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        if self.jobs.send(Box::new(job)).is_err() {
            log::error!("the background threads ended early; a job was dropped");
        }
    }
}

/// Runs a job, and keeps its thread alive if it panics: the next jobs still
/// run. The job's own result is never posted, and the panic is logged.
fn run_guarded(job: Job) {
    if catch_unwind(AssertUnwindSafe(job)).is_err() {
        let thread = thread::current();
        log::error!(
            "a job on the {} thread panicked; the thread goes on with the next",
            thread.name().unwrap_or("background")
        );
    }
}

/// The queue is held only while a job is taken, never while it runs.
fn next_job(queue: &Mutex<Receiver<Job>>) -> Option<Job> {
    queue.lock().ok()?.recv().ok()
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

    #[test]
    fn a_pool_runs_as_many_jobs_at_once_as_it_has_threads() {
        let pool = WorkerPool::new("test pool", 3).expect("the threads start");
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let (results, received) = channel();
        for number in 0..3 {
            let barrier = Arc::clone(&barrier);
            let results = results.clone();
            pool.run(move || {
                // Passes only once all three jobs run together.
                barrier.wait();
                results.send(number).expect("the test is listening");
            });
        }
        let mut finished: Vec<i32> = (0..3)
            .map(|_| {
                received
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the three jobs run at once")
            })
            .collect();
        finished.sort_unstable();
        assert_eq!(finished, [0, 1, 2]);
    }

    #[test]
    fn a_job_that_panics_does_not_stop_the_jobs_after_it() {
        let worker = Worker::new("test worker").expect("the thread starts");
        worker.run(|| panic!("a job fails"));
        let (results, received) = channel();
        worker.run(move || results.send("ran").expect("the test is listening"));
        assert_eq!(received.recv_timeout(Duration::from_secs(5)), Ok("ran"));
    }

    #[test]
    fn a_pool_keeps_every_thread_after_its_jobs_panic() {
        let pool = WorkerPool::new("test pool", 2).expect("the threads start");
        for _ in 0..2 {
            pool.run(|| panic!("a job fails"));
        }
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (results, received) = channel();
        for number in 0..2 {
            let barrier = Arc::clone(&barrier);
            let results = results.clone();
            pool.run(move || {
                // Passes only if both threads are still there to run it.
                barrier.wait();
                results.send(number).expect("the test is listening");
            });
        }
        for _ in 0..2 {
            received
                .recv_timeout(Duration::from_secs(5))
                .expect("both threads still run jobs");
        }
    }
}
