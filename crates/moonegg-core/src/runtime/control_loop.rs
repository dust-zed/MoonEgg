use std::sync::mpsc::{Receiver, Sender};

use crate::{
    media::MediaTime,
    pipeline::{EpochError, PlaybackEpoch},
    player::{InvalidTransition, PlayerCommand, PlayerEvent, PlayerState, StateAction},
    runtime::{player_controller::PlayerController, worker::WorkerEvent},
};

pub type ControlResult = Result<ControlOutcome, ControlError>;

pub struct ControlLoop {
    controller: PlayerController,
}

impl ControlLoop {
    pub fn new() -> Self {
        Self {
            controller: PlayerController::new(),
        }
    }

    // 这里使用 mut self 而不是 &mut self 很重要：
    // 启动控制线程时，把整个 ControlLoop 所有权移动进线程，从此只有该线程能访问状态。
    pub fn run(
        mut self,
        message_receiver: Receiver<ControlMessage>,
        result_sender: Sender<ControlResult>,
    ) -> ControlLoopExit {
        loop {
            let message = match message_receiver.recv() {
                Ok(message) => message,
                Err(_) => {
                    return ControlLoopExit::InputDisconnected;
                }
            };

            let result = self.handle_message(message);

            let fatal = matches!(
                &result,
                Err(ControlError::UnexpectedWorkEvent { .. } | ControlError::Epoch(_))
            );

            let released = self.controller.state() == PlayerState::Released;

            if result_sender.send(result).is_err() {
                return ControlLoopExit::OutputDisconnected;
            }

            if fatal {
                return ControlLoopExit::FatalControlError;
            }

            if released {
                return ControlLoopExit::Released;
            }
        }
    }

    pub fn handle_message(
        &mut self,
        message: ControlMessage,
    ) -> Result<ControlOutcome, ControlError> {
        match message {
            ControlMessage::Command(command) => self.controller.handle_command(command),
            ControlMessage::WorkerEvent(event) => self.controller.handle_worker_event(event),
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlLoopExit {
    Released,
    InputDisconnected,
    OutputDisconnected,
    FatalControlError,
}
