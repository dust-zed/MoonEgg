use crate::media::{DecodedFrame, Packet};

#[derive(Debug)]
pub enum DecodeInput {
    Packet(Packet),
    EndOfStream,
}

#[derive(Debug)]
pub enum SubmitResult {
    Accepted,
    Backpressure(DecodeInput),
}

#[derive(Debug)]
pub enum ReceiveResult<T> {
    Frame(DecodedFrame<T>),
    NeedInput,
    EndOfStream,
}

#[derive(Debug)]
pub enum DecodeError {
    InvalidData,
    InvalidState,
    Unsupported,
    Platform,
}

pub trait Decoder {
    type Output;

    fn submit(&mut self, input: DecodeInput) -> Result<SubmitResult, DecodeError>;

    fn receive(&mut self) -> Result<ReceiveResult<Self::Output>, DecodeError>;

    fn flush(&mut self) -> Result<(), DecodeError>;
}
