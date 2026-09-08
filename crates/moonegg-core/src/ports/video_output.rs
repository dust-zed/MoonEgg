use crate::media::DecodedFrame;

#[derive(Debug)]
pub enum VideoSubmitResult<T> {
    Accepted,
    Backpressure(DecodedFrame<T>),
}

#[derive(Debug)]
pub enum VideoOutputError {
    InvalidFormat,
    InvalidState,
    SurfaceUnavailable,
    Platform,
}

pub trait VideoOutput {
    type FramePayload;

    fn present(
        &mut self,
        frame: DecodedFrame<Self::FramePayload>,
    ) -> Result<VideoSubmitResult<Self::FramePayload>, VideoOutputError>;

    fn discard(&mut self, frame: DecodedFrame<Self::FramePayload>) -> Result<(), VideoOutputError>;

    fn flush(&mut self) -> Result<(), VideoOutputError>;
}
