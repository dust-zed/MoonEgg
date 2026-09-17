use crate::{
    media::{AudioCodecId, AudioTrackFormat},
    ports::DemuxError,
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

fn parse_wav_format(data: &[u8]) -> Result<AudioTrackFormat, DemuxError> {
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

        if chunk_id == b"fmt " {
            return parse_pcm_format(&data[payload_start..payload_end]);
        }

        cursor = payload_end + padding;
    }
    Err(DemuxError::InvalidData)
}
