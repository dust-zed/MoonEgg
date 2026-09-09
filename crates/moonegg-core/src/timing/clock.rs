use std::time::Instant;

use crate::{
    media::{MediaDelta, MediaTime},
    ports::AudioPlaybackPosition,
};

/// 在单调时钟的 `observed_at`时刻，音频播放头对应的媒体时间是`media_time`
#[derive(Debug, Clone, Copy)]
pub struct ClockSnapshot {
    media_time: MediaTime,
    observed_at: Instant,
}

impl ClockSnapshot {
    pub const fn new(media_time: MediaTime, observed_at: Instant) -> Self {
        Self {
            media_time,
            observed_at,
        }
    }
    pub const fn media_time(self) -> MediaTime {
        self.media_time
    }

    pub const fn observed_at(self) -> Instant {
        self.observed_at
    }
}

#[derive(Debug)]
pub struct AudioClock {
    sample_rate: u32,
    anchor_media: MediaTime,   // 媒体时间
    anchor_played_frames: u64, // 设备累计播放 frame，考虑 seek 情况
}

impl AudioClock {
    pub fn new(
        sample_rate: u32,
        anchor_media: MediaTime,
        anchor_played_frames: u64,
    ) -> Result<Self, ClockError> {
        if sample_rate == 0 {
            return Err(ClockError::InvalidSampleRate);
        }
        Ok(Self {
            sample_rate,
            anchor_media,
            anchor_played_frames,
        })
    }

    pub fn snapshot(&self, position: AudioPlaybackPosition) -> Result<ClockSnapshot, ClockError> {
        let delta_frames = position
            .played_frames()
            .checked_sub(self.anchor_played_frames)
            .ok_or(ClockError::PositionWentBackward)?;

        let delta_nanoseconds = i128::from(delta_frames)
            .checked_mul(1_000_000_000)
            .ok_or(ClockError::Overflow)?
            / i128::from(self.sample_rate);
        let delta_nanoseconds =
            i64::try_from(delta_nanoseconds).map_err(|_| ClockError::Overflow)?;

        let media_time = self
            .anchor_media
            .checked_add(MediaDelta::from_nanoseconds(delta_nanoseconds))
            .map_err(|_| ClockError::Overflow)?;
        Ok(ClockSnapshot {
            media_time,
            observed_at: position.observed_at(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    InvalidSampleRate,
    PositionWentBackward,
    Overflow,
}
