//! Demuxer 输出的压缩媒体数据。

use crate::media::{TimeSpan, Timestamp, TrackId};

#[derive(Debug)]
pub struct Packet {
    track_id: TrackId,
    data: Vec<u8>,
    pts: Option<Timestamp>,
    dts: Option<Timestamp>,
    duration: Option<TimeSpan>,
    is_keyframe: bool,
}

impl Packet {
    pub fn new(
        track_id: TrackId,
        data: Vec<u8>,
        pts: Option<Timestamp>,
        dts: Option<Timestamp>,
        duration: Option<TimeSpan>,
        is_keyframe: bool,
    ) -> Self {
        Self {
            track_id,
            data,
            pts,
            dts,
            duration,
            is_keyframe,
        }
    }

    pub const fn track_id(&self) -> TrackId {
        self.track_id
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub const fn pts(&self) -> Option<Timestamp> {
        self.pts
    }

    pub const fn dts(&self) -> Option<Timestamp> {
        self.dts
    }

    pub const fn duration(&self) -> Option<TimeSpan> {
        self.duration
    }
}
