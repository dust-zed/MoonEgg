use std::ffi::CStr;

use moonegg_core::media::{
    AudioCodecId, AudioTrackFormat, H264CodecConfig, TimeBase, TrackFormat, TrackId, TrackInfo,
    VideoCodecId, VideoTrackFormat,
};

use crate::{
    media_extractor::{MediaExtractorError, NativeMediaExtractor},
    media_format::{MediaFormatError, NativeMediaFormat},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TrackProbeError {
    #[error("获取轨道格式失败：{source}")]
    Extractor { source: MediaExtractorError },
    #[error("读取轨道{track_index}字段失败：{source}")]
    Format {
        track_index: usize,
        source: MediaFormatError,
    },
    #[error("轨道{track_index}缺少有效 MIME")]
    MissingMime { track_index: usize },

    #[error("轨道 {track_index} 缺少必要字段：{field}")]
    MissingField {
        track_index: usize,
        field: &'static str,
    },
    #[error("轨道 {track_index} 的字段值无效：{field}={value}")]
    InvalidField {
        track_index: usize,
        field: &'static str,
        value: i64,
    },

    #[error("轨道 {track_index} 的解码初始化配置 {field} 为空")]
    EmptyCodecConfig {
        track_index: usize,
        field: &'static str,
    },
    #[error("轨道索引无法表示为 TrackId：track_index={track_index}")]
    TrackIndexOutOfRange { track_index: usize },
}

pub(crate) fn find_aac_track(
    extractor: &NativeMediaExtractor,
) -> Result<Option<(usize, NativeMediaFormat)>, TrackProbeError> {
    find_track_by_mime(extractor, "audio/mp4a-latm")
}

pub(crate) fn find_h264_track(
    extractor: &NativeMediaExtractor,
) -> Result<Option<(usize, NativeMediaFormat)>, TrackProbeError> {
    find_track_by_mime(extractor, "video/avc")
}

fn find_track_by_mime(
    extractor: &NativeMediaExtractor,
    expected_mime: &str,
) -> Result<Option<(usize, NativeMediaFormat)>, TrackProbeError> {
    let track_count = extractor.track_count();
    for track_index in 0..track_count {
        let mut track_format = extractor
            .track_format(track_index)
            .map_err(|error| TrackProbeError::Extractor { source: error })?;
        let mime_type = track_format
            .get_string(c"mime")
            .map_err(|error| TrackProbeError::Format {
                track_index,
                source: error,
            })?
            .ok_or(TrackProbeError::MissingMime { track_index })?;
        if mime_type.is_empty() {
            return Err(TrackProbeError::MissingMime { track_index });
        }
        if mime_type == expected_mime {
            return Ok(Some((track_index, track_format)));
        }
    }
    Ok(None)
}

fn read_sample_rate(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<u32, TrackProbeError> {
    let raw_sample_rate =
        track_format
            .get_i32(c"sample-rate")
            .ok_or(TrackProbeError::MissingField {
                track_index,
                field: "sample-rate",
            })?;
    if raw_sample_rate <= 0 {
        return Err(TrackProbeError::InvalidField {
            track_index,
            field: "sample-rate",
            value: raw_sample_rate as i64,
        });
    }

    u32::try_from(raw_sample_rate).map_err(|_| TrackProbeError::InvalidField {
        track_index,
        field: "sample-rate",
        value: raw_sample_rate as i64,
    })
}

fn read_channel_count(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<u16, TrackProbeError> {
    let raw_channel_count =
        track_format
            .get_i32(c"channel-count")
            .ok_or(TrackProbeError::MissingField {
                track_index,
                field: "channel-count",
            })?;
    if raw_channel_count <= 0 {
        return Err(TrackProbeError::InvalidField {
            track_index,
            field: "channel-count",
            value: raw_channel_count as i64,
        });
    }

    u16::try_from(raw_channel_count).map_err(|_| TrackProbeError::InvalidField {
        track_index,
        field: "channel-count",
        value: raw_channel_count as i64,
    })
}

fn read_duration_us(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<Option<u64>, TrackProbeError> {
    let raw_duration_us = track_format.get_i64(c"durationUs");
    if let Some(duration_us) = raw_duration_us {
        if duration_us < 0 {
            return Err(TrackProbeError::InvalidField {
                track_index,
                field: "durationUs",
                value: duration_us,
            });
        }
        return Ok(Some(duration_us as u64));
    }
    Ok(None)
}

fn read_aac_codec_config(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<Vec<u8>, TrackProbeError> {
    read_required_codec_config(track_format, track_index, c"csd-0", "csd-0")
}

fn read_video_dimensions(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<(u32, u32), TrackProbeError> {
    let width = track_format
        .get_i32(c"width")
        .ok_or(TrackProbeError::MissingField {
            track_index,
            field: "width",
        })?;
    let height = track_format
        .get_i32(c"height")
        .ok_or(TrackProbeError::MissingField {
            track_index,
            field: "height",
        })?;
    if width <= 0 {
        return Err(TrackProbeError::InvalidField {
            track_index,
            field: "width",
            value: width as i64,
        });
    }
    if height <= 0 {
        return Err(TrackProbeError::InvalidField {
            track_index,
            field: "height",
            value: height as i64,
        });
    }
    let witdh_u32 = width as u32;
    let height_u32 = height as u32;
    Ok((witdh_u32, height_u32))
}

fn read_h264_codec_config(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
) -> Result<H264CodecConfig, TrackProbeError> {
    let sps = read_required_codec_config(track_format, track_index, c"csd-0", "csd-0")?;
    let pps = read_required_codec_config(track_format, track_index, c"csd-1", "csd-1")?;
    Ok(H264CodecConfig::new(sps, pps))
}

fn read_required_codec_config(
    track_format: &mut NativeMediaFormat,
    track_index: usize,
    key: &CStr,
    field: &'static str,
) -> Result<Vec<u8>, TrackProbeError> {
    let codec_config = track_format
        .get_buffer(key)
        .map_err(|error| TrackProbeError::Format {
            track_index,
            source: error,
        })?;
    if let Some(codec_config) = codec_config {
        if codec_config.is_empty() {
            return Err(TrackProbeError::EmptyCodecConfig { track_index, field });
        }
        return Ok(codec_config);
    }
    Err(TrackProbeError::MissingField { track_index, field })
}

pub(crate) fn probe_aac_track(
    extractor: &NativeMediaExtractor,
) -> Result<Option<TrackInfo>, TrackProbeError> {
    match find_aac_track(extractor)? {
        Some((track_index, mut track_format)) => {
            let raw_track_id = u32::try_from(track_index)
                .map_err(|_| TrackProbeError::TrackIndexOutOfRange { track_index })?;
            let track_id = TrackId::new(raw_track_id);
            let sample_rate = read_sample_rate(&mut track_format, track_index)?;
            let channel_count = read_channel_count(&mut track_format, track_index)?;
            let duration_us = read_duration_us(&mut track_format, track_index)?;

            let codec_config = read_aac_codec_config(&mut track_format, track_index)?;
            let audio_format =
                AudioTrackFormat::new(AudioCodecId::Aac, sample_rate, channel_count, codec_config);

            let time_base = TimeBase::new(1, 1_000_000).expect("固定的微秒时间基必须有效");
            let track_info = TrackInfo::new(
                track_id,
                time_base,
                None,
                duration_us,
                TrackFormat::Audio(audio_format),
            );

            Ok(Some(track_info))
        }
        None => Ok(None),
    }
}

pub(crate) fn probe_h264_track(
    extractor: &NativeMediaExtractor,
) -> Result<Option<TrackInfo>, TrackProbeError> {
    match find_h264_track(extractor)? {
        Some((track_index, mut track_format)) => {
            let raw_track_id = u32::try_from(track_index)
                .map_err(|_| TrackProbeError::TrackIndexOutOfRange { track_index })?;
            let track_id = TrackId::new(raw_track_id);

            let (width, height) = read_video_dimensions(&mut track_format, track_index)?;
            let codec_config = read_h264_codec_config(&mut track_format, track_index)?;
            let video_format =
                VideoTrackFormat::new(VideoCodecId::H264, width, height, codec_config);
            let time_base = TimeBase::new(1, 1_000_000).expect("固定的微秒时间基必然有效");
            let duration_us = read_duration_us(&mut track_format, track_index)?;
            let track_info = TrackInfo::new(
                track_id,
                time_base,
                None,
                duration_us,
                TrackFormat::Video(video_format),
            );
            Ok(Some(track_info))
        }
        None => Ok(None),
    }
}
