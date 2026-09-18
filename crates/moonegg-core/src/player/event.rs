use crate::{
    error::PlaybackError,
    media::MediaTime,
    pipeline::PlaybackEpoch,
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
        media_time: MediaTime,
    },
    CommandRejected {
        command: PlayerCommand,
        state: PlayerState,
    },
}
