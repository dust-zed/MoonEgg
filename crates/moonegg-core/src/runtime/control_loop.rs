use std::sync::mpsc::{Receiver, Sender};

use crate::{
    media::MediaTime,
    pipeline::{EpochError, PlaybackEpoch},
    player::{InvalidTransition, PlayerCommand, PlayerEvent, PlayerState, StateAction},
    runtime::worker::WorkerEvent,
};

pub type ControlResult = Result<ControlOutcome, ControlError>;

pub struct ControlLoop {
    state: PlayerState,
    epoch: PlaybackEpoch,
}

impl ControlLoop {
    pub fn new() -> Self {
        Self {
            state: PlayerState::Idle,
            epoch: PlaybackEpoch::INITIAL,
        }
    }

    pub fn state(&self) -> PlayerState {
        self.state
    }

    pub const fn epoch(&self) -> PlaybackEpoch {
        self.epoch
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

            let released = self.state == PlayerState::Released;

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
            ControlMessage::Command(command) => self.handle_command(command),
            ControlMessage::WorkerEvent(event) => self.handle_worker_event(event),
        }
    }

    pub fn handle_command(
        &mut self,
        command: PlayerCommand,
    ) -> Result<ControlOutcome, ControlError> {
        let action = match command {
            PlayerCommand::Prepare => StateAction::BeginPrepare,
            PlayerCommand::Play => StateAction::Play,
            PlayerCommand::Pause => StateAction::Pause,
            PlayerCommand::Seek(_) => StateAction::Seek,
            PlayerCommand::Stop => StateAction::Stop,
            PlayerCommand::Release => StateAction::Release,
        };

        let previous = self.state;
        let next =
            previous
                .transition(action)
                .map_err(|transition| ControlError::CommandRejected {
                    command,
                    transition,
                })?;

        let repeated_command = previous == next && !matches!(command, PlayerCommand::Seek(_));
        if repeated_command {
            return Ok(ControlOutcome::none());
        }

        let invalidates_pipeline = matches!(
            command,
            PlayerCommand::Seek(_) | PlayerCommand::Stop | PlayerCommand::Release
        );

        let next_epoch = if invalidates_pipeline {
            self.epoch().next().map_err(ControlError::Epoch)?
        } else {
            self.epoch
        };

        self.state = next;
        self.epoch = next_epoch;

        let effect = match command {
            PlayerCommand::Prepare => ControlEffect::BeginPrepare { epoch: next_epoch },
            PlayerCommand::Play => ControlEffect::StartPlayback,
            PlayerCommand::Pause => ControlEffect::PausePlayback,
            PlayerCommand::Seek(target) => ControlEffect::Seek {
                target,
                epoch: next_epoch,
            },
            PlayerCommand::Stop => ControlEffect::Stop { epoch: next_epoch },
            PlayerCommand::Release => ControlEffect::Release { epoch: next_epoch },
        };
        let event = if previous != next {
            Some(PlayerEvent::StateChanged {
                previous,
                current: next,
            })
        } else {
            None
        };
        Ok(ControlOutcome {
            effect: Some(effect),
            event,
        })
    }

    pub fn handle_worker_event(
        &mut self,
        event: WorkerEvent,
    ) -> Result<ControlOutcome, ControlError> {
        match event {
            WorkerEvent::PreparationCompleted { epoch } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }

                let previous = self.state;

                let next = previous
                    .transition(StateAction::PreparationCompleted)
                    .map_err(|transition| ControlError::UnexpectedWorkEvent { transition })?;

                self.state = next;
                Ok(ControlOutcome {
                    effect: None,
                    event: Some(PlayerEvent::StateChanged {
                        previous,
                        current: next,
                    }),
                })
            }
            WorkerEvent::Failed { epoch, error } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }

                let next_state = self
                    .state
                    .transition(StateAction::FatalError)
                    .map_err(|transition| ControlError::UnexpectedWorkEvent { transition })?;

                let next_epoch = self.epoch.next().map_err(ControlError::Epoch)?;
                self.state = next_state;
                self.epoch = next_epoch;

                Ok(ControlOutcome {
                    effect: Some(ControlEffect::CleanupAfterFailure { epoch: next_epoch }),
                    event: Some(PlayerEvent::PlaybackFailed { error }),
                })
            }
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
    StartPlayback,
    PausePlayback,
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
