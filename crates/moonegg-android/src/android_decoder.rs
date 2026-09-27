use moonegg_core::{
    media::{
        AudioBuffer, AudioTrackFormat, DecodedFrame, MediaTime, Rounding, TimeBase, TimeError,
        Timestamp, TrackId,
    },
    ports::{DecodeInput, ReceiveResult, SubmitResult},
};
use ndk_sys::AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG;

use crate::{
    decoder_format::{DecoderFormatError, build_aac_decoder_format},
    media_codec::{CodecOutput, InputQueueResult, MediaCodecError, NativeMediaCodec},
    pcm_output::{PcmOutputError, PcmOutputFormat, parse_pcm_output_format},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AndroidDecoderError {
    #[error("")]
    Format { source: DecoderFormatError },
    #[error("")]
    Codec { source: MediaCodecError },
    #[error("")]
    Pcm { source: PcmOutputError },
    #[error("")]
    MissingOutputFormat,
    #[error("")]
    TimestampOutOfRange { time_us: i64 },
    #[error("")]
    InvalidState,
    #[error("")]
    UnexpectedTrack { expected: TrackId, actual: TrackId },
    #[error("")]
    MissingPts,
    #[error("")]
    TimeConversion { reason: TimeError },
    #[error("")]
    UnsupportedInputTimestamp { timestamp: Timestamp },
}

pub(crate) struct AndroidAudioDecoder {
    codec: NativeMediaCodec,
    track_id: TrackId,
    pcm_format: Option<PcmOutputFormat>,
    failed: bool,
}

impl AndroidAudioDecoder {
    pub(crate) fn new(
        track_id: TrackId,
        audio_format: &AudioTrackFormat,
    ) -> Result<Self, AndroidDecoderError> {
        let decoder_format = build_aac_decoder_format(audio_format)
            .map_err(|error| AndroidDecoderError::Format { source: error })?;
        let codec = NativeMediaCodec::new_audio_decoder(c"audio/mp4a-latm", &decoder_format)
            .map_err(|error| AndroidDecoderError::Codec { source: error })?;

        Ok(Self {
            codec,
            track_id,
            pcm_format: None,
            failed: false,
        })
    }

    fn receive_inner(&mut self) -> Result<ReceiveResult<AudioBuffer>, AndroidDecoderError> {
        match self
            .codec
            .try_receive()
            .map_err(|error| AndroidDecoderError::Codec { source: error })?
        {
            CodecOutput::NotReady => Ok(ReceiveResult::NotReady),
            CodecOutput::FormatChanged { mut format } => {
                let pcm_format = parse_pcm_output_format(&mut format)
                    .map_err(|error| AndroidDecoderError::Pcm { source: error })?;
                self.pcm_format = Some(pcm_format);
                Ok(ReceiveResult::NotReady)
            }
            CodecOutput::EndOfStream => Ok(ReceiveResult::EndOfStream),
            CodecOutput::Buffer { buffer } => {
                let (data, presentation_time_us, flags) = buffer.into_parts();
                if (flags & AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG) != 0 {
                    return Ok(ReceiveResult::NotReady);
                }
                let Some(output_format) = self.pcm_format.as_ref() else {
                    return Err(AndroidDecoderError::MissingOutputFormat);
                };
                let pts_ns = presentation_time_us.checked_mul(1000).ok_or(
                    AndroidDecoderError::TimestampOutOfRange {
                        time_us: presentation_time_us,
                    },
                )?;
                let audio_buffer = output_format
                    .to_audio_buffer(&data)
                    .map_err(|error| AndroidDecoderError::Pcm { source: error })?;
                let frame = DecodedFrame::new(
                    self.track_id,
                    MediaTime::from_nanoseconds(pts_ns),
                    audio_buffer,
                );
                Ok(ReceiveResult::Frame(frame))
            }
        }
    }

    fn receive(&mut self) -> Result<ReceiveResult<AudioBuffer>, AndroidDecoderError> {
        if self.failed {
            return Err(AndroidDecoderError::InvalidState);
        }

        let result = self.receive_inner();
        self.failed = result.is_err();
        result
    }

    fn submit_inner(&mut self, input: DecodeInput) -> Result<SubmitResult, AndroidDecoderError> {
        match input {
            DecodeInput::Packet(packet) => {
                if self.track_id != packet.track_id() {
                    return Err(AndroidDecoderError::UnexpectedTrack {
                        expected: self.track_id,
                        actual: packet.track_id(),
                    });
                }
                let Some(packet_pts) = packet.pts() else {
                    return Err(AndroidDecoderError::MissingPts);
                };
                let presentation_time_us = input_time_us(packet_pts)?;
                match self
                    .codec
                    .try_queue_data(packet.data(), presentation_time_us)
                    .map_err(|error| AndroidDecoderError::Codec { source: error })?
                {
                    InputQueueResult::Queued => Ok(SubmitResult::Accepted),
                    InputQueueResult::WouldBlock => {
                        Ok(SubmitResult::Backpressure(DecodeInput::Packet(packet)))
                    }
                }
            }
            DecodeInput::EndOfStream => {
                match self
                    .codec
                    .try_queue_eos()
                    .map_err(|error| AndroidDecoderError::Codec { source: error })?
                {
                    InputQueueResult::Queued => Ok(SubmitResult::Accepted),
                    InputQueueResult::WouldBlock => {
                        Ok(SubmitResult::Backpressure(DecodeInput::EndOfStream))
                    }
                }
            }
        }
    }

    fn submit(&mut self, input: DecodeInput) -> Result<SubmitResult, AndroidDecoderError> {
        if self.failed {
            return Err(AndroidDecoderError::InvalidState);
        }

        let result = self.submit_inner(input);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

fn input_time_us(timestamp: Timestamp) -> Result<u64, AndroidDecoderError> {
    if timestamp.ticks() < 0 {
        return Err(AndroidDecoderError::UnsupportedInputTimestamp { timestamp });
    }
    let time_base = TimeBase::new(1, 1_000_000).expect("msg");
    let new_timestamp = timestamp
        .rescale(time_base, Rounding::TowardZero)
        .map_err(|error| AndroidDecoderError::TimeConversion { reason: error })?;
    let ticks = new_timestamp.ticks() as u64;
    Ok(ticks)
}
