//! 负责
//! 队列、背压、EOS、flush、seek epoch、数据流协调
mod coordinator;
mod epoch;
mod queue;
pub(crate) use coordinator::{CoordinateVideoError, VideoStepResult, coordinate_video_frame};
