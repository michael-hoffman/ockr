//! Background thread pool for dispatching plugin commands.

use std::sync::{Arc, Mutex, mpsc};

use super::runtime::PluginInstance;

pub struct PluginJob {
    pub instance: Arc<Mutex<PluginInstance>>,
    pub command_id: String,
}

pub struct PluginThreadPool {
    tx: mpsc::SyncSender<PluginJob>,
}

impl PluginThreadPool {
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel::<PluginJob>(64);
        let rx = Arc::new(Mutex::new(rx));

        for _ in 0..threads {
            let rx = Arc::clone(&rx);
            std::thread::spawn(move || loop {
                let job = {
                    let guard = rx.lock().unwrap();
                    guard.recv()
                };
                match job {
                    Ok(job) => {
                        if let Ok(mut inst) = job.instance.lock() {
                            inst.dispatch(&job.command_id);
                        }
                    }
                    Err(_) => break, // channel closed
                }
            });
        }

        Self { tx }
    }

    /// Submit a dispatch job. Returns `Err(job)` if the queue is full.
    pub fn dispatch(&self, job: PluginJob) -> Result<(), PluginJob> {
        self.tx.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(j) | mpsc::TrySendError::Disconnected(j) => j,
        })
    }
}
