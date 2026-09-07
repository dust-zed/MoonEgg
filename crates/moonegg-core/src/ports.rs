//! 负责
//! demux、decode、audio/video output 的能力接口
mod audio_output;
mod decoder;
mod demuxer;
mod video_output;

pub use decoder::{DecodeError, DecodeInput, Decoder, ReceiveResult, SubmitResult};
pub use demuxer::{DemuxError, Demuxer, ReadPacketResult};
