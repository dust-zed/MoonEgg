//! 解码后的媒体帧及其呈现时间

use crate::media::{MediaTime, TrackId};

#[derive(Debug)]
pub struct DecodedFrame<T> {
    track_id: TrackId,
    pts: MediaTime,
    payload: T,
}

impl<T> DecodedFrame<T> {
    pub const fn new(track_id: TrackId, pts: MediaTime, payload: T) -> Self {
        Self {
            track_id,
            pts,
            payload,
        }
    }

    pub const fn track_id(&self) -> TrackId {
        self.track_id
    }

    pub const fn pts(&self) -> MediaTime {
        self.pts
    }

    pub const fn payload(&self) -> &T {
        &self.payload
    }

    pub fn into_payload(self) -> T {
        self.payload
    }

    pub fn into_parts(self) -> (TrackId, MediaTime, T) {
        (self.track_id, self.pts, self.payload)
    }
}
