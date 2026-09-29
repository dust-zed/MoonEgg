use std::sync::Arc;

use crate::{
    error::{PlaybackError, RuntimeError},
    media::{AudioBuffer, TrackFormat, TrackInfo},
    pipeline::{DeferredAudioOutput, PlaybackEpoch, PlaybackPipeline, PlaybackPipelineError},
    ports::{AudioBackendFactory, AudioOutput, AudioOutputFactory, Decoder, Demuxer},
    runtime::worker::CancellationToken,
};

pub(crate) trait AudioPipelineFactory: Send + Sync + 'static {
    type Demux: Demuxer + 'static;
    type Decode: Decoder<Output = AudioBuffer> + 'static;
    type Output: AudioOutput + 'static;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError>;

    fn create_audio_components(
        &self,
        track: &TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError>;

    fn packet_capacity(&self) -> usize;

    fn build(
        &self,
        epoch: PlaybackEpoch,
        cancel: &CancellationToken,
    ) -> Result<PlaybackPipeline<Self::Demux, Self::Decode, Self::Output>, PlaybackError> {
        Self::check_canceled(cancel)?;
        let demuxer = self.open_demuxer(cancel)?;

        Self::check_canceled(cancel)?;

        let track = demuxer
            .tracks()
            .iter()
            .find(|track| matches!(track.format(), TrackFormat::Audio(_)))
            .cloned()
            .ok_or(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack))?;

        let (decoder, output) = self.create_audio_components(&track, cancel)?;

        Self::check_canceled(cancel)?;

        PlaybackPipeline::new(
            demuxer,
            decoder,
            output,
            track.id(),
            epoch,
            self.packet_capacity(),
        )
        .map_err(PlaybackError::Pipeline)
    }

    fn check_canceled(cancel: &CancellationToken) -> Result<(), PlaybackError> {
        if cancel.is_canceled() {
            Err(PlaybackError::Runtime(RuntimeError::Cancelled))
        } else {
            Ok(())
        }
    }
}

pub(crate) struct BackendPlaybackFactory<B, F> {
    backend: B,
    output_factory: Arc<F>,
}

impl<B, F> BackendPlaybackFactory<B, F> {
    pub(crate) fn new(backend: B, output_factory: F) -> Self {
        Self {
            backend,
            output_factory: Arc::new(output_factory),
        }
    }
}

impl<B, F> AudioPipelineFactory for BackendPlaybackFactory<B, F>
where
    B: AudioBackendFactory,
    F: AudioOutputFactory,
{
    type Decode = B::Decode;
    type Demux = B::Demux;
    type Output = DeferredAudioOutput<Arc<F>>;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        Self::check_canceled(cancel)?;

        let demuxer = self.backend.open_demuxer().map_err(PlaybackError::Demux)?;
        Self::check_canceled(cancel)?;
        Ok(demuxer)
    }

    fn create_audio_components(
        &self,
        track: &TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError> {
        Self::check_canceled(cancel)?;
        let decoder = self
            .backend
            .create_audio_decoder(track)
            .map_err(PlaybackError::Decode)?;
        Self::check_canceled(cancel)?;

        let deferred_output = DeferredAudioOutput::new(Arc::clone(&self.output_factory));
        Ok((decoder, deferred_output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
