use moonegg_core::{PlayerEvent, PlayerState};

#[derive(Debug, uniffi::Enum, Clone, Copy)]
pub enum NativeState {
    Idle,
    Preparing,
    Ready,
    Playing,
    Paused,
    Error,
    Ended,
    Released,
}

impl From<PlayerState> for NativeState {
    fn from(value: PlayerState) -> Self {
        match value {
            PlayerState::Idle => NativeState::Idle,
            PlayerState::Preparing => NativeState::Preparing,
            PlayerState::Ready => NativeState::Ready,
            PlayerState::Playing => NativeState::Playing,
            PlayerState::Paused => NativeState::Paused,
            PlayerState::Error => NativeState::Error,
            PlayerState::Ended => NativeState::Ended,
            PlayerState::Released => NativeState::Released,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum NativeEvent {
    StateChanged {
        previous: NativeState,
        current: NativeState,
    },
    DurationChanged {
        duration_ms: Option<i64>,
    },
    AudioProgress {
        position_ms: i64,
    },
    PlaybackFailed {
        reason: String,
    },
    CommandRejected {
        reason: String,
    },
}

impl From<PlayerEvent> for NativeEvent {
    fn from(value: PlayerEvent) -> Self {
        match value {
            PlayerEvent::StateChanged { previous, current } => NativeEvent::StateChanged {
                previous: previous.into(),
                current: current.into(),
            },
            PlayerEvent::DurationChanged { duration_ms } => {
                NativeEvent::DurationChanged { duration_ms }
            }
            PlayerEvent::AudioProgress { media_time } => NativeEvent::AudioProgress {
                position_ms: media_time.nanoseconds() / 1_000_000,
            },
            PlayerEvent::PlaybackFailed { error } => NativeEvent::PlaybackFailed {
                reason: format!("{error:?}"),
            },
            PlayerEvent::CommandRejected { command, state } => NativeEvent::CommandRejected {
                reason: format!("{command:?} rejected in {state:?}"),
            },
        }
    }
}
