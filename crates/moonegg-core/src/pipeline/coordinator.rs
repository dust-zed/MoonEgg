use std::time::Instant;

use crate::{
    media::DecodedFrame,
    ports::{VideoOutput, VideoOutputError, VideoSubmitResult},
    timing::{AvSync, AvSyncError, ClockSnapshot, VideoSyncDecision},
};

#[derive(Debug)]
pub enum VideoStepResult<T> {
    WaitUtil {
        deadline: Instant,
        frame: DecodedFrame<T>,
    },
    Submitted,
    Discarded,
    Backpressure(DecodedFrame<T>),
}

#[derive(Debug)]
pub enum CoordinateVideoError {
    Sync(AvSyncError),
    Output(VideoOutputError),
}

/// 协调函数
pub fn coordinate_video_frame<O>(
    sync: &AvSync,
    output: &mut O,
    frame: DecodedFrame<O::FramePayload>,
    audio_snapshot: ClockSnapshot,
    now: Instant,
) -> Result<VideoStepResult<O::FramePayload>, CoordinateVideoError>
where
    O: VideoOutput,
{
    match sync.decide(frame.pts(), audio_snapshot, now) {
        Ok(VideoSyncDecision::WaitUntil(deadline)) => {
            Ok(VideoStepResult::WaitUtil { deadline, frame })
        }
        Ok(VideoSyncDecision::PresentNow) => match output.present(frame) {
            Ok(VideoSubmitResult::Accepted) => Ok(VideoStepResult::Submitted),
            Ok(VideoSubmitResult::Backpressure(frame)) => Ok(VideoStepResult::Backpressure(frame)),
            Err(err) => Err(CoordinateVideoError::Output(err)),
        },
        Ok(VideoSyncDecision::Drop) => {
            output
                .discard(frame)
                .map_err(CoordinateVideoError::Output)?;
            Ok(VideoStepResult::Discarded)
        }
        Err(err) => Err(CoordinateVideoError::Sync(err)),
    }
}
