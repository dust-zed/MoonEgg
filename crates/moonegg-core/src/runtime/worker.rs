use crate::pipeline::PlaybackEpoch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerEvent {
    PreparationCompleted { epoch: PlaybackEpoch },
}
