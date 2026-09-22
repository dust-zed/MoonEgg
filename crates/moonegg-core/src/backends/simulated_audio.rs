use std::time::Instant;

use crate::{
    media::{AudioBuffer, AudioSamples, DecodedFrame},
    ports::{
        AudioOutput, AudioOutputError, AudioOutputFactory, AudioPlaybackPosition, AudioSubmitResult,
    },
    runtime::AudioPipelineFactory,
};

pub(crate) struct SimulatedAudioOutput {
    sample_rate: u32,
    channels: u16,

    capacity_frames: u64,
    buffered_frames: u64,
    played_frames: u64,

    last_update: Option<Instant>,

    frame_remainder: u128,
}

impl SimulatedAudioOutput {
    pub(crate) fn new(
        sample_rate: u32,
        channels: u16,
        capacity_frames: u64,
    ) -> Result<Self, AudioOutputError> {
        if sample_rate == 0 || !matches!(channels, 1 | 2) {
            return Err(AudioOutputError::InvalidFormat);
        }

        if capacity_frames == 0 {
            return Err(AudioOutputError::InvalidState);
        }

        Ok(Self {
            sample_rate,
            channels,
            capacity_frames,
            buffered_frames: 0,
            played_frames: 0,
            last_update: None,
            frame_remainder: 0,
        })
    }

    fn update(&mut self, now: Instant) -> Result<(), AudioOutputError> {
        let Some(previous) = self.last_update else {
            return Ok(());
        };

        let elapsed = now
            .checked_duration_since(previous)
            .ok_or(AudioOutputError::InvalidState)?;

        let scaled_frames =
            elapsed.as_nanos() * u128::from(self.sample_rate) + self.frame_remainder;

        // 根据经过的时间，现在应该消费多少个 audio frame。
        let due_frames = scaled_frames / 1_000_000_000;

        let consumed = due_frames.min(u128::from(self.buffered_frames)) as u64;

        let played_frames = self
            .played_frames
            .checked_add(consumed)
            .ok_or(AudioOutputError::InvalidState)?;

        self.buffered_frames -= consumed;
        self.played_frames = played_frames;

        self.frame_remainder = if self.buffered_frames == 0 {
            0
        } else {
            scaled_frames % 1_000_000_000
        };

        self.last_update = Some(now);
        Ok(())
    }
}

impl AudioOutput for SimulatedAudioOutput {
    fn submit(
        &mut self,
        frame: DecodedFrame<AudioBuffer>,
    ) -> Result<crate::ports::AudioSubmitResult, AudioOutputError> {
        self.update(Instant::now())?;

        let buffer = frame.payload();

        if buffer.sample_rate() != self.sample_rate
            || buffer.channel_count() != self.channels
            || !matches!(buffer.samples(), AudioSamples::I16(_))
        {
            return Err(AudioOutputError::InvalidFormat);
        }

        let frames =
            u64::try_from(buffer.frame_count()).map_err(|_| AudioOutputError::InvalidFormat)?;

        if frames > self.capacity_frames {
            return Err(AudioOutputError::BufferTooLarge);
        }

        let free_frames = self.capacity_frames - self.buffered_frames;

        if frames > free_frames {
            return Ok(AudioSubmitResult::Backpressure(frame));
        }

        self.buffered_frames += frames;

        // 模拟输出只保存数量，采样内容随 frame 释放
        Ok(AudioSubmitResult::Accepted)
    }

    fn start(&mut self) -> Result<(), AudioOutputError> {
        if self.last_update.is_none() {
            self.last_update = Some(Instant::now());
        }
        Ok(())
    }

    fn pause(&mut self) -> Result<(), AudioOutputError> {
        self.update(Instant::now())?;
        self.last_update = None;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), AudioOutputError> {
        self.update(Instant::now())?;

        self.buffered_frames = 0;
        self.frame_remainder = 0;

        Ok(())
    }

    fn playback_position(&mut self) -> Result<AudioPlaybackPosition, AudioOutputError> {
        let now = Instant::now();
        self.update(now)?;

        Ok(AudioPlaybackPosition::new(self.played_frames, now))
    }

    fn is_drained(&mut self) -> Result<bool, AudioOutputError> {
        self.update(Instant::now())?;

        Ok(self.buffered_frames == 0)
    }
}

pub(crate) struct SimulatedAudioOutputFactory {
    capacity_frames: u64,
}

impl SimulatedAudioOutputFactory {
    pub(crate) const fn new(capacity_frames: u64) -> Self {
        Self { capacity_frames }
    }
}

impl AudioOutputFactory for SimulatedAudioOutputFactory {
    type Output = SimulatedAudioOutput;
    fn create(
        &self,
        format: &crate::media::AudioTrackFormat,
    ) -> Result<Self::Output, AudioOutputError> {
        SimulatedAudioOutput::new(
            format.sample_rate(),
            format.channel_count(),
            self.capacity_frames,
        )
    }
}
