use std::ops::Range;

use crate::{
    media::{
        AudioCodecId, AudioTrackFormat, MediaTime, Packet, Rounding, TimeBase, TimeSpan, Timestamp,
        TrackFormat, TrackId, TrackInfo,
    },
    ports::{DemuxError, Demuxer, ReadPacketResult},
};

use std::io::{ErrorKind, Read, Seek, SeekFrom};

// 输入从偏移 0 开始表示完整 WAV；成功后游标位于 12。
fn read_riff_header<R: Read + Seek>(reader: &mut R) -> Result<u64, DemuxError> {
    let source_len = reader.seek(SeekFrom::End(0)).map_err(|_| DemuxError::Io)?;
    if source_len < 12 {
        return Err(DemuxError::InvalidData);
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| DemuxError::Io)?;
    let mut header = [0u8; 12];
    reader.read_exact(&mut header).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            DemuxError::InvalidData
        } else {
            DemuxError::Io
        }
    })?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(DemuxError::InvalidData);
    }
    let riff_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let riff_end = u64::from(riff_size) + 8;

    if riff_end < 12 || riff_end > source_len {
        return Err(DemuxError::InvalidData);
    }
    Ok(riff_end)
}

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

#[derive(Debug)]
struct WavChunk {
    id: [u8; 4],
    payload: Range<u64>,
    next_offset: u64,
}

// riff_end 应来自已校验的 RIFF 头
// 调用前游标应位于 chunk 头部或 riff_end
// 返回 Some 时只保证头部和声明范围合法，尚未读取 payload。
fn read_chunk_header<R: Read + Seek>(
    reader: &mut R,
    riff_end: u64,
) -> Result<Option<WavChunk>, DemuxError> {
    let chunk_start = reader.stream_position().map_err(|_| DemuxError::Io)?;

    if chunk_start == riff_end {
        return Ok(None);
    }

    if chunk_start > riff_end {
        return Err(DemuxError::InvalidData);
    }

    if riff_end - chunk_start < 8 {
        return Err(DemuxError::InvalidData);
    }

    let mut header = [0; 8];
    reader.read_exact(&mut header).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            DemuxError::InvalidData
        } else {
            DemuxError::Io
        }
    })?;
    let id = [header[0], header[1], header[2], header[3]];
    let chunk_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let chunk_size = u64::from(chunk_size);

    let payload_start = chunk_start.checked_add(8).ok_or(DemuxError::InvalidData)?;
    let payload_end = payload_start
        .checked_add(chunk_size)
        .ok_or(DemuxError::InvalidData)?;
    let next_offset = payload_end
        .checked_add(chunk_size % 2)
        .ok_or(DemuxError::InvalidData)?;

    if payload_end > riff_end || next_offset > riff_end {
        return Err(DemuxError::InvalidData);
    }

    Ok(Some(WavChunk {
        id,
        payload: payload_start..payload_end,
        next_offset,
    }))
}

#[derive(Debug)]
struct WavLayout {
    format: AudioTrackFormat,
    data_range: Range<u64>,
}

impl WavLayout {
    fn bytes_per_frame(&self) -> u64 {
        u64::from(self.format.channel_count()) * 2
    }

    fn frame_count(&self) -> u64 {
        let data_len = self.data_range.end - self.data_range.start;
        data_len / self.bytes_per_frame()
    }
}

pub(crate) struct WavDemuxer<R> {
    reader: R,
    layout: WavLayout,
    next_frame: u64,
    time_base: TimeBase,
    tracks: [TrackInfo; 1],
}

impl<R: Read + Seek> WavDemuxer<R> {
    pub(crate) fn from_reader(mut reader: R) -> Result<Self, DemuxError> {
        let layout = read_wav_layout(&mut reader)?;
        let time_base =
            TimeBase::from_hz(layout.format.sample_rate()).map_err(|_| DemuxError::InvalidData)?;
        let duration_ticks = layout.frame_count();

        let track_info = TrackInfo::new(
            TrackId::new(0),
            time_base,
            Some(0),
            Some(duration_ticks),
            TrackFormat::Audio(layout.format.clone()),
        );
        Ok(Self {
            reader,
            layout,
            next_frame: 0,
            time_base,
            tracks: [track_info],
        })
    }
}

impl<R> Demuxer for WavDemuxer<R>
where
    R: Read + Seek,
{
    fn read_packet(&mut self) -> Result<ReadPacketResult, DemuxError> {
        const FRAMES_PER_PACKET: u64 = 1024;

        let total_frames = self.layout.frame_count();
        if self.next_frame >= total_frames {
            return Ok(ReadPacketResult::EndOfStream);
        }
        let remaining_frames = total_frames - self.next_frame;
        let frames_to_read = remaining_frames.min(FRAMES_PER_PACKET);

        let bytes_per_frame = self.layout.bytes_per_frame();
        let byte_offset = self.layout.data_range.start + self.next_frame * bytes_per_frame;
        let packet_len = frames_to_read * bytes_per_frame;

        let start_ticks = i64::try_from(self.next_frame).map_err(|_| DemuxError::InvalidData)?;
        let timestamp = Timestamp::new(start_ticks, self.time_base);
        let duration = TimeSpan::new(frames_to_read, self.time_base);

        let packet_len = usize::try_from(packet_len).map_err(|_| DemuxError::InvalidData)?;

        let mut packet_data = vec![0u8; packet_len];
        self.reader
            .seek(SeekFrom::Start(byte_offset))
            .map_err(|_| DemuxError::Io)?;
        self.reader.read_exact(&mut packet_data).map_err(|error| {
            if error.kind() == ErrorKind::UnexpectedEof {
                DemuxError::InvalidData
            } else {
                DemuxError::Io
            }
        })?;

        let packet = Packet::new(
            self.tracks[0].id(),
            packet_data,
            Some(timestamp),
            Some(timestamp),
            Some(duration),
            true,
        );

        self.next_frame += frames_to_read;
        Ok(ReadPacketResult::Packet(packet))
    }

    fn seek(&mut self, target: MediaTime) -> Result<MediaTime, DemuxError> {
        let target_ns = target.nanoseconds().max(0);
        let requested_frame =
            i128::from(target_ns) * i128::from(self.layout.format.sample_rate()) / 1_000_000_000;
        let total_frames = i128::from(self.layout.frame_count());
        let next_frame = u64::try_from(requested_frame.min(total_frames))
            .map_err(|_| DemuxError::InvalidData)?;

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

fn read_wav_layout<R: Read + Seek>(reader: &mut R) -> Result<WavLayout, DemuxError> {
    let riff_end = read_riff_header(reader)?;

    let mut format = None;
    let mut data_range = None;

    while let Some(chunk) = read_chunk_header(reader, riff_end)? {
        match &chunk.id {
            b"fmt " => {
                if let Some(format) = format {
                    return Err(DemuxError::Unsupported);
                }
                let payload_len = chunk.payload.end - chunk.payload.start;
                if payload_len < 16 {
                    return Err(DemuxError::InvalidData);
                }
                let mut format_bytes = [0u8; 16];
                reader.read_exact(&mut format_bytes).map_err(|error| {
                    if error.kind() == ErrorKind::UnexpectedEof {
                        DemuxError::InvalidData
                    } else {
                        DemuxError::Io
                    }
                })?;

                format = Some(parse_pcm_format(&format_bytes)?);
            }
            b"data" => {
                if let Some(range) = data_range {
                    return Err(DemuxError::Unsupported);
                }
                data_range = Some(chunk.payload);
            }
            _ => {}
        }
        reader
            .seek(SeekFrom::Start(chunk.next_offset))
            .map_err(|error| {
                if error.kind() == ErrorKind::UnexpectedEof {
                    DemuxError::InvalidData
                } else {
                    DemuxError::Io
                }
            })?;
    }

    let (Some(format), Some(data_range)) = (format, data_range) else {
        return Err(DemuxError::InvalidData);
    };

    let bytes_per_frame = u64::from(format.channel_count()) * 2;
    let data_len = data_range.end - data_range.start;

    if data_len % bytes_per_frame != 0 {
        return Err(DemuxError::InvalidData);
    }

    Ok(WavLayout { format, data_range })
}
