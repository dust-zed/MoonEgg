use std::{
    result,
    time::{Duration, Instant},
};

use ndk::audio::{
    AudioDirection, AudioError, AudioFormat, AudioSharingMode, AudioStream, AudioStreamBuilder,
    AudioStreamState,
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
    #[error("当前音频流状态不允许此操作")]
    InvalidState,
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

        let result = unsafe {
            self.stream
                .write(samples.as_ptr().cast(), requested_frames, 0)
        };

        let written = match result {
            // 当前 ndk 版本内部错误，这版本走不到 Ok
            Ok(frames) => frames,
            Err(error) => {
                let code: i32 = error.into();
                if code <= 0 {
                    return Err(AAudioError::Native(error));
                }
                code as u32
            }
        };

        let written = usize::try_from(written).map_err(|_| AAudioError::InvalidWriteCount)?;

        if written > frame_count {
            return Err(AAudioError::InvalidWriteCount);
        }

        Ok(written)
    }

    fn current_state(&self) -> Result<AudioStreamState, AAudioError> {
        let state = self
            .stream
            .wait_for_state_change(AudioStreamState::Unknown, 0)
            .map_err(AAudioError::Native)?;

        if state == AudioStreamState::Disconnected {
            return Err(AAudioError::Native(AudioError::Disconnected));
        }

        Ok(state)
    }

    fn wait_until(&self, expected: AudioStreamState) -> Result<(), AAudioError> {
        let deadline = Instant::now() + Duration::from_millis(500);

        loop {
            let state = self.current_state()?;

            if state == expected {
                return Ok(());
            }

            let remaining = deadline.saturating_duration_since(Instant::now());

            if remaining.is_zero() {
                return Err(AAudioError::Native(AudioError::Timeout));
            }

            let wait = remaining.min(Duration::from_millis(20));

            match self
                .stream
                .wait_for_state_change(state, wait.as_nanos() as i64)
            {
                Ok(_) | Err(AudioError::Timeout) => {}
                Err(error) => return Err(AAudioError::Native(error)),
            }
        }
    }

    pub(crate) fn request_start(&mut self) -> Result<(), AAudioError> {
        use AudioStreamState::*;

        match self.current_state()? {
            Started | Starting => Ok(()),
            Open | Paused | Flushed | Stopped => {
                self.stream.request_start().map_err(AAudioError::Native)
            }
            _ => Err(AAudioError::InvalidState),
        }
    }

    pub(crate) fn pause(&mut self) -> Result<(), AAudioError> {
        use AudioStreamState::*;

        match self.current_state()? {
            Open | Paused | Flushed | Stopped => return Ok(()),
            Pausing => {}
            Started | Starting => {
                self.stream.request_pause().map_err(AAudioError::Native)?;
            }
            _ => return Err(AAudioError::InvalidState),
        }
        self.wait_until(Paused)
    }

    pub(crate) fn flush(&mut self) -> Result<(), AAudioError> {
        self.pause()?;

        match self.current_state()? {
            AudioStreamState::Open | AudioStreamState::Flushed => Ok(()),
            AudioStreamState::Paused => {
                self.stream.request_flush().map_err(AAudioError::Native)?;
                self.wait_until(AudioStreamState::Flushed)
            }
            _ => Err(AAudioError::InvalidState),
        }
    }

    pub(crate) fn frame_counters(&self) -> Result<(u64, u64), AAudioError> {
        self.current_state()?;

        let consumed =
            u64::try_from(self.stream.frames_read()).map_err(|_| AAudioError::InvalidState)?;

        let written =
            u64::try_from(self.stream.frames_written()).map_err(|_| AAudioError::InvalidState)?;

        Ok((consumed, written))
    }
}
