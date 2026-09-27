use moonegg_core::{
    media::{
        AudioBuffer, AudioTrackFormat, DecodedFrame, MediaTime, Rounding, TimeBase, TimeError,
        Timestamp, TrackId,
    },
    ports::{DecodeError, DecodeInput, Decoder, ReceiveResult, SubmitResult},
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

impl AndroidDecoderError {
    fn into_decode_error(self) -> DecodeError {
        match self {
            Self::Format { source } => match source {
                DecoderFormatError::UnsupportedCodec { .. } => DecodeError::Unsupported,
                DecoderFormatError::InvalidSampleRate { .. }
                | DecoderFormatError::InvalidChannelCount { .. }
                | DecoderFormatError::EmptyCodecConfig => DecodeError::InvalidData,
                DecoderFormatError::Format { .. } => DecodeError::Platform,
            },
            Self::Codec { source } => match source {
                MediaCodecError::InvalidState => DecodeError::InvalidState,
                MediaCodecError::EmptyInput => DecodeError::InvalidData,
                // 合法数据包也可能超过当前输入槽位容量。
                MediaCodecError::InputBufferTooSmall { .. } => DecodeError::Unsupported,
                MediaCodecError::CreateFailed
                | MediaCodecError::ConfigureFailed { .. }
                | MediaCodecError::StartFailed { .. }
                | MediaCodecError::DequeueInputFailed { .. }
                | MediaCodecError::NullInputBuffer { .. }
                | MediaCodecError::QueueInputFailed { .. }
                | MediaCodecError::DequeueOutputFailed { .. }
                | MediaCodecError::GetOutputFormatFailed
                | MediaCodecError::InvalidOutputSize { .. }
                | MediaCodecError::NullOutputBuffer { .. }
                | MediaCodecError::ReleaseOutputFailed { .. }
                | MediaCodecError::FlushFailed { .. } => DecodeError::Platform,
            },
            Self::Pcm { source } => match source {
                PcmOutputError::UnsupportedEncoding { .. } => DecodeError::Unsupported,
                // 这些异常来自平台输出，不能据此断言输入媒体损坏。
                PcmOutputError::Format { .. }
                | PcmOutputError::MissingField { .. }
                | PcmOutputError::InvalidField { .. }
                | PcmOutputError::UnsupportedMime { .. }
                | PcmOutputError::InvalidPcmLength { .. }
                | PcmOutputError::InvalidAudioBuffer { .. } => DecodeError::Platform,
            },
            Self::InvalidState | Self::UnexpectedTrack { .. } => DecodeError::InvalidState,
            Self::MissingPts => DecodeError::InvalidData,
            Self::MissingOutputFormat => DecodeError::Platform,
            Self::TimeConversion { reason } => match reason {
                TimeError::InvalidTimeBase => DecodeError::InvalidData,
                TimeError::Overflow => DecodeError::Unsupported,
            },
            // 时间戳可能合法，但超出了当前实现的表示或处理范围。
            Self::TimestampOutOfRange { .. } | Self::UnsupportedInputTimestamp { .. } => {
                DecodeError::Unsupported
            }
        }
    }
}
pub(crate) struct AndroidAudioDecoder {
    codec: NativeMediaCodec,
    track_id: TrackId,
    pcm_format: Option<PcmOutputFormat>,
    failed: bool,
    codec_config: Vec<u8>,
    // 是否必须在普通输入前补交配置
    codec_config_pending: bool,
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
            codec_config: audio_format.codec_config().to_vec(),
            codec_config_pending: false,
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
        if !self.try_restore_codec_config()? {
            return Ok(SubmitResult::Backpressure(input));
        }
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

    fn flush(&mut self) -> Result<(), AndroidDecoderError> {
        if self.failed {
            return Err(AndroidDecoderError::InvalidState);
        }
        let needs_codec_config = self.codec_config_pending || !self.codec.has_output_started();
        self.codec.flush().map_err(|error| {
            self.failed = true;
            AndroidDecoderError::Codec { source: error }
        })?;
        self.codec_config_pending = needs_codec_config;
        Ok(())
    }
    fn try_restore_codec_config(&mut self) -> Result<bool, AndroidDecoderError> {
        if !self.codec_config_pending {
            return Ok(true);
        }
        match self
            .codec
            .try_queue_codec_config(&self.codec_config)
            .map_err(|error| AndroidDecoderError::Codec { source: error })?
        {
            InputQueueResult::WouldBlock => Ok(false),
            InputQueueResult::Queued => {
                self.codec_config_pending = false;
                Ok(true)
            }
        }
    }
}

impl Decoder for AndroidAudioDecoder {
    type Output = AudioBuffer;
    fn flush(&mut self) -> Result<(), DecodeError> {
        AndroidAudioDecoder::flush(self).map_err(|error| error.into_decode_error())
    }
    fn receive(&mut self) -> Result<ReceiveResult<Self::Output>, DecodeError> {
        AndroidAudioDecoder::receive(self).map_err(|error| error.into_decode_error())
    }
    fn submit(&mut self, input: DecodeInput) -> Result<SubmitResult, DecodeError> {
        AndroidAudioDecoder::submit(self, input).map_err(|error| error.into_decode_error())
    }
}

fn input_time_us(timestamp: Timestamp) -> Result<u64, AndroidDecoderError> {
    if timestamp.ticks() < 0 {
        return Err(AndroidDecoderError::UnsupportedInputTimestamp { timestamp });
    }
    let microsecond_time_base = TimeBase::new(1, 1_000_000).expect("msg");
    let timestamp_us = timestamp
        .rescale(microsecond_time_base, Rounding::TowardZero)
        .map_err(|error| AndroidDecoderError::TimeConversion { reason: error })?;
    let ticks = timestamp_us.ticks() as u64;
    Ok(ticks)
}
