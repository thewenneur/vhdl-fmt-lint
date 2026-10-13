//! A background thread that owns the [`Engine`], so that loading projects
//! and analyzing them never blocks the editor.
//!
//! The editor submits jobs, each with a copy of the buffer it is about,
//! and gets their results back through [`Worker::results`]; the `notify`
//! callback tells it when one is ready. A job with a [`Key`] supersedes the
//! jobs with the same key still waiting: only the latest lint, hover or
//! completion request runs. Jobs without a key (closing a buffer, reloading)
//! always run, in order.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::engine::Engine;

/// What a job is for. A new job replaces the waiting one with its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Lint,
    /// Hover, or Show type.
    Info,
    Parameters,
    Completion,
    Highlight,
    /// A menu command: navigation, symbols, fixes, rename, code generation.
    Command,
    /// Checking the files on disk for changes.
    Rescan,
}

const KEYS: usize = 7;

impl Key {
    fn index(self) -> usize {
        self as usize
    }
}

type Work = Box<dyn FnOnce(&mut Engine) -> Box<dyn Any + Send> + Send>;

struct Job {
    seq: u64,
    key: Option<Key>,
    work: Work,
}

/// A finished job.
pub struct Done {
    pub seq: u64,
    /// The job's result, or the message of the panic it hit.
    pub value: Result<Box<dyn Any + Send>, String>,
}

pub struct Worker {
    jobs: Sender<Job>,
    results: Receiver<Done>,
    /// The latest job submitted with each key.
    latest: Arc<[AtomicU64; KEYS]>,
    next: u64,
}

impl Worker {
    /// Starts the thread. `notify` is called on it after each result.
    pub fn start(notify: impl Fn() + Send + 'static) -> Worker {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, results) = mpsc::channel();
        let latest: Arc<[AtomicU64; KEYS]> = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
        let superseded = latest.clone();
        std::thread::Builder::new()
            .name("vfl".into())
            .spawn(move || {
                let mut engine = Engine::default();
                for job in inbox {
                    if job
                        .key
                        .is_some_and(|k| superseded[k.index()].load(Ordering::Acquire) != job.seq)
                    {
                        continue;
                    }
                    let value = catch_unwind(AssertUnwindSafe(|| (job.work)(&mut engine)))
                        .map_err(|payload| panic_message(&*payload));
                    if outbox
                        .send(Done {
                            seq: job.seq,
                            value,
                        })
                        .is_err()
                    {
                        break;
                    }
                    notify();
                }
            })
            .expect("the vfl thread starts");
        Worker {
            jobs,
            results,
            latest,
            next: 0,
        }
    }

    /// Queues `work`, replacing the waiting job with the same key. Returns
    /// the job's sequence number, which its [`Done`] carries.
    pub fn submit<T: Send + 'static>(
        &mut self,
        key: Option<Key>,
        work: impl FnOnce(&mut Engine) -> T + Send + 'static,
    ) -> u64 {
        self.next += 1;
        let seq = self.next;
        if let Some(key) = key {
            self.latest[key.index()].store(seq, Ordering::Release);
        }
        let work: Work = Box::new(move |engine| Box::new(work(engine)));
        // The thread only stops when the worker is dropped.
        let _ = self.jobs.send(Job { seq, key, work });
        seq
    }

    /// The next finished job, if any.
    pub fn result(&self) -> Option<Done> {
        self.results.try_recv().ok()
    }
}

pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    fn wait(worker: &Worker, ready: &Receiver<()>) -> Done {
        loop {
            if let Some(done) = worker.result() {
                return done;
            }
            match ready.recv_timeout(Duration::from_secs(10)) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => panic!("no result"),
                Err(RecvTimeoutError::Disconnected) => panic!("worker stopped"),
            }
        }
    }

    fn started() -> (Worker, Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::start(move || {
            let _ = tx.send(());
        });
        (worker, rx)
    }

    #[test]
    fn engine_moves_to_the_worker() {
        fn send<T: Send>() {}
        send::<Engine>();
    }

    #[test]
    fn runs_jobs_off_the_calling_thread() {
        let (mut worker, ready) = started();
        let caller = std::thread::current().id();
        let seq = worker.submit(None, move |engine| {
            assert_ne!(std::thread::current().id(), caller);
            engine
                .lint(None, "entity e is\nend entity;\narchitecture a of e is\n  signal s : bit;\nbegin\nend;\n")
                .findings
                .len()
        });
        let done = wait(&worker, &ready);
        assert_eq!(done.seq, seq);
        assert_eq!(*done.value.unwrap().downcast::<usize>().unwrap(), 1);
    }

    #[test]
    fn newer_jobs_supersede_waiting_ones() {
        let (mut worker, ready) = started();
        // Hold the thread until the queue is full.
        let (release, hold) = mpsc::channel::<()>();
        let first = worker.submit(None, move |_| hold.recv().is_ok());
        let _stale = worker.submit(Some(Key::Lint), |_| "stale");
        let other = worker.submit(Some(Key::Info), |_| "other");
        let latest = worker.submit(Some(Key::Lint), |_| "latest");
        release.send(()).unwrap();
        let seqs: Vec<u64> = (0..3).map(|_| wait(&worker, &ready).seq).collect();
        assert_eq!(seqs, [first, other, latest]);
    }

    #[test]
    fn survives_panics() {
        let (mut worker, ready) = started();
        worker.submit(None, |_| -> () { panic!("boom") });
        assert_eq!(wait(&worker, &ready).value.err().as_deref(), Some("boom"));
        worker.submit(None, |_| 1u8);
        assert!(wait(&worker, &ready).value.is_ok());
    }
}
