use ndk::audio::{
    AudioDirection, AudioError, AudioFormat, AudioSharingMode, AudioStream, AudioStreamBuilder,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AAudioError {
    #[error("无效的 PCM 参数")]
    InvalidFormat,
    #[error(
        "输出格式不匹配: 期望 {expected_rate} Hz / {expected_channels} 声道 / I16，、
        实际 {actual_rate} Hz / {actual_channels} 声道 / {actual_format:?}"
    )]
    FormatMismatch {
        expected_rate: u32,
        expected_channels: u16,
        actual_rate: i32,
        actual_channels: i32,
        actual_format: AudioFormat,
    },
    #[error("PCM 数据没有完整音频帧对齐")]
    IncompleteFrame,
    #[error("单次写入的音频帧数过大")]
    BufferTooLarge,
    #[error("AAudio 返回的写入帧数超出请求范围")]
    InvalidWriteCount,
    #[error("AAudio 调用失败: {0:?}")]
    Native(AudioError),
}

pub(crate) struct AAudioPcmStream {
    stream: AudioStream,
    sample_rate: u32,
    channels: u16,
}

impl AAudioPcmStream {
    pub(crate) fn new(sample_rate: u32, channels: u16) -> Result<Self, AAudioError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AAudioError::InvalidFormat);
        }

        let requested_rate = i32::try_from(sample_rate).map_err(|_| AAudioError::InvalidFormat)?;
        let requested_channels = i32::from(channels);

        let stream = AudioStreamBuilder::new()
            .map_err(AAudioError::Native)?
            .direction(AudioDirection::Output)
            .sharing_mode(AudioSharingMode::Shared)
            .format(AudioFormat::PCM_I16)
            .sample_rate(requested_rate)
            .channel_count(requested_channels)
            .open_stream()
            .map_err(AAudioError::Native)?;

        let actual_rate = stream.sample_rate();
        let actual_channels = stream.channel_count();
        let actual_format = stream.format();

        if actual_format != AudioFormat::PCM_I16
            || actual_channels != requested_channels
            || actual_rate != requested_rate
        {
            return Err(AAudioError::FormatMismatch {
                expected_rate: sample_rate,
                expected_channels: channels,
                actual_rate,
                actual_channels,
                actual_format,
            });
        }

        Ok(Self {
            stream,
            sample_rate,
            channels,
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(&self) -> u16 {
        self.channels
    }

    /// 返回本次实际写入的 audio frame 数量
    pub(crate) fn write_i16(&mut self, samples: &[i16]) -> Result<usize, AAudioError> {
        let channels = usize::from(self.channels);

        if samples.len() % channels != 0 {
            return Err(AAudioError::IncompleteFrame);
        }
        let frame_count = samples.len() / channels;

        if frame_count == 0 {
            return Ok(0);
        }

        let requested_frames =
            i32::try_from(frame_count).map_err(|_| AAudioError::BufferTooLarge)?;

        let written = unsafe {
            self.stream
                .write(samples.as_ptr().cast(), requested_frames, 0)
        }
        .map_err(AAudioError::Native)?;

        let written = usize::try_from(written).map_err(|_| AAudioError::InvalidWriteCount)?;

        if written > frame_count {
            return Err(AAudioError::InvalidWriteCount);
        }

        Ok(written)
    }
}
