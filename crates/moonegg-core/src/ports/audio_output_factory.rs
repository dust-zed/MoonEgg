use crate::{
    media::AudioPcmFormat,
    ports::{AudioOutput, AudioOutputError},
};

pub trait AudioOutputFactory: Send + Sync + 'static {
    type Output: AudioOutput + 'static;

    fn create(&self, format: &AudioPcmFormat) -> Result<Self::Output, AudioOutputError>;
}
