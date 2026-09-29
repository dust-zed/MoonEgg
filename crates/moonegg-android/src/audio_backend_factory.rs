use std::fs::File;

use moonegg_core::{
    FileSource,
    media::{TrackFormat, TrackInfo},
    ports::{AudioBackendFactory, DecodeError, DemuxError},
};

use crate::{
    android_decoder::AndroidAudioDecoder,
    android_demuxer::{AndroidDemuxer, AndroidDemuxerError},
};

pub(crate) struct AndroidAudioBackendFactory {
    source: FileSource,
}

impl AndroidAudioBackendFactory {
    pub(crate) fn new(source: FileSource) -> Self {
        Self { source }
    }
}

impl AudioBackendFactory for AndroidAudioBackendFactory {
    type Decode = AndroidAudioDecoder;
    type Demux = AndroidDemuxer;

    fn create_audio_decoder(&self, track: &TrackInfo) -> Result<Self::Decode, DecodeError> {
        let TrackFormat::Audio(format) = track.format() else {
            return Err(DecodeError::Unsupported);
        };
        AndroidAudioDecoder::new(track.id(), format).map_err(|error| error.into_decode_error())
    }

    fn open_demuxer(&self) -> Result<Self::Demux, DemuxError> {
        let (file, start, length) = match &self.source {
            FileSource::Path(path) => {
                let file = File::open(path).map_err(|_| DemuxError::Io)?;
                let length = file.metadata().map_err(|_| DemuxError::Io)?.len();
                (file, 0, length)
            }
            FileSource::Region {
                file,
                start,
                length,
            } => {
                let file_clone = file.try_clone().map_err(|_| DemuxError::Io)?;
                (file_clone, *start, *length)
            }
        };
        let demuxer = AndroidDemuxer::from_file(file, start, length)
            .map_err(|error| error.into_demux_error())?;
        Ok(demuxer)
    }
}
