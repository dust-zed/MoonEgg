use std::fs::File;

use moonegg_core::{
    media::{MediaTime, Packet, Timestamp, TrackInfo},
    ports::{DemuxError, Demuxer, ReadPacketResult},
};
use ndk_sys::{
    AMEDIAEXTRACTOR_SAMPLE_FLAG_ENCRYPTED, AMEDIAEXTRACTOR_SAMPLE_FLAG_SYNC, media_status_t,
};

use crate::{
    media_extractor::{MediaExtractorError, NativeMediaExtractor},
    media_format::MediaFormatError,
    track_probe::{TrackProbeError, probe_aac_track},
};

fn map_extractor_error(error: MediaExtractorError) -> DemuxError {
    match error {
        MediaExtractorError::InvalidSourceRange { .. }
        | MediaExtractorError::InvalidSeekTarget { .. }
        | MediaExtractorError::InvalidTrackIndex { .. } => DemuxError::InvalidData,
        MediaExtractorError::SetDataSourceFailed { status }
        | MediaExtractorError::SelectTrackFailed { status, .. }
        | MediaExtractorError::SeekFailed { status, .. } => map_media_status(status),
        MediaExtractorError::CreateFailed
        | MediaExtractorError::GetTrackFormatFailed { .. }
        | MediaExtractorError::ReadSampleFailed => DemuxError::Platform,
        MediaExtractorError::InvalidReadSize { .. } => DemuxError::Platform,
    }
}

fn map_media_status(status: i32) -> DemuxError {
    match status {
        status if status == media_status_t::AMEDIA_ERROR_IO.0 => DemuxError::Io,
        status if status == media_status_t::AMEDIA_ERROR_UNSUPPORTED.0 => DemuxError::Unsupported,
        status if status == media_status_t::AMEDIA_ERROR_MALFORMED.0 => DemuxError::InvalidData,
        _ => DemuxError::Platform,
    }
}

fn map_probe_error(error: TrackProbeError) -> DemuxError {
    match error {
        TrackProbeError::Extractor { source } => map_extractor_error(source),
        TrackProbeError::Format { source, .. } => match source {
            MediaFormatError::InvalidUtf8 => DemuxError::InvalidData,
            MediaFormatError::NullBufferPointer
            | MediaFormatError::NullStringPointer
            | MediaFormatError::InvalidBufferSize { .. } => DemuxError::Platform,
        },
        TrackProbeError::MissingMime { .. }
        | TrackProbeError::MissingField { .. }
        | TrackProbeError::InvalidField { .. }
        | TrackProbeError::EmptyCodecConfig => DemuxError::InvalidData,
        TrackProbeError::TrackIndexOutOfRange { .. } => DemuxError::Unsupported,
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AndroidDemuxerError {
    #[error("")]
    Extractor { source: MediaExtractorError },
    #[error("")]
    Probe { source: TrackProbeError },
    #[error("")]
    NoAacTrack,
    #[error("")]
    UnexpectedTrack { track_index: usize },
    #[error("")]
    MissingSampleSize,
    #[error("")]
    InvalidSampleSize { size: usize, limit: usize },
    #[error("")]
    MissingSampleTime,
    #[error("")]
    EncryptedSample,
    #[error("")]
    SampleSizeMismatch { expected: usize, actual: usize },
    #[error("")]
    InvalidSeekTarget { target_ns: i64 },
    #[error("")]
    NoSampleAfterSeek,
    #[error("")]
    TimestampOutOfRange { time_us: i64 },
}

impl AndroidDemuxerError {
    fn into_demux_error(self) -> DemuxError {
        match self {
            AndroidDemuxerError::Extractor { source } => map_extractor_error(source),
            AndroidDemuxerError::Probe { source } => map_probe_error(source),
            AndroidDemuxerError::NoAacTrack | AndroidDemuxerError::EncryptedSample => {
                DemuxError::Unsupported
            }
            AndroidDemuxerError::InvalidSampleSize { size, limit } => {
                if size > limit {
                    DemuxError::Unsupported
                } else {
                    DemuxError::InvalidData
                }
            }
            AndroidDemuxerError::UnexpectedTrack { .. }
            | AndroidDemuxerError::SampleSizeMismatch { .. } => DemuxError::Platform,
            AndroidDemuxerError::MissingSampleSize
            | AndroidDemuxerError::MissingSampleTime
            | AndroidDemuxerError::InvalidSeekTarget { .. }
            | AndroidDemuxerError::NoSampleAfterSeek => DemuxError::InvalidData,
            AndroidDemuxerError::TimestampOutOfRange { .. } => DemuxError::Unsupported,
        }
    }
}

// 1MiB
const MAX_SAMPLE_BYTES: usize = 1024 * 1024;

pub(crate) struct AndroidDemuxer {
    extractor: NativeMediaExtractor,
    tracks: Vec<TrackInfo>,
    source_ended: bool,
}

impl AndroidDemuxer {
    pub(crate) fn from_file(
        file: File,
        start: u64,
        length: u64,
    ) -> Result<Self, AndroidDemuxerError> {
        let mut extractor = NativeMediaExtractor::from_file(file, start, length)
            .map_err(|err| AndroidDemuxerError::Extractor { source: err })?;

        let track = probe_aac_track(&extractor)
            .map_err(|error| AndroidDemuxerError::Probe { source: error })?
            .ok_or(AndroidDemuxerError::NoAacTrack)?;

        let track_index = track.id().value() as usize;

        extractor
            .select_track(track_index)
            .map_err(|error| AndroidDemuxerError::Extractor { source: error })?;

        let tracks = vec![track];

        Ok(Self {
            extractor,
            tracks,
            source_ended: false,
        })
    }

    pub(crate) fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    fn read_packet(&mut self) -> Result<ReadPacketResult, AndroidDemuxerError> {
        if self.source_ended {
            return Ok(ReadPacketResult::EndOfStream);
        }

        let track_index = match self.extractor.sample_track_index() {
            Some(track_index) => track_index,
            None => {
                self.source_ended = true;
                return Ok(ReadPacketResult::EndOfStream);
            }
        };

        let track_info = self
            .tracks
            .iter()
            .find(|track| track_index == track.id().value() as usize)
            .ok_or(AndroidDemuxerError::UnexpectedTrack { track_index })?;

        let time_base = track_info.time_base();
        let track_id = track_info.id();

        let sample_size = self
            .extractor
            .sample_size()
            .ok_or(AndroidDemuxerError::MissingSampleSize)?;

        if sample_size == 0 || sample_size > MAX_SAMPLE_BYTES {
            return Err(AndroidDemuxerError::InvalidSampleSize {
                size: sample_size,
                limit: MAX_SAMPLE_BYTES,
            });
        }

        let pts_us = self
            .extractor
            .sample_time_us()
            .ok_or(AndroidDemuxerError::MissingSampleTime)?;

        // Android 返回的 PTS 单位为微秒，轨道时间基也为微秒，因此可直接作为 ticks。
        let timestamp = Timestamp::new(pts_us, time_base);

        let sample_flags = self.extractor.sample_flags();
        if (sample_flags & AMEDIAEXTRACTOR_SAMPLE_FLAG_ENCRYPTED) != 0 {
            return Err(AndroidDemuxerError::EncryptedSample);
        }

        let is_keyframe = (sample_flags & AMEDIAEXTRACTOR_SAMPLE_FLAG_SYNC) != 0;
        let mut data = vec![0u8; sample_size];
        let bytes_read = self
            .extractor
            .read_sample_data(&mut data)
            .map_err(|error| AndroidDemuxerError::Extractor { source: error })?;

        if bytes_read != sample_size {
            return Err(AndroidDemuxerError::SampleSizeMismatch {
                expected: sample_size,
                actual: bytes_read,
            });
        }

        let packet = Packet::new(track_id, data, Some(timestamp), None, None, is_keyframe);

        if !self.extractor.advance() {
            self.source_ended = true;
        }

        Ok(ReadPacketResult::Packet(packet))
    }

    fn seek(&mut self, target: MediaTime) -> Result<MediaTime, AndroidDemuxerError> {
        let target_ns = target.nanoseconds();
        if target_ns < 0 {
            return Err(AndroidDemuxerError::InvalidSeekTarget { target_ns });
        }
        let target_us = target_ns / 1000;

        self.extractor
            .seek_to_us(target_us)
            .map_err(|error| AndroidDemuxerError::Extractor { source: error })?;
        self.source_ended = false;

        let sample_track_index = self.extractor.sample_track_index();
        let Some(sample_track_index) = sample_track_index else {
            self.source_ended = true;
            return Err(AndroidDemuxerError::NoSampleAfterSeek);
        };

        let seek_track_exist = self
            .tracks
            .iter()
            .any(|track| sample_track_index == (track.id().value()) as usize);

        if !seek_track_exist {
            return Err(AndroidDemuxerError::UnexpectedTrack {
                track_index: sample_track_index,
            });
        }

        let landed_us = self
            .extractor
            .sample_time_us()
            .ok_or(AndroidDemuxerError::MissingSampleTime)?;

        let landed_ns = landed_us
            .checked_mul(1000)
            .ok_or(AndroidDemuxerError::TimestampOutOfRange { time_us: landed_us })?;

        Ok(MediaTime::from_nanoseconds(landed_ns))
    }
}

impl Demuxer for AndroidDemuxer {
    fn seek(&mut self, target: MediaTime) -> Result<MediaTime, DemuxError> {
        todo!()
    }
    fn tracks(&self) -> &[TrackInfo] {
        todo!()
    }
    fn read_packet(&mut self) -> Result<ReadPacketResult, DemuxError> {
        todo!()
    }
}
