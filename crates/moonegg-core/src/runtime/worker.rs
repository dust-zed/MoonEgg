use crate::{error::PlaybackError, pipeline::PlaybackEpoch};

#[derive(Debug)]
pub enum WorkerEvent {
    PreparationCompleted {
        epoch: PlaybackEpoch,
    },
    Failed {
        epoch: PlaybackEpoch,
        error: PlaybackError,
    },
}
