use std::path::PathBuf;

use crate::{
    backends::{pcm::PcmDecoder, simulated_audio::SimulatedAudioOutput, wav::WavDemuxer},
    error::PlaybackError,
    media::TrackFormat,
    pipeline::PlaybackPipelineError,
    ports::{AudioOutputFactory, DemuxError},
    runtime::{AudioPipelineFactory, CancellationToken},
};

pub(crate) struct WavPlaybackFactory<F> {
    path: PathBuf,
    output_factory: F,
}

impl<F> WavPlaybackFactory<F> {
    pub(crate) fn new(path: PathBuf, output_factory: F) -> Self {
        Self {
            path,
            output_factory,
        }
    }
}

impl<F> AudioPipelineFactory for WavPlaybackFactory<F>
where
    F: AudioOutputFactory,
{
    type Demux = WavDemuxer;
    type Decode = PcmDecoder;
    type Output = F::Output;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        Self::check_canceled(cancel)?;

        let bytes = std::fs::read(&self.path).map_err(|_| PlaybackError::Demux(DemuxError::Io))?;

        Self::check_canceled(cancel)?;

        WavDemuxer::from_bytes(bytes).map_err(PlaybackError::Demux)
    }

    fn create_audio_components(
        &self,
        track: &crate::media::TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError> {
        Self::check_canceled(cancel)?;

        let TrackFormat::Audio(format) = track.format() else {
            return Err(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack));
        };

        let decoder = PcmDecoder::new(track.id(), format.clone()).map_err(PlaybackError::Decode)?;
        Self::check_canceled(cancel)?;

        let output = self
            .output_factory
            .create(format)
            .map_err(PlaybackError::AudioOutput)?;
        Self::check_canceled(cancel)?;

        Ok((decoder, output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
