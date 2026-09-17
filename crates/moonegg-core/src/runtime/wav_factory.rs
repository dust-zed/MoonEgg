use std::path::PathBuf;

use crate::{
    error::PlaybackError,
    media::TrackFormat,
    pcm::PcmDecoder,
    pipeline::PlaybackPipelineError,
    ports::DemuxError,
    runtime::{AudioPipelineFactory, worker::CancellationToken},
    simulated_audio::SimulatedAudioOutput,
    wav::WavDemuxer,
};

pub(crate) struct WavPlaybackFactory {
    path: PathBuf,
}

impl WavPlaybackFactory {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AudioPipelineFactory for WavPlaybackFactory {
    type Demux = WavDemuxer;
    type Decode = PcmDecoder;
    type Output = SimulatedAudioOutput;

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

        let output = SimulatedAudioOutput::new(format.sample_rate(), format.channel_count(), 4096)
            .map_err(PlaybackError::AudioOutput)?;

        Ok((decoder, output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
