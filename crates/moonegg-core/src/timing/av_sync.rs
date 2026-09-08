use std::time::{Duration, Instant};

use crate::{
    media::{MediaDelta, MediaTime, TimeError},
    timing::ClockSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSyncDecision {
    WaitUntil(Instant),
    PresentNow,
    Drop,
}

#[derive(Debug)]
pub struct AvSync {
    early_tolerance: MediaDelta,
    late_tolerance: MediaDelta,
}

impl AvSync {
    pub fn new(
        early_tolerance: MediaDelta,
        late_tolerance: MediaDelta,
    ) -> Result<Self, AvSyncError> {
        if early_tolerance.nanoseconds() < 0 || late_tolerance.nanoseconds() < 0 {
            return Err(AvSyncError::InvalidTolerance);
        }

        Ok(Self {
            early_tolerance,
            late_tolerance,
        })
    }

    pub fn decide(
        &self,
        video_pts: MediaTime,
        audio_snapshot: ClockSnapshot,
        now: Instant,
    ) -> Result<VideoSyncDecision, AvSyncError> {
        let elapsed = now
            .checked_duration_since(audio_snapshot.observed_at())
            .ok_or(AvSyncError::ObservationFromFuture)?;
        let elapsed = i64::try_from(elapsed.as_nanos()).map_err(|_| AvSyncError::Overflow)?;
        let audio_now = audio_snapshot
            .media_time()
            .checked_add(MediaDelta::from_nanoseconds(elapsed))
            .map_err(|_| AvSyncError::Overflow)?;

        let offset = video_pts
            .checked_delta(audio_now)
            .map_err(|_| AvSyncError::Overflow)?;

        if offset > self.early_tolerance {
            let wait_nanoseconds = offset
                .nanoseconds()
                .checked_sub(self.early_tolerance.nanoseconds())
                .ok_or(AvSyncError::Overflow)?;
            let wait_duration = Duration::from_nanos(
                u64::try_from(wait_nanoseconds).map_err(|_| AvSyncError::Overflow)?,
            );
            let deadline = now
                .checked_add(wait_duration)
                .ok_or(AvSyncError::Overflow)?;
            return Ok(VideoSyncDecision::WaitUntil(deadline));
        }

        let late_boundary = self
            .late_tolerance
            .nanoseconds()
            .checked_neg()
            .ok_or(AvSyncError::Overflow)?;

        if offset.nanoseconds() < late_boundary {
            return Ok(VideoSyncDecision::Drop);
        }

        Ok(VideoSyncDecision::PresentNow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvSyncError {
    ObservationFromFuture,
    Overflow,
    InvalidTolerance,
}
