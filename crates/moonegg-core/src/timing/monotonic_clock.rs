use std::time::Instant;

use crate::{
    media::{MediaDelta, MediaTime},
    timing::{ClockError, ClockSnapshot},
};

pub(crate) struct MonotonicClock {
    anchor_media: MediaTime,
    anchor_instant: Option<Instant>,
}

impl MonotonicClock {
    pub(crate) fn new(anchor_media: MediaTime) -> Self {
        Self {
            anchor_media,
            anchor_instant: None,
        }
    }

    pub(crate) fn snapshot(&self, now: Instant) -> Result<ClockSnapshot, ClockError> {
        if let Some(anchor) = self.anchor_instant
            && now < anchor
        {
            return Err(ClockError::ObservationFromFuture);
        }
        let elapsed_ns = match self.anchor_instant {
            Some(anchor) => now.duration_since(anchor).as_nanos(),
            None => 0,
        };
        let elapsed_ns = i64::try_from(elapsed_ns).map_err(|_| ClockError::Overflow)?;
        let media_time = self
            .anchor_media
            .checked_add(MediaDelta::from_nanoseconds(elapsed_ns))
            .map_err(|_| ClockError::Overflow)?;

        Ok(ClockSnapshot::new(media_time, now))
    }

    pub(crate) fn pause(&mut self, now: Instant) -> Result<(), ClockError> {
        let snapshot = self.snapshot(now)?;
        self.anchor_instant = None;
        self.anchor_media = snapshot.media_time();
        Ok(())
    }

    pub(crate) fn resume(&mut self, now: Instant) {
        if self.anchor_instant.is_none() {
            self.anchor_instant = Some(now);
        }
    }
}
