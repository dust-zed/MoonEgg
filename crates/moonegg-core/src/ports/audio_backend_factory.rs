use crate::{
    media::{AudioBuffer, TrackInfo},
    ports::{DecodeError, Decoder, DemuxError, Demuxer},
};

pub trait AudioBackendFactory: Sync + Send + 'static {
    type Demux: Demuxer + 'static;
    type Decode: Decoder<Output = AudioBuffer> + 'static;

    fn open_demuxer(&self) -> Result<Self::Demux, DemuxError>;

    fn create_audio_decoder(&self, track: &TrackInfo) -> Result<Self::Decode, DecodeError>;
}
