//! Two detached IO workers. No Core reference and no blocking join on shutdown.

use super::{Completed, Job};
use std::io;
use std::sync::{Arc, Mutex, mpsc};

pub(super) struct Pool {
    jobs: mpsc::SyncSender<Job>,
    pub results: mpsc::Receiver<Completed>,
}

impl Pool {
    pub fn new(run: impl Fn(Job) -> Completed + Send + Sync + 'static) -> io::Result<Self> {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(2);
        let (results, received) = mpsc::channel();
        let incoming = Arc::new(Mutex::new(incoming));
        let run = Arc::new(run);
        for _ in 0..2 {
            let (incoming, results, run) = (incoming.clone(), results.clone(), run.clone());
            // Dropping the handle detaches it. A filesystem call may never return.
            std::thread::Builder::new().name("repository".into()).spawn(move || {
                loop {
                    let job = incoming.lock().unwrap_or_else(|p| p.into_inner()).recv();
                    let Ok(job) = job else { break };
                    if results.send(run(job)).is_err() {
                        break;
                    }
                }
            })?;
        }
        Ok(Self { jobs, results: received })
    }

    pub fn submit(&self, job: Job) -> Result<(), Job> {
        self.jobs.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
        })
    }
}
