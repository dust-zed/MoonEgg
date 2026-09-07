//! 解析媒体容器，发现轨道，并按容器中的读取顺序输出压缩 Packet

use crate::media::{MediaTime, Packet, TrackInfo};

#[derive(Debug)]
pub enum ReadPacketResult {
    Packet(Packet),
    NotReady,
    EndOfStream,
}

#[derive(Debug)]
pub enum DemuxError {
    InvalidData,
    Unsupported,
    NotSeekable,
    Io,
}

pub trait Demuxer {
    fn tracks(&self) -> &[TrackInfo];

    fn read_packet(&mut self) -> Result<ReadPacketResult, DemuxError>;

    fn seek(&mut self, target: MediaTime) -> Result<MediaTime, DemuxError>;
}
