//! 解码后的媒体帧及其呈现时间

use crate::media::{
    MediaTime, TrackId,
    format::{AudioPcmFormat, AudioPcmFormatError, AudioSampleFormat},
};

/// type AudioFrame = DecodedFrame<pcmBuffer>
/// type SoftwareVideoFrame = DecodedFrame<YuvBuffer>
/// type AndroidideoFrame = DecodedFrame<MediaCodecBuffer>
#[derive(Debug)]
pub struct DecodedFrame<T> {
    track_id: TrackId,
    pts: MediaTime,
    payload: T,
}

impl<T> DecodedFrame<T> {
    pub const fn new(track_id: TrackId, pts: MediaTime, payload: T) -> Self {
        Self {
            track_id,
            pts,
            payload,
        }
    }

    pub const fn track_id(&self) -> TrackId {
        self.track_id
    }

    pub const fn pts(&self) -> MediaTime {
        self.pts
    }

    pub const fn payload(&self) -> &T {
        &self.payload
    }

    pub fn into_payload(self) -> T {
        self.payload
    }

    pub fn into_parts(self) -> (TrackId, MediaTime, T) {
        (self.track_id, self.pts, self.payload)
    }
}

#[derive(Debug)]
pub enum AudioSamples {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

impl AudioSamples {
    pub fn len(&self) -> usize {
        match self {
            Self::F32(data) => data.len(),
            Self::I16(data) => data.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sample_format(&self) -> AudioSampleFormat {
        match self {
            Self::F32(_) => AudioSampleFormat::F32,
            Self::I16(_) => AudioSampleFormat::I16,
        }
    }
}

#[derive(Debug)]
pub struct AudioBuffer {
    format: AudioPcmFormat,
    samples: AudioSamples,
}

impl AudioBuffer {
    pub fn new(
        sample_rate: u32,
        channel_count: u16,
        samples: AudioSamples,
    ) -> Result<AudioBuffer, AudioBufferError> {
        if sample_rate == 0 {
            return Err(AudioBufferError::InvalidSampleRate);
        }

        if channel_count == 0 {
            return Err(AudioBufferError::InvalidChannelCount);
        }
        let sample_format = samples.sample_format();
        let format =
            AudioPcmFormat::new(sample_rate, channel_count, sample_format).map_err(|error| {
                match error {
                    AudioPcmFormatError::InvalidChannelCount { .. } => {
                        AudioBufferError::InvalidChannelCount
                    }
                    AudioPcmFormatError::InvalidSampleRate { .. } => {
                        AudioBufferError::InvalidSampleRate
                    }
                }
            })?;

        if !samples.len().is_multiple_of(usize::from(channel_count)) {
            return Err(AudioBufferError::IncompleteFrame);
        }

        Ok(AudioBuffer { format, samples })
    }
    pub fn sample_rate(&self) -> u32 {
        self.format.sample_rate()
    }

    pub fn channel_count(&self) -> u16 {
        self.format.channel_count()
    }

    pub fn format(&self) -> AudioPcmFormat {
        self.format
    }

    pub fn samples(&self) -> &AudioSamples {
        &self.samples
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn frame_count(&self) -> usize {
        self.sample_count() / self.channel_count() as usize
    }

    pub fn discard_prefix_frames(&mut self, frames: usize) -> Result<(), AudioBufferError> {
        let available_frames = self.frame_count();
        if frames > available_frames {
            return Err(AudioBufferError::TrimOutOfRange {
                requested_frames: frames,
                available_frames,
            });
        }
        let channels = usize::from(self.channel_count());
        let samples_to_remove = channels * frames;
        match &mut self.samples {
            AudioSamples::I16(data) => {
                data.drain(..samples_to_remove);
            }
            AudioSamples::F32(data) => {
                data.drain(..samples_to_remove);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBufferError {
    InvalidSampleRate,
    InvalidChannelCount,
    IncompleteFrame,
    TrimOutOfRange {
        requested_frames: usize,
        available_frames: usize,
    },
}
