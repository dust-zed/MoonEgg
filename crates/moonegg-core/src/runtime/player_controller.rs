use crate::{
    PlayerCommand, PlayerEvent, PlayerState,
    pipeline::PlaybackEpoch,
    player::StateAction,
    runtime::{ControlEffect, ControlError, ControlOutcome, worker::WorkerEvent},
};

pub(crate) struct PlayerController {
    state: PlayerState,
    epoch: PlaybackEpoch,
}

impl PlayerController {
    pub(crate) fn new() -> Self {
        Self {
            state: PlayerState::Idle,
            epoch: PlaybackEpoch::INITIAL,
        }
    }

    pub(crate) fn state(&self) -> PlayerState {
        self.state
    }

    pub(crate) fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    pub(crate) fn handle_command(
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
            self.epoch.next().map_err(ControlError::Epoch)?
        } else {
            self.epoch
        };

        self.state = next;
        self.epoch = next_epoch;

        let effect = match command {
            PlayerCommand::Prepare => ControlEffect::BeginPrepare { epoch: next_epoch },
            PlayerCommand::Play => ControlEffect::StartPlayback { epoch: next_epoch },
            PlayerCommand::Pause => ControlEffect::PausePlayback { epoch: next_epoch },
            PlayerCommand::Seek(target) => ControlEffect::Seek {
                target,
                epoch: next_epoch,
                resume_after_seek: next == PlayerState::Playing,
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

    pub(crate) fn handle_worker_event(
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
            WorkerEvent::DurationChanged { epoch, duration_ms } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }
                Ok(ControlOutcome {
                    effect: None,
                    event: Some(PlayerEvent::DurationChanged { duration_ms }),
                })
            }
            WorkerEvent::AudioProgress { epoch, media_time } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }
                Ok(ControlOutcome {
                    effect: None,
                    event: Some(PlayerEvent::AudioProgress { media_time }),
                })
            }
            WorkerEvent::SeekCompleted {
                epoch,
                requested,
                landed,
            } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }
                Ok(ControlOutcome {
                    effect: None,
                    event: Some(PlayerEvent::SeekCompleted { requested, landed }),
                })
            }
            WorkerEvent::PlaybackCompleted { epoch } => {
                if epoch != self.epoch {
                    return Ok(ControlOutcome::none());
                }
                let previous = self.state;
                let next = previous
                    .transition(StateAction::PlaybackCompleted)
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
                let previous = self.state;
                let next = previous
                    .transition(StateAction::FatalError)
                    .map_err(|transition| ControlError::UnexpectedWorkEvent { transition })?;
                let next_epoch = self.epoch.next().map_err(ControlError::Epoch)?;

                self.state = next;
                self.epoch = next_epoch;

                Ok(ControlOutcome {
                    effect: Some(ControlEffect::CleanupAfterFailure { epoch: next_epoch }),
                    event: Some(PlayerEvent::PlaybackFailed { error }),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_ignores_late_preparation_completed() {
        let mut control = PlayerController::new();

        let prepare_outcome = control
            .handle_command(PlayerCommand::Prepare)
            .expect("Prepare 应成功");

        let prepare_epoch = control.epoch();
        assert_eq!(control.state(), PlayerState::Preparing);
        assert!(matches!(
            prepare_outcome.effect,
            Some(ControlEffect::BeginPrepare { epoch })
            if epoch == prepare_epoch
        ));

        let release_outcome = control
            .handle_command(PlayerCommand::Release)
            .expect("Release 应成功");

        let release_epoch = control.epoch();
        assert_eq!(control.state(), PlayerState::Released);
        assert!(matches!(
            release_outcome.effect,
            Some(ControlEffect::Release { epoch })
            if epoch == release_epoch
        ));

        let delayed_event = WorkerEvent::PreparationCompleted {
            epoch: prepare_epoch,
        };

        let delayed_outcome = control
            .handle_worker_event(delayed_event)
            .expect("旧事件应该忽略");

        assert!(delayed_outcome.effect.is_none());
        assert!(delayed_outcome.event.is_none());
        assert_eq!(PlayerState::Released, control.state());
        assert_eq!(control.epoch(), release_epoch);
    }
}
