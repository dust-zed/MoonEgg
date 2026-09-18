use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use crate::{
    error::{self, PlaybackError},
    media::MediaTime,
    pipeline::PlaybackEpoch,
};

#[derive(Debug)]
pub enum WorkerEvent {
    PreparationCompleted {
        epoch: PlaybackEpoch,
    },
    Failed {
        epoch: PlaybackEpoch,
        error: PlaybackError,
    },
    AudioProgress {
        epoch: PlaybackEpoch,
        media_time: MediaTime,
    },
    PlaybackCompleted {
        epoch: PlaybackEpoch,
    },
}

#[derive(Debug)]
pub struct WorkerHandle {
    cancel: CancellationToken,
    thread: JoinHandle<()>,
}

impl WorkerHandle {
    pub fn new(cancel: CancellationToken, thread: JoinHandle<()>) -> Self {
        Self { cancel, thread }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn join(self) -> Result<(), WorkerJoinError> {
        self.thread.join().map_err(|_| WorkerJoinError::Panicked)
    }
}

#[derive(Debug, Default)]
pub struct WorkerGroup {
    workers: Vec<WorkerHandle>,
}

impl WorkerGroup {
    pub fn push(&mut self, worker: WorkerHandle) {
        self.workers.push(worker);
    }

    pub fn cancel_and_join_all(&mut self) -> Result<(), WorkerJoinError> {
        for worker in &self.workers {
            worker.cancel();
        }

        let workers = std::mem::take(&mut self.workers);
        let mut first_error = None;

        for worker in workers {
            if let Err(error) = worker.join()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_canceled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
pub enum WorkerJoinError {
    Panicked,
}
