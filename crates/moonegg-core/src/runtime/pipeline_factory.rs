use crate::{
    error::{PlaybackError, RuntimeError},
    media::{AudioBuffer, TrackFormat, TrackInfo},
    pipeline::{PlaybackEpoch, PlaybackPipeline, PlaybackPipelineError},
    ports::{AudioOutput, Decoder, Demuxer},
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
