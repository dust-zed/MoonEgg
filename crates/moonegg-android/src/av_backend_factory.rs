use moonegg_core::{
    FileSource,
    media::{TrackFormat, TrackInfo},
    ports::{AudioBackendFactory, DecodeError, DemuxError, VideoBackendError, VideoBackendFactory},
};
use ndk::native_window::NativeWindow;

use crate::{
    AndroidAudioOutput,
    android_decoder::AndroidAudioDecoder,
    android_demuxer::{AndroidDemuxer, DemuxTrackSelection},
    android_video_decoder::AndroidVideoDecoder,
    audio_backend_factory::AndroidAudioBackendFactory,
    video_buffer::AndroidVideoBuffer,
    video_output::AndroidVideoOutput,
};

#[derive(Debug)]
pub(crate) struct AndroidAvBackendFactory {
    audio_backend: AndroidAudioBackendFactory,
    output_window: NativeWindow,
}

impl AndroidAvBackendFactory {
    pub(crate) fn new(source: FileSource, output_window: NativeWindow) -> Self {
        let audio_backend = AndroidAudioBackendFactory::new(source);
        Self {
            audio_backend,
            output_window,
        }
    }
}

impl AudioBackendFactory for AndroidAvBackendFactory {
    type Demux = AndroidDemuxer;
    type Decode = AndroidAudioDecoder;

    fn open_demuxer(&self) -> Result<Self::Demux, DemuxError> {
        let selection = DemuxTrackSelection::AudioVideo;
        self.audio_backend.open_demuxer_with_selection(selection)
    }

    fn create_audio_decoder(&self, track: &TrackInfo) -> Result<Self::Decode, DecodeError> {
        self.audio_backend.create_audio_decoder(track)
    }
}

impl VideoBackendFactory for AndroidAvBackendFactory {
    type VideoPayload = AndroidVideoBuffer;
    type VideoDecode = AndroidVideoDecoder;
    type VideoOutput = AndroidVideoOutput;

    fn create_video_components(
        &self,
        track: &TrackInfo,
    ) -> Result<(Self::VideoDecode, Self::VideoOutput), VideoBackendError> {
        let track_format = track.format();
        let TrackFormat::Video(video_format) = track_format else {
            return Err(VideoBackendError::Decode(DecodeError::Unsupported));
        };
        let output_window = self.output_window.clone();
        let decoder = AndroidVideoDecoder::new(track.id(), video_format, output_window)
            .map_err(|error| VideoBackendError::Decode(error.into_decode_error()))?;
        let output = AndroidVideoOutput;

        Ok((decoder, output))
    }
}
