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
    AudioProgress {
        played_frames: u64,
    },
    CommandRejected {
        command: PlayerCommand,
        state: PlayerState,
    },
}
