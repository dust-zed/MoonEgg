use crate::{
    media::TrackInfo,
    ports::{AudioBackendFactory, DecodeError, Decoder, VideoOutput, VideoOutputError},
};

#[derive(Debug)]
pub enum VideoBackendError {
    Decode(DecodeError),
    Output(VideoOutputError),
}

pub trait VideoBackendFactory: AudioBackendFactory {
    type VideoPayload: 'static;

    type VideoDecode: Decoder<Output = Self::VideoPayload> + 'static;

    type VideoOutput: VideoOutput<FramePayload = Self::VideoPayload> + 'static;

    fn create_video_components(
        &self,
        track: &TrackInfo,
    ) -> Result<(Self::VideoDecode, Self::VideoOutput), VideoBackendError>;
}
