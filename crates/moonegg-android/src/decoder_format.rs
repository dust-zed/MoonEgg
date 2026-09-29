use std::error;

use moonegg_core::media::{AudioCodecId, AudioTrackFormat};

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
