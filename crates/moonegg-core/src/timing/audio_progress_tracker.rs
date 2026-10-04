use std::time::Duration;

use crate::{ports::AudioPlaybackPosition, timing::ClockError};

/// 音频进度观察器，判断播放头是否持续停滞
#[derive(Debug)]
pub(crate) struct AudioProgressTracker {
    stall_timeout: Duration,
    last_progress: Option<AudioPlaybackPosition>,
}

impl AudioProgressTracker {
    pub(crate) fn new(stall_timeout: Duration) -> Self {
        Self {
            stall_timeout,
            last_progress: None,
        }
    }

    pub(crate) fn observe(&mut self, position: AudioPlaybackPosition) -> Result<bool, ClockError> {
        let Some(last_progress) = self.last_progress else {
            self.last_progress = Some(position);
            return Ok(false);
        };
        if position.observed_at() < last_progress.observed_at() {
            return Err(ClockError::ObservationFromFuture);
        }

        if position.played_frames() < last_progress.played_frames() {
            return Err(ClockError::PositionWentBackward);
        }

        if position.played_frames() > last_progress.played_frames() {
            self.last_progress = Some(position);
            return Ok(false);
        }

        let stall_for = position
            .observed_at()
            .duration_since(last_progress.observed_at());

        Ok(stall_for >= self.stall_timeout)
    }

    pub(crate) fn reset(&mut self) {
        self.last_progress = None;
    }
}
