use crate::{
    error::PlaybackError,
    player::{PlayerCommand, PlayerState},
};

#[derive(Debug)]
pub enum PlayerEvent {
    StateChanged {
        previous: PlayerState,
        current: PlayerState,
    },
    PlaybackFailed {
        error: PlaybackError,
    },
    CommandRejected {
        command: PlayerCommand,
        state: PlayerState,
    },
}
