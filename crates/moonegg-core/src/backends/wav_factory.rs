use std::{fs::File, sync::Arc};

use crate::{
    backends::{pcm::PcmDecoder, wav::WavDemuxer},
    error::PlaybackError,
    media::TrackFormat,
    pipeline::{DeferredAudioOutput, PlaybackPipelineError},
    ports::{AudioOutputFactory, DemuxError},
    runtime::{CancellationToken, PlaybackPipelineFactory},
    source::{BoundedReader, FileSource},
};

pub(crate) struct WavPlaybackFactory<F> {
    source: FileSource,
    output_factory: Arc<F>,
}

impl<F> WavPlaybackFactory<F> {
    pub(crate) fn new(source: FileSource, output_factory: F) -> Self {
        Self {
            source,
            output_factory: Arc::new(output_factory),
        }
    }
}

impl<F> PlaybackPipelineFactory for WavPlaybackFactory<F>
where
    F: AudioOutputFactory,
{
    type Demux = WavDemuxer<BoundedReader<File>>;
    type Decode = PcmDecoder;
    type Output = DeferredAudioOutput<Arc<F>>;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        Self::check_canceled(cancel)?;

        let reader = self
            .source
            .open_reader()
            .map_err(|_| PlaybackError::Demux(DemuxError::Io))?;

        Self::check_canceled(cancel)?;

        WavDemuxer::from_reader(reader).map_err(PlaybackError::Demux)
    }

    fn create_audio_components(
        &self,
        track: &crate::media::TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError> {
        Self::check_canceled(cancel)?;

        let TrackFormat::Audio(track_format) = track.format() else {
            return Err(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack));
        };

        let decoder =
            PcmDecoder::new(track.id(), track_format.clone()).map_err(PlaybackError::Decode)?;
        Self::check_canceled(cancel)?;

        let output = {
            let factory_clone = Arc::clone(&self.output_factory);
            DeferredAudioOutput::new(factory_clone)
        };
        Self::check_canceled(cancel)?;

        Ok((decoder, output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
