use moonegg_core::media::{
    AudioCodecId, AudioTrackFormat, TimeBase, TrackFormat, TrackId, TrackInfo,
};

use crate::{
    media_extractor::{MediaExtractorError, NativeMediaExtractor},
    media_format::{MediaFormatError, NativeMediaFormat},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TrackProbeError {
    #[error("获取轨道格式失败")]
    Extractor { source: MediaExtractorError },
    #[error("读取轨道{track_index}字段失败：{source}")]
    Format {
        track_index: usize,
        source: MediaFormatError,
    },
    #[error("轨道{track_index}缺少有效 MIME")]
    MissingMime { track_index: usize },

    #[error("")]
    MissingField {
        track_index: usize,
        field: &'static str,
    },
    #[error("")]
    InvalidField {
        track_index: usize,
        field: &'static str,
        value: i64,
    },

    #[error("")]
    EmptyCodecConfig,
    #[error("")]
    TrackIndexOutOfRange { track_index: usize },
}

pub(crate) fn find_aac_track(
    extractor: &NativeMediaExtractor,
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
        if mime_type == "audio/mp4a-latm" {
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
    let codec_config =
        track_format
            .get_buffer(c"csd-0")
            .map_err(|error| TrackProbeError::Format {
                track_index,
                source: error,
            })?;

    if let Some(codec_config) = codec_config {
        if codec_config.is_empty() {
            return Err(TrackProbeError::EmptyCodecConfig);
        }
        return Ok(codec_config);
    }

    Err(TrackProbeError::MissingField {
        track_index,
        field: "csd-0",
    })
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
