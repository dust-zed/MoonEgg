use moonegg_core::media::{AudioBuffer, AudioBufferError, AudioSamples};

use crate::media_format::{MediaFormatError, NativeMediaFormat};

#[derive(Debug, thiserror::Error)]
pub(crate) enum PcmOutputError {
    #[error("")]
    Format { source: MediaFormatError },
    #[error("")]
    MissingField { field: &'static str },
    #[error("")]
    InvalidField { field: &'static str, value: i32 },
    #[error("")]
    UnsupportedMime { mime: String },
    #[error("")]
    UnsupportedEncoding { encoding: i32 },
    #[error("")]
    InvalidPcmLength {
        byte_len: usize,
        bytes_per_frame: usize,
    },
    #[error("")]
    InvalidAudioBuffer { reason: AudioBufferError },
}
#[derive(Debug)]
pub(crate) enum PcmEncoding {
    I16,
    F32,
}

#[derive(Debug)]
pub(crate) struct PcmOutputFormat {
    sample_rate: u32,
    channel_count: u16,
    encoding: PcmEncoding,
}

impl PcmOutputFormat {
    pub(crate) fn to_audio_buffer(&self, data: &[u8]) -> Result<AudioBuffer, PcmOutputError> {
        let bytes_per_sample = match self.encoding {
            PcmEncoding::I16 => 2usize,
            PcmEncoding::F32 => 4,
        };

        let bytes_per_frame = bytes_per_sample * usize::from(self.channel_count);

        if !data.len().is_multiple_of(bytes_per_frame) {
            return Err(PcmOutputError::InvalidPcmLength {
                byte_len: data.len(),
                bytes_per_frame,
            });
        }

        let samples = match self.encoding {
            PcmEncoding::I16 => {
                let samples = data
                    .chunks_exact(2)
                    .map(|bytes| i16::from_ne_bytes([bytes[0], bytes[1]]))
                    .collect::<Vec<i16>>();
                AudioSamples::I16(samples)
            }
            PcmEncoding::F32 => {
                let samples = data
                    .chunks_exact(4)
                    .map(|bytes| f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    .collect::<Vec<f32>>();
                AudioSamples::F32(samples)
            }
        };
        AudioBuffer::new(self.sample_rate, self.channel_count, samples)
            .map_err(|error| PcmOutputError::InvalidAudioBuffer { reason: error })
    }
}

pub(crate) fn parse_pcm_output_format(
    format: &mut NativeMediaFormat,
) -> Result<PcmOutputFormat, PcmOutputError> {
    let mime = format
        .get_string(c"mime")
        .map_err(|error| PcmOutputError::Format { source: error })?
        .ok_or(PcmOutputError::MissingField { field: "mime" })?;

    if mime != "audio/raw" {
        return Err(PcmOutputError::UnsupportedMime { mime });
    }

    let raw_sample_rate = format
        .get_i32(c"sample-rate")
        .ok_or(PcmOutputError::MissingField {
            field: "sample-rate",
        })?;
    if raw_sample_rate <= 0 {
        return Err(PcmOutputError::InvalidField {
            field: "sample-rate",
            value: raw_sample_rate,
        });
    }
    let sample_rate = raw_sample_rate as u32;
    let raw_channel_count =
        format
            .get_i32(c"channel-count")
            .ok_or(PcmOutputError::MissingField {
                field: "channel-count",
            })?;
    if raw_channel_count < 0 {
        return Err(PcmOutputError::InvalidField {
            field: "channel-count",
            value: raw_channel_count,
        });
    }
    let channel_count =
        u16::try_from(raw_channel_count).map_err(|_| PcmOutputError::InvalidField {
            field: "channel-count",
            value: raw_channel_count,
        })?;

    let pcm_encoding = match format.get_i32(c"pcm-encoding") {
        None | Some(2) => PcmEncoding::I16,
        Some(4) => PcmEncoding::F32,
        Some(encoding) => {
            return Err(PcmOutputError::UnsupportedEncoding { encoding });
        }
    };
    Ok(PcmOutputFormat {
        sample_rate,
        channel_count,
        encoding: pcm_encoding,
    })
}
