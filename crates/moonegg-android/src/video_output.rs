use moonegg_core::{
    media::DecodedFrame,
    ports::{VideoOutput, VideoOutputError, VideoSubmitResult},
};

use crate::{
    media_codec::MediaCodecError, video_buffer::AndroidVideoBuffer,
    video_codec_session::VideoCodecSessionError,
};

pub(crate) struct AndroidVideoOutput;

impl VideoOutput for AndroidVideoOutput {
    type FramePayload = AndroidVideoBuffer;

    fn discard(&mut self, frame: DecodedFrame<Self::FramePayload>) -> Result<(), VideoOutputError> {
        let video_buffer = frame.into_payload();
        match video_buffer.discard() {
            Err(error) => Err(map_session_error(error)),
            Ok(_) => Ok(()),
        }
    }

    fn present(
        &mut self,
        frame: DecodedFrame<Self::FramePayload>,
    ) -> Result<VideoSubmitResult<Self::FramePayload>, VideoOutputError> {
        let video_buffer = frame.into_payload();
        match video_buffer.present() {
            Err(error) => Err(map_session_error(error)),
            Ok(()) => Ok(VideoSubmitResult::Accepted),
        }
    }

    fn flush(&mut self) -> Result<(), VideoOutputError> {
        Ok(())
    }
}

fn map_session_error(source: VideoCodecSessionError) -> VideoOutputError {
    match source {
        VideoCodecSessionError::BorrowConflict => VideoOutputError::InvalidState,
        VideoCodecSessionError::Codec { source }
            if matches!(source, MediaCodecError::InvalidState) =>
        {
            VideoOutputError::InvalidState
        }
        _ => VideoOutputError::Platform,
    }
}
