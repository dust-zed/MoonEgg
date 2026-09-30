use moonegg_core::media::{AudioCodecId, AudioTrackFormat, VideoCodecId, VideoTrackFormat};

use crate::media_format::{MediaFormatError, NativeMediaFormat};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecoderFormatError {
    #[error("AAC 解码器不支持此编码格式：{codec:?}")]
    UnsupportedCodec { codec: AudioCodecId },
    #[error("解码采样率必须为正数且可表示为 i32：sample_rate={sample_rate} Hz")]
    InvalidSampleRate { sample_rate: u32 },
    #[error("解码声道数必须大于零：channel_count={channel_count}")]
    InvalidChannelCount { channel_count: u16 },
    #[error("AAC 解码初始化配置 csd-0 为空")]
    EmptyCodecConfig,
    #[error("创建解码器 MediaFormat 失败：{source}")]
    Format { source: MediaFormatError },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum VideoDecoderFormatError {
    #[error("H.264 解码器不支持此编码格式：{codec:?}")]
    UnsupportedCodec { codec: VideoCodecId },
    #[error("视频解码尺寸必须大于0且可表示为i32：width={width},height={height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("H.264 解码初始化配置 csd-0（sps）为空")]
    EmptySps,
    #[error("H.264 解码初始化配置 csd-1 （pps）为空")]
    EmptyPps,
    #[error("创建视频解码器 MediaFormat 失败：{source}")]
    Format { source: MediaFormatError },
}

pub(crate) fn build_aac_decoder_format(
    audio_format: &AudioTrackFormat,
) -> Result<NativeMediaFormat, DecoderFormatError> {
    if audio_format.codec() != AudioCodecId::Aac {
        return Err(DecoderFormatError::UnsupportedCodec {
            codec: audio_format.codec(),
        });
    }

    let sample_rate = audio_format.sample_rate();
    if sample_rate == 0 {
        return Err(DecoderFormatError::InvalidSampleRate { sample_rate });
    }
    let sample_rate_i32 = i32::try_from(sample_rate)
        .map_err(|_| DecoderFormatError::InvalidSampleRate { sample_rate })?;

    let channel_count = audio_format.channel_count();
    if channel_count == 0 {
        return Err(DecoderFormatError::InvalidChannelCount { channel_count });
    }
    let channel_count_i32 = i32::from(channel_count);
    let codec_config = audio_format.codec_config();
    if codec_config.is_empty() {
        return Err(DecoderFormatError::EmptyCodecConfig);
    }

    let mut decoder_format =
        NativeMediaFormat::new().map_err(|error| DecoderFormatError::Format { source: error })?;
    decoder_format.set_string(c"mime", c"audio/mp4a-latm");
    decoder_format.set_i32(c"sample-rate", sample_rate_i32);
    decoder_format.set_i32(c"channel-count", channel_count_i32);

    decoder_format.set_buffer(c"csd-0", codec_config);

    Ok(decoder_format)
}

pub(crate) fn build_h264_decoder_format(
    video_format: &VideoTrackFormat,
) -> Result<NativeMediaFormat, VideoDecoderFormatError> {
    if video_format.codec() != VideoCodecId::H264 {
        return Err(VideoDecoderFormatError::UnsupportedCodec {
            codec: video_format.codec(),
        });
    }

    let width = video_format.width();
    let height = video_format.height();

    if width == 0 || height == 0 {
        return Err(VideoDecoderFormatError::InvalidDimensions { width, height });
    }
    let width_i32 = i32::try_from(width)
        .map_err(|_| VideoDecoderFormatError::InvalidDimensions { width, height })?;
    let height_i32 = i32::try_from(height)
        .map_err(|_| VideoDecoderFormatError::InvalidDimensions { width, height })?;

    let codec_config = video_format.codec_config();
    if codec_config.sps().is_empty() {
        return Err(VideoDecoderFormatError::EmptySps);
    }
    if codec_config.pps().is_empty() {
        return Err(VideoDecoderFormatError::EmptyPps);
    }

    let mut decoder_format = NativeMediaFormat::new()
        .map_err(|error| VideoDecoderFormatError::Format { source: error })?;
    decoder_format.set_string(c"mime", c"video/avc");
    decoder_format.set_i32(c"width", width_i32);
    decoder_format.set_i32(c"height", height_i32);
    decoder_format.set_buffer(c"csd-0", codec_config.sps());
    decoder_format.set_buffer(c"csd-1", codec_config.pps());

    Ok(decoder_format)
}
