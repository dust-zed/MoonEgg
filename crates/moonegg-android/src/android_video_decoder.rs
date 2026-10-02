use std::rc::Rc;

use moonegg_core::{
    media::{
        DecodedFrame, MediaTime, Rounding, TimeBase, TimeError, Timestamp, TrackId,
        VideoTrackFormat,
    },
    ports::ReceiveResult,
};
use ndk::native_window::NativeWindow;

use crate::{
    decoder_format::{VideoDecoderFormatError, build_h264_decoder_format},
    media_codec::{MediaCodecError, NativeMediaCodec, SurfaceCodecOutput},
    video_buffer::AndroidVideoBuffer,
    video_codec_session::{VideoCodecSession, VideoCodecSessionError},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AndroidVideoDecoderError {
    #[error("构造视频解码格式失败：{source}")]
    Format { source: VideoDecoderFormatError },

    #[error("视频编解码器调用失败：{source}")]
    Codec { source: MediaCodecError },

    #[error("访问视频编解码会话失败：{source}")]
    Session { source: VideoCodecSessionError },

    #[error("视频解码器状态不允许此操作，或时间戳映射尚未建立")]
    InvalidState,

    #[error("编码包轨道不匹配：expected={expected:?}, actual={actual:?}")]
    UnexpectedTrack { expected: TrackId, actual: TrackId },

    #[error("视频编码包缺少 PTS")]
    MissingPts,

    #[error("视频时间戳换算失败：{reason:?}")]
    TimeConversion { reason: TimeError },

    #[error("时间戳计算超出支持范围：time_us={time_us} μs")]
    TimestampOutOfRange { time_us: i64 },

    #[error("输入时间戳无法映射到当前解码器时间区间：{timestamp:?}")]
    UnsupportedInputTimestamp { timestamp: Timestamp },
}

pub(crate) struct AndroidVideoDecoder {
    session: Rc<VideoCodecSession>,
    track_id: TrackId,
    failed: bool,
    timestamp_offset_us: Option<i64>,
}

impl AndroidVideoDecoder {
    pub(crate) fn new(
        track_id: TrackId,
        video_format: &VideoTrackFormat,
        output_window: NativeWindow,
    ) -> Result<Self, AndroidVideoDecoderError> {
        let decoder_format = build_h264_decoder_format(video_format)
            .map_err(|error| AndroidVideoDecoderError::Format { source: error })?;
        let codec =
            NativeMediaCodec::new_video_decoder(c"video/avc", &decoder_format, output_window)
                .map_err(|error| AndroidVideoDecoderError::Codec { source: error })?;
        let session = VideoCodecSession::new(codec);
        Ok(Self {
            session,
            track_id,
            failed: false,
            timestamp_offset_us: None,
        })
    }

    fn map_input_timestamp(
        &mut self,
        timestamp: Timestamp,
    ) -> Result<u64, AndroidVideoDecoderError> {
        let microsecond_time_base = TimeBase::new(1, 1_000_000).expect("msg");
        let timestamp_us = timestamp
            .rescale(microsecond_time_base, Rounding::TowardZero)
            .map_err(|error| AndroidVideoDecoderError::TimeConversion { reason: error })?;
        let offset_us = match self.timestamp_offset_us {
            Some(offset) => offset,
            None => {
                let ticks = timestamp_us.ticks();
                if ticks < 0 {
                    ticks
                        .checked_neg()
                        .ok_or(AndroidVideoDecoderError::TimestampOutOfRange { time_us: ticks })?
                } else {
                    0
                }
            }
        };
        let codec_pts_us = timestamp_us.ticks().checked_add(offset_us).ok_or(
            AndroidVideoDecoderError::TimestampOutOfRange {
                time_us: timestamp_us.ticks(),
            },
        )?;
        let codec_pts_us = u64::try_from(codec_pts_us)
            .map_err(|_| AndroidVideoDecoderError::UnsupportedInputTimestamp { timestamp })?;
        self.timestamp_offset_us = Some(offset_us);
        Ok(codec_pts_us)
    }

    fn restore_output_timestamp(
        &self,
        codec_pts_us: i64,
    ) -> Result<MediaTime, AndroidVideoDecoderError> {
        let Some(offset_us) = self.timestamp_offset_us else {
            return Err(AndroidVideoDecoderError::InvalidState);
        };
        let media_pts_us = codec_pts_us.checked_sub(offset_us).ok_or(
            AndroidVideoDecoderError::TimestampOutOfRange {
                time_us: codec_pts_us,
            },
        )?;
        let media_pts_ns = media_pts_us.checked_mul(1000).ok_or(
            AndroidVideoDecoderError::TimestampOutOfRange {
                time_us: codec_pts_us,
            },
        )?;
        Ok(MediaTime::from_nanoseconds(media_pts_ns))
    }

    fn receive_inner(
        &mut self,
    ) -> Result<ReceiveResult<AndroidVideoBuffer>, AndroidVideoDecoderError> {
        let codec_output = self
            .session
            .with_codec(|codec| codec.try_receive_surface())
            .map_err(|error| AndroidVideoDecoderError::Session { source: error })?;
        match codec_output {
            SurfaceCodecOutput::NotReady => Ok(ReceiveResult::NotReady),
            SurfaceCodecOutput::FormatChanged { format } => {
                // TODO
                // 先消费这次通知，其他操作后续接入再处理
                Ok(ReceiveResult::NotReady)
            }
            SurfaceCodecOutput::EndOfStream => Ok(ReceiveResult::EndOfStream),
            SurfaceCodecOutput::Frame { token } => {
                let codec_pts_us = token.presentation_time_us();
                let video_buffer = AndroidVideoBuffer::new(Rc::clone(&self.session), token);
                let media_pts = self.restore_output_timestamp(codec_pts_us)?;
                let frame = DecodedFrame::new(self.track_id, media_pts, video_buffer);
                Ok(ReceiveResult::Frame(frame))
            }
        }
    }

    fn receive(&mut self) -> Result<ReceiveResult<AndroidVideoBuffer>, AndroidVideoDecoderError> {
        if self.failed {
            return Err(AndroidVideoDecoderError::InvalidState);
        }
        let result = self.receive_inner();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}
