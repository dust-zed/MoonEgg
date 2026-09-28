use std::fs::File;

use crate::{
    backends::{pcm::PcmDecoder, wav::WavDemuxer},
    error::PlaybackError,
    media::{AudioPcmFormat, AudioSampleFormat, TrackFormat},
    pipeline::PlaybackPipelineError,
    ports::{AudioOutputFactory, DecodeError, DemuxError},
    runtime::{AudioPipelineFactory, CancellationToken},
    source::{BoundedReader, FileSource},
};

pub(crate) struct WavPlaybackFactory<F> {
    source: FileSource,
    output_factory: F,
}

impl<F> WavPlaybackFactory<F> {
    pub(crate) fn new(source: FileSource, output_factory: F) -> Self {
        Self {
            source,
            output_factory,
        }
    }
}

impl<F> AudioPipelineFactory for WavPlaybackFactory<F>
where
    F: AudioOutputFactory,
{
    type Demux = WavDemuxer<BoundedReader<File>>;
    type Decode = PcmDecoder;
    type Output = F::Output;

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

        // PcmDecoder 构造成功，已确认轨道是当前支持的 PCM S16LE。
        // 该解码路径输出 I16，并保留轨道采样率和声道数。
        let pcm_format = AudioPcmFormat::new(
            track_format.sample_rate(),
            track_format.channel_count(),
            AudioSampleFormat::I16,
        )
        .map_err(|_| PlaybackError::Decode(DecodeError::InvalidData))?;

        let output = self
            .output_factory
            .create(&pcm_format)
            .map_err(PlaybackError::AudioOutput)?;
        Self::check_canceled(cancel)?;

        Ok((decoder, output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}
