use std::{fs::File, sync::Arc};

use crate::{
    backends::{pcm::PcmDecoder, wav::WavDemuxer},
    error::PlaybackError,
    media::{MediaTime, Rounding, TrackFormat},
    pipeline::{self, AudioPipeline, DeferredAudioOutput, PlaybackPipeline, PlaybackPipelineError},
    ports::{AudioOutputFactory, DemuxError, Demuxer},
    runtime::{CancellationToken, PlaybackPipelineFactory},
    source::{BoundedReader, FileSource},
};

pub(crate) struct WavPlaybackFactory<F> {
    source: FileSource,
    output_factory: Arc<F>,
}

impl<F> WavPlaybackFactory<F>
where
    F: AudioOutputFactory,
{
    pub(crate) fn new(source: FileSource, output_factory: F) -> Self {
        Self {
            source,
            output_factory: Arc::new(output_factory),
        }
    }

    fn create_audio_components(
        &self,
        track: &crate::media::TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(PcmDecoder, DeferredAudioOutput<Arc<F>>), PlaybackError> {
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
}

impl<F> PlaybackPipelineFactory for WavPlaybackFactory<F>
where
    F: AudioOutputFactory,
{
    type Demux = WavDemuxer<BoundedReader<File>>;

    fn build(
        &self,
        epoch: crate::pipeline::PlaybackEpoch,
        cancel: &CancellationToken,
    ) -> Result<crate::pipeline::PlaybackPipeline<Self::Demux>, PlaybackError> {
        Self::check_canceled(cancel)?;
        let demuxer = self.open_demuxer(cancel)?;

        Self::check_canceled(cancel)?;
        let track = demuxer
            .tracks()
            .iter()
            .find(|track| matches!(track.format(), TrackFormat::Audio(_)))
            .cloned()
            .ok_or(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack))?;

        let presentation_boundary = match track.start_time() {
            Some(timestamp) => timestamp
                .to_media_time(Rounding::TowardZero)
                .map_err(|error| PlaybackError::Pipeline(PlaybackPipelineError::Time(error)))?,
            None => MediaTime::from_nanoseconds(0),
        };

        let (decoder, output) = self.create_audio_components(&track, cancel)?;
        let audio = AudioPipeline::new(
            decoder,
            output,
            track.id(),
            epoch,
            self.packet_capacity(),
            presentation_boundary,
        )
        .map_err(|error| PlaybackError::Pipeline(PlaybackPipelineError::Audio(error)))?;
        Self::check_canceled(cancel)?;
        let playback =
            PlaybackPipeline::new(demuxer, audio, epoch).map_err(PlaybackError::Pipeline)?;
        Ok(playback)
    }

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        Self::check_canceled(cancel)?;

        let reader = self
            .source
            .open_reader()
            .map_err(|_| PlaybackError::Demux(DemuxError::Io))?;

        Self::check_canceled(cancel)?;

        WavDemuxer::from_reader(reader).map_err(PlaybackError::Demux)
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
