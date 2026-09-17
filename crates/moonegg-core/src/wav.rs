use std::ops::Range;

use crate::{
    media::{
        AudioCodecId, AudioTrackFormat, MediaTime, Packet, Rounding, TimeBase, TimeSpan, Timestamp,
        TrackFormat, TrackId, TrackInfo,
    },
    ports::{DemuxError, Demuxer, ReadPacketResult},
};

fn parse_pcm_format(data: &[u8]) -> Result<AudioTrackFormat, DemuxError> {
    if data.len() < 16 {
        return Err(DemuxError::InvalidData);
    }

    let format_tag = u16::from_le_bytes([data[0], data[1]]);
    let channels = u16::from_le_bytes([data[2], data[3]]);

    let sample_rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

    let byte_rate = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);

    let block_align = u16::from_le_bytes([data[12], data[13]]);
    let bits_per_sample = u16::from_le_bytes([data[14], data[15]]);

    // 当前仅支持普通的 16-bit PCM
    if format_tag != 1 || bits_per_sample != 16 {
        return Err(DemuxError::Unsupported);
    }

    if channels == 0 || sample_rate == 0 {
        return Err(DemuxError::InvalidData);
    }

    if !matches!(channels, 1 | 2) {
        return Err(DemuxError::Unsupported);
    }

    let expected_align = channels * 2;

    if block_align != expected_align {
        return Err(DemuxError::InvalidData);
    }

    let expected_bytes_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or(DemuxError::InvalidData)?;

    if byte_rate != expected_bytes_rate {
        return Err(DemuxError::InvalidData);
    }

    Ok(AudioTrackFormat::new(
        AudioCodecId::PcmS16Le,
        sample_rate,
        channels,
        Vec::new(),
    ))
}

fn parse_wav(data: &[u8]) -> Result<WavInfo, DemuxError> {
    if data.len() < 12 {
        return Err(DemuxError::InvalidData);
    }

    if &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(DemuxError::InvalidData);
    }

    let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

    let riff_size = usize::try_from(riff_size).map_err(|_| DemuxError::InvalidData)?;

    let riff_end = 8usize
        .checked_add(riff_size)
        .ok_or(DemuxError::InvalidData)?;

    if riff_end < 12 || riff_end > data.len() {
        return Err(DemuxError::InvalidData);
    }

    let mut format = None;
    let mut data_range = None;
    let mut cursor = 12;

    while cursor < riff_end {
        if riff_end - cursor < 8 {
            return Err(DemuxError::InvalidData);
        }

        let chunk_id = &data[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes([
            data[cursor + 4],
            data[cursor + 5],
            data[cursor + 6],
            data[cursor + 7],
        ]);

        let chunk_size = usize::try_from(chunk_size).map_err(|_| DemuxError::InvalidData)?;

        let payload_start = cursor + 8;

        if chunk_size > riff_end - payload_start {
            return Err(DemuxError::InvalidData);
        }

        let payload_end = payload_start + chunk_size;
        let padding = chunk_size % 2;

        if padding > riff_end - payload_end {
            return Err(DemuxError::InvalidData);
        }

        match chunk_id {
            b"fmt " => {
                if format.is_some() {
                    return Err(DemuxError::Unsupported);
                }

                format = Some(parse_pcm_format(&data[payload_start..payload_end])?);
            }
            b"data" => {
                if data_range.is_some() {
                    return Err(DemuxError::Unsupported);
                }

                data_range = Some(payload_start..payload_end)
            }
            _ => {}
        }

        cursor = payload_end + padding;
    }
    let format = format.ok_or(DemuxError::InvalidData)?;
    let data_range = data_range.ok_or(DemuxError::InvalidData)?;

    let bytes_per_frame = usize::from(format.channel_count()) * 2;
    let data_len = data_range.end - data_range.start;

    if data_len % bytes_per_frame != 0 {
        return Err(DemuxError::InvalidData);
    }

    Ok(WavInfo { format, data_range })
}

#[derive(Debug)]
struct WavInfo {
    format: AudioTrackFormat,
    data_range: Range<usize>,
}

impl WavInfo {
    fn frame_count(&self) -> usize {
        let data_len = self.data_range.end - self.data_range.start;
        let bytes_per_frame = usize::from(self.format.channel_count()) * 2;

        data_len / bytes_per_frame
    }

    fn duration_seconds(&self) -> f64 {
        self.frame_count() as f64 / f64::from(self.format.sample_rate())
    }
}

pub(crate) struct WavDemuxer {
    data: Vec<u8>,
    info: WavInfo,
    next_frame: usize,
    time_base: TimeBase,

    tracks: [TrackInfo; 1],
}

impl WavDemuxer {
    pub(crate) fn from_bytes(data: Vec<u8>) -> Result<Self, DemuxError> {
        let info = parse_wav(&data)?;

        let time_base =
            TimeBase::from_hz(info.format.sample_rate()).map_err(|_| DemuxError::InvalidData)?;

        let duration_ticks =
            u64::try_from(info.frame_count()).map_err(|_| DemuxError::InvalidData)?;

        let track = TrackInfo::new(
            TrackId::new(0),
            time_base,
            Some(0),
            Some(duration_ticks),
            TrackFormat::Audio(info.format.clone()),
        );

        Ok(Self {
            data,
            info,
            next_frame: 0,
            time_base,

            tracks: [track],
        })
    }

    pub(crate) fn read_packet(&mut self) -> Result<ReadPacketResult, DemuxError> {
        const FRAMES_PER_PACKET: usize = 1024;

        let total_frames = self.info.frame_count();

        if self.next_frame >= total_frames {
            return Ok(ReadPacketResult::EndOfStream);
        }

        // 1. 决定本次读取多少个完整 frame。
        let remaining_frames = total_frames - self.next_frame;
        let frames_to_read = remaining_frames.min(FRAMES_PER_PACKET);

        let bytes_per_frame = usize::from(self.info.format.channel_count()) * 2;
        let bytes_start = self.info.data_range.start + self.next_frame * bytes_per_frame;
        let bytes_end = bytes_start + frames_to_read * bytes_per_frame;

        let start_stick = i64::try_from(self.next_frame).map_err(|_| DemuxError::InvalidData)?;
        let duration_ticks = u64::try_from(frames_to_read).map_err(|_| DemuxError::InvalidData)?;

        let timestamp = Timestamp::new(start_stick, self.time_base);

        let packet = Packet::new(
            TrackId::new(0),
            self.data[bytes_start..bytes_end].to_vec(),
            Some(timestamp),
            Some(timestamp),
            Some(TimeSpan::new(duration_ticks, self.time_base)),
            true,
        );

        self.next_frame += frames_to_read;

        Ok(ReadPacketResult::Packet(packet))
    }
}

impl Demuxer for WavDemuxer {
    fn read_packet(&mut self) -> Result<ReadPacketResult, DemuxError> {
        WavDemuxer::read_packet(self)
    }

    fn seek(&mut self, target: MediaTime) -> Result<MediaTime, DemuxError> {
        // 1. 负数目标按 0 处理。
        let target_ns = target.nanoseconds().max(0);

        let request_frame =
            i128::from(target_ns) * i128::from(self.info.format.sample_rate()) / 1_000_000_000;

        let total_frames =
            i128::try_from(self.info.frame_count()).map_err(|_| DemuxError::InvalidData)?;

        let frame = request_frame.min(total_frames);
        let next_frame = usize::try_from(frame).map_err(|_| DemuxError::InvalidData)?;

        // 计算实际落点对应的媒体时间
        let ticks = i64::try_from(next_frame).map_err(|_| DemuxError::InvalidData)?;
        let landed = Timestamp::new(ticks, self.time_base)
            .to_media_time(Rounding::TowardZero)
            .map_err(|_| DemuxError::InvalidData)?;

        self.next_frame = next_frame;
        Ok(landed)
    }

    fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }
}
