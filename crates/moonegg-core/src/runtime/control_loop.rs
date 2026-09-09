use crate::{
    media::MediaTime,
    pipeline::{EpochError, PlaybackEpoch},
    player::{InvalidTransition, PlayerCommand, PlayerState, StateAction},
};

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

    pub fn handle_command(
        &mut self,
        command: PlayerCommand,
    ) -> Result<ControlEffect, ControlError> {
        let action = match command {
            PlayerCommand::Prepare => StateAction::BeginPrepare,
            PlayerCommand::Play => StateAction::Play,
            PlayerCommand::Pause => StateAction::Pause,
            PlayerCommand::Seek(_) => StateAction::Seek,
            PlayerCommand::Stop => StateAction::Stop,
            PlayerCommand::Release => StateAction::Release,
        };

        let previous = self.state;
        let next = previous
            .transition(action)
            .map_err(ControlError::InvalidTransition)?;
        self.state = next;

        let repeated_command = previous == next && !matches!(command, PlayerCommand::Seek(_));
        if repeated_command {
            return Ok(ControlEffect::None);
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
        Ok(effect)
    }
}

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
    None,
}

pub enum ControlError {
    InvalidTransition(InvalidTransition),
    Epoch(EpochError),
}
