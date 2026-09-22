use crate::{
    media::AudioTrackFormat,
    ports::{AudioOutput, AudioOutputError},
};

pub trait AudioOutputFactory: Send + Sync + 'static {
    type Output: AudioOutput + 'static;

    fn create(&self, format: &AudioTrackFormat) -> Result<Self::Output, AudioOutputError>;
}
