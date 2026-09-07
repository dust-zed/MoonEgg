//! 媒体轨道标识和轨道描述

use crate::media::{TimeBase, TimeSpan, Timestamp, TrackFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackId(u32);

impl TrackId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    id: TrackId,
    time_base: TimeBase,
    start_ticks: Option<i64>,
    duration_ticks: Option<u64>,
    format: TrackFormat,
}

impl TrackInfo {
    pub fn new(
        id: TrackId,
        time_base: TimeBase,
        start_ticks: Option<i64>,
        duration_ticks: Option<u64>,
        format: TrackFormat,
    ) -> Self {
        Self {
            id,
            time_base,
            start_ticks,
            duration_ticks,
            format,
        }
    }

    pub const fn id(&self) -> TrackId {
        self.id
    }

    pub const fn time_base(&self) -> TimeBase {
        self.time_base
    }

    pub const fn start_time(&self) -> Option<Timestamp> {
        match self.start_ticks {
            Some(ticks) => Some(Timestamp::new(ticks, self.time_base)),
            None => None,
        }
    }

    pub const fn duration(&self) -> Option<TimeSpan> {
        match self.duration_ticks {
            Some(ticks) => Some(TimeSpan::new(ticks, self.time_base)),
            None => None,
        }
    }

    pub fn format(&self) -> &TrackFormat {
        &self.format
    }
}
