use crate::{
    error::PlaybackError,
    media::MediaTime,
    player::{PlayerCommand, PlayerState},
};

#[derive(Debug)]
pub enum PlayerEvent {
    StateChanged {
        previous: PlayerState,
        current: PlayerState,
    },
    DurationChanged {
        duration_ms: Option<i64>,
    },
    PlaybackFailed {
        error: PlaybackError,
    },
    AudioProgress {
        media_time: MediaTime,
    },
    SeekCompleted {
        requested: MediaTime,
        landed: MediaTime,
    },
    CommandRejected {
        command: PlayerCommand,
        state: PlayerState,
    },
}
