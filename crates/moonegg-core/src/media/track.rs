//! 媒体轨道标识和轨道描述

use crate::media::TimeBase;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpan {
    ticks: u64,
    time_base: TimeBase,
}

impl TimeSpan {
    pub const fn new(ticks: u64, time_base: TimeBase) -> TimeSpan {
        Self { ticks, time_base }
    }
}
