use std::time::Instant;

use moonegg_core::{
    media::{AudioBuffer, AudioPcmFormat, AudioSampleFormat, AudioSamples, DecodedFrame},
    ports::{
        AudioOutput, AudioOutputError, AudioOutputFactory, AudioPlaybackPosition, AudioSubmitResult,
    },
};
use ndk::audio::AudioError;

use crate::aaudio::{AAudioError, AAudioPcmStream};

#[derive(Debug, Default)]
pub struct AndroidAudioOutputFactory;

impl AudioOutputFactory for AndroidAudioOutputFactory {
    type Output = AndroidAudioOutput;

    fn create(
        &self,
        format: &AudioPcmFormat,
    ) -> Result<Self::Output, moonegg_core::ports::AudioOutputError> {
        AndroidAudioOutput::new(format)
    }
}

struct PendingPcm {
    frame: DecodedFrame<AudioBuffer>,
    offset_frames: usize,
}

pub struct AndroidAudioOutput {
    stream: AAudioPcmStream,
    format: AudioPcmFormat,
    pending: Option<PendingPcm>,

    running: bool,
    request_started: bool,
}

impl AndroidAudioOutput {
    // 当前 WAV 解码块是 1024 帧；这里给单块设置明确上限
    const MAX_BLOCK_FRAMES: usize = 4096;

    fn new(format: &AudioPcmFormat) -> Result<Self, AudioOutputError> {
        // 当前 AAudioPcmStream 只支持写入 I16。
        if format.sample_format() != AudioSampleFormat::I16 {
            return Err(AudioOutputError::InvalidFormat);
        }
        let stream = AAudioPcmStream::new(format.sample_rate(), format.channel_count())
            .map_err(map_error)?;

        Ok(Self {
            stream,
            format: *format,
            pending: None,
            running: false,
            request_started: false,
        })
    }

    fn validate(&self, frame: &DecodedFrame<AudioBuffer>) -> Result<(), AudioOutputError> {
        let buffer = frame.payload();

        if buffer.format() != self.format {
            return Err(AudioOutputError::InvalidFormat);
        }

        if buffer.frame_count() > Self::MAX_BLOCK_FRAMES {
            return Err(AudioOutputError::BufferTooLarge);
        }

        Ok(())
    }
    /// 最多执行一次非阻塞写入。
    fn pump(&mut self) -> Result<(), AudioOutputError> {
        if !self.running {
            return Ok(());
        }

        let had_pending = self.pending.is_some();

        if let Some(pending) = self.pending.as_mut() {
            let buffer = pending.frame.payload();

            let AudioSamples::I16(samples) = buffer.samples() else {
                return Err(AudioOutputError::InvalidFormat);
            };

            let channels = usize::from(buffer.channel_count());
            let offset_samples = pending.offset_frames * channels;

            let written = self
                .stream
                .write_i16(&samples[offset_samples..])
                .map_err(map_error)?;

            pending.offset_frames += written;

            if pending.offset_frames == buffer.frame_count() {
                self.pending = None;
            }
        }

        if !self.request_started {
            let (consumed, written) = self.stream.frame_counters().map_err(map_error)?;
            if had_pending || written > consumed {
                self.stream.request_start().map_err(map_error)?;
                self.request_started = true;
            }
        }
        Ok(())
    }
}

impl AudioOutput for AndroidAudioOutput {
    fn submit(
        &mut self,
        frame: DecodedFrame<AudioBuffer>,
    ) -> Result<moonegg_core::ports::AudioSubmitResult, AudioOutputError> {
        self.validate(&frame)?;

        self.pump()?;

        if self.pending.is_some() {
            return Ok(AudioSubmitResult::Backpressure(frame));
        }

        if frame.payload().frame_count() == 0 {
            return Ok(AudioSubmitResult::Accepted);
        }

        self.pending = Some(PendingPcm {
            frame,
            offset_frames: 0,
        });

        self.pump()?;

        Ok(AudioSubmitResult::Accepted)
    }

    fn start(&mut self) -> Result<(), AudioOutputError> {
        self.running = true;
        self.pump()
    }

    fn pause(&mut self) -> Result<(), AudioOutputError> {
        self.running = false;
        self.stream.pause().map_err(map_error)?;
        self.request_started = false;

        Ok(())
    }

    fn flush(&mut self) -> Result<(), AudioOutputError> {
        self.pause()?;

        self.stream.flush().map_err(map_error)?;
        self.pending = None;

        Ok(())
    }

    fn playback_position(
        &mut self,
    ) -> Result<moonegg_core::ports::AudioPlaybackPosition, AudioOutputError> {
        let (consumed, written) = self.stream.frame_counters().map_err(map_error)?;

        Ok(AudioPlaybackPosition::new(
            consumed.min(written),
            Instant::now(),
        ))
    }

    fn is_drained(&mut self) -> Result<bool, AudioOutputError> {
        self.pump()?;

        let (consumed, written) = self.stream.frame_counters().map_err(map_error)?;
        Ok(self.pending.is_none() && consumed >= written)
    }

    fn format(&self) -> Option<AudioPcmFormat> {
        Some(self.format)
    }
}

fn map_error(error: AAudioError) -> AudioOutputError {
    match error {
        AAudioError::InvalidFormat
        | AAudioError::FormatMismatch { .. }
        | AAudioError::IncompleteFrame => AudioOutputError::InvalidFormat,
        AAudioError::BufferTooLarge => AudioOutputError::BufferTooLarge,

        AAudioError::InvalidState => AudioOutputError::InvalidState,

        AAudioError::Native(
            AudioError::Disconnected | AudioError::Unavailable | AudioError::NoService,
        ) => AudioOutputError::DeviceUnavailable,

        other => AudioOutputError::Platform {
            reason: format!("{other:?}"),
        },
    }
}
