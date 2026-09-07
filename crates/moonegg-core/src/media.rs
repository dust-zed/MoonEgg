//! 负责
//! 时间戳、time base、轨道、 packet/frame 元数据、媒体格式
mod format;
mod frame;
mod packet;
mod time;
mod track;

pub use format::{AudioTrackFormat, CodecId, TrackFormat, VideoTrackFormat};
pub use packet::Packet;
pub use time::{MediaDelta, MediaTime, Rounding, TimeBase, TimeError, TimeSpan, Timestamp};
pub use track::{TrackId, TrackInfo};
