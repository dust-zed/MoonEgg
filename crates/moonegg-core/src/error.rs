//! 负责
//! 可恢复、致命、输入、后端和状态错误分类

use crate::ports::{AudioOutputError, DecodeError, DemuxError, VideoOutputError};
#[derive(Debug)]
pub enum PlaybackError {
    Demux(DemuxError),
    Decode(DecodeError),
    AudioOutput(AudioOutputError),
    VideoOutput(VideoOutputError),
}
