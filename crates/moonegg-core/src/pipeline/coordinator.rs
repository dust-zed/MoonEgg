use std::time::Instant;

use crate::{
    media::DecodedFrame,
    ports::{VideoOutput, VideoOutputError, VideoSubmitResult},
    timing::{AvSync, AvSyncError, ClockSnapshot, VideoSyncDecision},
};

#[derive(Debug)]
pub enum VideoStepResult<T> {
    WaitUntil {
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
            Ok(VideoStepResult::WaitUntil { deadline, frame })
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        media::{DecodedFrame, MediaDelta, MediaTime, TrackId},
        pipeline::{VideoStepResult, coordinate_video_frame},
        ports::VideoOutput,
        timing::{AvSync, ClockSnapshot, VideoSyncDecision},
    };

    #[derive(Debug, Default)]
    struct RecordingVideoOutput {
        presented: Vec<u32>,
        discarded: Vec<u32>,
        backpressure: bool,
    }

    impl VideoOutput for RecordingVideoOutput {
        type FramePayload = u32;

        fn present(
            &mut self,
            frame: crate::media::DecodedFrame<Self::FramePayload>,
        ) -> Result<
            crate::ports::VideoSubmitResult<Self::FramePayload>,
            crate::ports::VideoOutputError,
        > {
            if self.backpressure {
                return Ok(crate::ports::VideoSubmitResult::Backpressure(frame));
            }
            self.presented.push(frame.into_payload());
            Ok(crate::ports::VideoSubmitResult::Accepted)
        }

        fn discard(
            &mut self,
            frame: crate::media::DecodedFrame<Self::FramePayload>,
        ) -> Result<(), crate::ports::VideoOutputError> {
            self.discarded.push(frame.into_payload());
            Ok(())
        }

        fn flush(&mut self) -> Result<(), crate::ports::VideoOutputError> {
            Ok(())
        }
    }

    const NANOS_PER_MILLISECOND: i64 = 1_000_000;

    fn media_time_ms(milliseconds: i64) -> MediaTime {
        MediaTime::from_nanoseconds(milliseconds * NANOS_PER_MILLISECOND)
    }

    fn media_delta_ms(milliseconds: i64) -> MediaDelta {
        MediaDelta::from_nanoseconds(milliseconds * NANOS_PER_MILLISECOND)
    }

    fn video_frame(pts_ms: i64, token: u32) -> DecodedFrame<u32> {
        DecodedFrame::new(TrackId::new(1), media_time_ms(pts_ms), token)
    }

    fn av_sync() -> AvSync {
        AvSync::new(
            media_delta_ms(10), // 允许提前 10 ms
            media_delta_ms(50), // 允许落后 50 ms
        )
        .unwrap()
    }

    fn audio_snapshot(observed_at: Instant) -> ClockSnapshot {
        ClockSnapshot::new(media_time_ms(1_000), observed_at)
    }

    #[test]
    fn returns_early_frame_to_pipeline_with_deadline() {
        let now = Instant::now();
        let snapshot = audio_snapshot(now);
        let mut output = RecordingVideoOutput::default();

        let result = coordinate_video_frame(
            &av_sync(),
            &mut output,
            video_frame(1_080, 7),
            snapshot,
            now,
        )
        .unwrap();

        match result {
            VideoStepResult::WaitUntil { deadline, frame } => {
                assert_eq!(deadline.duration_since(now), Duration::from_millis(70));
                assert_eq!(frame.into_payload(), 7);
            }

            other => panic!("expected WaitUntil, got {other:?}"),
        }
        assert!(output.presented.is_empty());
        assert!(output.discarded.is_empty());
    }

    #[test]
    fn submits_frame_inside_tolerance_window() {
        let now = Instant::now();
        let snapshot = audio_snapshot(now);
        let mut output = RecordingVideoOutput::default();

        let result =
            coordinate_video_frame(&av_sync(), &mut output, video_frame(1005, 8), snapshot, now)
                .unwrap();
        assert!(matches!(result, VideoStepResult::Submitted));
        assert_eq!(output.presented.as_slice(), &[8]);
        assert!(output.discarded.is_empty());
    }

    #[test]
    fn discards_frame_later_than_late_tolerance() {
        let now = Instant::now();
        let snapshot = audio_snapshot(now);
        let mut output = RecordingVideoOutput::default();

        let result =
            coordinate_video_frame(&av_sync(), &mut output, video_frame(940, 9), snapshot, now)
                .unwrap();
        assert!(matches!(result, VideoStepResult::Discarded));
        assert!(output.presented.is_empty());
        assert_eq!(output.discarded.as_slice(), &[9]);
    }

    #[test]
    fn returns_frame_when_video_output_applies_backpressure() {
        let now = Instant::now();
        let snapshot = audio_snapshot(now);
        let mut output = RecordingVideoOutput {
            backpressure: true,
            ..RecordingVideoOutput::default()
        };

        let result = coordinate_video_frame(
            &av_sync(),
            &mut output,
            video_frame(1000, 10),
            snapshot,
            now,
        )
        .unwrap();

        match result {
            VideoStepResult::Backpressure(frame) => {
                assert_eq!(frame.into_payload(), 10);
            }
            other => panic!("expected Backpressure, got {other:?}"),
        }
        assert!(output.discarded.is_empty());
        assert!(output.presented.is_empty());
    }

    #[test]
    fn presents_frames_extractly_on_tolerance_boundaries() {
        let now = Instant::now();
        let snapshot = audio_snapshot(now);
        let sync = av_sync();

        for video_pts_ms in [1_010, 950] {
            let decision = sync
                .decide(media_time_ms(video_pts_ms), snapshot, now)
                .unwrap();
            assert!(matches!(decision, VideoSyncDecision::PresentNow));
        }
    }
}
