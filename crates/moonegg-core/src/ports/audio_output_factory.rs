use std::sync::Arc;

use crate::{
    media::AudioPcmFormat,
    ports::{AudioOutput, AudioOutputError},
};

pub trait AudioOutputFactory: Send + Sync + 'static {
    type Output: AudioOutput + 'static;

    fn create(&self, format: &AudioPcmFormat) -> Result<Self::Output, AudioOutputError>;
}

impl<F: AudioOutputFactory> AudioOutputFactory for Arc<F> {
    type Output = F::Output;

    fn create(&self, format: &AudioPcmFormat) -> Result<Self::Output, AudioOutputError> {
        let factory = self.as_ref();
        factory.create(format)
    }
}
