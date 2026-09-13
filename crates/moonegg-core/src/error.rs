//! 负责
//! 可恢复、致命、输入、后端和状态错误分类

use crate::{
    pipeline::{PlaybackEpoch, PlaybackPipelineError},
    ports::{AudioOutputError, DecodeError, DemuxError, VideoOutputError},
};
#[derive(Debug)]
pub enum PlaybackError {
    Demux(DemuxError),
    Decode(DecodeError),
    AudioOutput(AudioOutputError),
    VideoOutput(VideoOutputError),
    Pipeline(PlaybackPipelineError),
    Runtime(RuntimeError),
}

#[derive(Debug)]
pub enum RuntimeError {
    THreadSpawn(std::io::Error),
    WorkerDisconnected,
    WorkerPanicked,
    SessionFailed,

    EpochMismatch {
        expected: PlaybackEpoch,
        actual: PlaybackEpoch,
    },
}
