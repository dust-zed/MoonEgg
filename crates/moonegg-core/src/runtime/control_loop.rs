use crate::{
    media::MediaTime,
    player::{InvalidTransition, PlayerCommand, PlayerState, StateAction},
};

pub struct ControlLoop {
    state: PlayerState,
}

impl ControlLoop {
    pub fn new() -> Self {
        Self {
            state: PlayerState::Idle,
        }
    }

    pub fn state(&self) -> PlayerState {
        self.state
    }

    pub fn handle_command(
        &mut self,
        command: PlayerCommand,
    ) -> Result<ControlEffect, InvalidTransition> {
        let (action, effect) = match command {
            PlayerCommand::Prepare => (StateAction::BeginPrepare, ControlEffect::BeginPrepare),
            PlayerCommand::Play => (StateAction::Play, ControlEffect::StartPlayback),
            PlayerCommand::Pause => (StateAction::Pause, ControlEffect::PausePlayback),
            PlayerCommand::Seek(target) => (StateAction::Seek, ControlEffect::Seek(target)),
            PlayerCommand::Stop => (StateAction::Stop, ControlEffect::Stop),
            PlayerCommand::Release => (StateAction::Release, ControlEffect::Release),
        };
        let previous = self.state;
        let next = previous.transition(action)?;
        self.state = next;

        if previous == next && !matches!(command, PlayerCommand::Seek(_)) {
            return Ok(ControlEffect::None);
        }
        Ok(effect)
    }
}

pub enum ControlEffect {
    BeginPrepare,
    StartPlayback,
    PausePlayback,
    Seek(MediaTime),
    Stop,
    Release,
    None,
}
