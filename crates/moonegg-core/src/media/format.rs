//! 压缩轨道的编解码格式描述。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodecId {
    PcmS16Le,
    Aac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodecId {
    H264,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrackFormat {
    codec: AudioCodecId,
    sample_rate: u32,
    channel_count: u16,
    codec_config: Vec<u8>,
}

impl AudioTrackFormat {
    pub fn new(
        codec: AudioCodecId,
        sample_rate: u32,
        channel_count: u16,
        codec_config: Vec<u8>,
    ) -> Self {
        Self {
            codec,
            sample_rate,
            channel_count,
            codec_config,
        }
    }

    pub const fn codec(&self) -> AudioCodecId {
        self.codec
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn channel_count(&self) -> u16 {
        self.channel_count
    }

    pub fn codec_config(&self) -> &[u8] {
        &self.codec_config
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrackFormat {
    codec: VideoCodecId,
    width: u32,
    height: u32,
    codec_config: Vec<u8>,
}

impl VideoTrackFormat {
    pub fn new(codec: VideoCodecId, width: u32, height: u32, codec_config: Vec<u8>) -> Self {
        Self {
            codec,
            width,
            height,
            codec_config,
        }
    }

    pub const fn codec(&self) -> VideoCodecId {
        self.codec
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn codec_config(&self) -> &[u8] {
        &self.codec_config
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackFormat {
    Audio(AudioTrackFormat),
    Video(VideoTrackFormat),
}
