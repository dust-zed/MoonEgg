use crate::{error::PlaybackError, player::PlayerState};

#[derive(Debug)]
pub enum PlayerEvent {
    StateChanged {
        previous: PlayerState,
        current: PlayerState,
    },
    PlaybackFailed {
        error: PlaybackError,
    },
}
