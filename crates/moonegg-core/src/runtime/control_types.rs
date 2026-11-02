use crate::{
    PlayerCommand, PlayerEvent,
    media::MediaTime,
    pipeline::{EpochError, PlaybackEpoch},
    player::InvalidTransition,
    runtime::worker::WorkerEvent,
};

pub type ControlResult = Result<ControlOutcome, ControlError>;

#[derive(Debug)]
pub enum ControlMessage {
    Command(PlayerCommand),
    WorkerEvent(WorkerEvent),
}

#[derive(Debug)]
pub enum ControlEffect {
    BeginPrepare {
        epoch: PlaybackEpoch,
    },
    StartPlayback {
        epoch: PlaybackEpoch,
    },
    PausePlayback {
        epoch: PlaybackEpoch,
    },
    Seek {
        target: MediaTime,
        epoch: PlaybackEpoch,
        resume_after_seek: bool,
    },
    Stop {
        epoch: PlaybackEpoch,
    },
    Release {
        epoch: PlaybackEpoch,
    },
    CleanupAfterFailure {
        epoch: PlaybackEpoch,
    },
}

impl ControlEffect {
    pub const fn epoch(&self) -> PlaybackEpoch {
        match self {
            Self::BeginPrepare { epoch }
            | Self::StartPlayback { epoch }
            | Self::PausePlayback { epoch }
            | Self::Seek { epoch, .. }
            | Self::Stop { epoch }
            | Self::Release { epoch }
            | Self::CleanupAfterFailure { epoch } => *epoch,
        }
    }
}

#[derive(Debug)]
pub struct ControlOutcome {
    pub effect: Option<ControlEffect>,
    pub event: Option<PlayerEvent>,
}

impl ControlOutcome {
    pub fn none() -> Self {
        Self {
            effect: None,
            event: None,
        }
    }
}

#[derive(Debug)]
pub enum ControlError {
    CommandRejected {
        command: PlayerCommand,
        transition: InvalidTransition,
    },
    UnexpectedWorkEvent {
        transition: InvalidTransition,
    },
    Epoch(EpochError),
}
