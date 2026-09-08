//! 接收有限长度的 PCM
//! 在设备队列满时产生背压
//! 开始、暂停和清空播放
//! 报告已经真正播放了多少 audio frame

use std::time::Instant;

use crate::media::{AudioBuffer, DecodedFrame};

#[derive(Debug)]
pub enum AudioSubmitResult {
    Accepted,
    Backpressure(DecodedFrame<AudioBuffer>),
}

#[derive(Debug, Clone, Copy)]
pub struct AudioPlaybackPosition {
    played_frames: u64,
    observed_at: Instant,
}

impl AudioPlaybackPosition {
    pub const fn new(played_frames: u64, observed_at: Instant) -> Self {
        Self {
            played_frames,
            observed_at,
        }
    }

    pub const fn played_frames(self) -> u64 {
        self.played_frames
    }

    pub const fn observed_at(self) -> Instant {
        self.observed_at
    }
}

#[derive(Debug)]
pub enum AudioOutputError {
    InvalidFormat,
    InvalidState,
    DeviceUnavailable,
    Platform,
}

pub trait AudioOutput {
    fn submit(
        &mut self,
        frame: DecodedFrame<AudioBuffer>,
    ) -> Result<AudioSubmitResult, AudioOutputError>;

    fn start(&mut self) -> Result<(), AudioOutputError>;

    fn pause(&mut self) -> Result<(), AudioOutputError>;

    fn flush(&mut self) -> Result<(), AudioOutputError>;

    fn playback_position(&mut self) -> Result<AudioPlaybackPosition, AudioOutputError>;
}
