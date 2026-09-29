//! 负责
//! 队列、背压、EOS、flush、seek epoch、数据流协调
mod audio;
mod coordinator;
mod deferred_audio_output;
mod epoch;
mod playback;
mod queue;
pub(crate) use coordinator::{
    CoordinateVideoError, VideoDiscardReason, VideoStepResult, coordinate_video_frame,
};
pub(crate) use deferred_audio_output::DeferredAudioOutput;
pub(crate) use epoch::{EpochError, EpochItem, PlaybackEpoch};
pub(crate) use playback::{
    PlaybackPipeline, PlaybackPipelineError, PlaybackStepResult, SeekOutcome,
};
pub(crate) use queue::{BoundedQueue, QueueError, QueuePushResult};
