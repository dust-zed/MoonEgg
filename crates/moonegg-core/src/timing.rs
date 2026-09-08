//! 负责
//! 主时钟、暂停补偿、音画同步与丢帧决策
mod av_sync;
mod clock;

pub use av_sync::{AvSync, AvSyncError, VideoSyncDecision};
pub use clock::{AudioClock, ClockError, ClockSnapshot};
