use std::time::Instant;

use crate::{
    media::{AudioPcmFormat, DecodedFrame},
    ports::{
        AudioOutput, AudioOutputError, AudioOutputFactory, AudioPlaybackPosition, AudioSubmitResult,
    },
};

struct ConfiguredOutput<O> {
    output: O,
    format: AudioPcmFormat,
}

pub(crate) struct DeferredAudioOutput<F: AudioOutputFactory> {
    factory: F,
    configured: Option<ConfiguredOutput<F::Output>>,
    play_requested: bool,
}

impl<F: AudioOutputFactory> DeferredAudioOutput<F> {
    pub(crate) fn new(factory: F) -> Self {
        Self {
            factory,
            configured: None,
            play_requested: false,
        }
    }

    fn ensure_output(&mut self, format: AudioPcmFormat) -> Result<(), AudioOutputError> {
        if let Some(configured) = self.configured.as_ref() {
            if configured.format != format {
                return Err(AudioOutputError::InvalidFormat);
            }
            return Ok(());
        }
        let mut output = self.factory.create(&format)?;

        if self.play_requested {
            output.start()?;
        }
        self.configured = Some(ConfiguredOutput { output, format });
        Ok(())
    }
}

impl<F: AudioOutputFactory> AudioOutput for DeferredAudioOutput<F> {
    fn start(&mut self) -> Result<(), AudioOutputError> {
        if let Some(configured) = self.configured.as_mut() {
            configured.output.start()?
        }
        self.play_requested = true;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), AudioOutputError> {
        if let Some(configured) = self.configured.as_mut() {
            configured.output.pause()?;
        }
        self.play_requested = false;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), AudioOutputError> {
        if let Some(configured) = self.configured.as_mut() {
            configured.output.pause()?;
            configured.output.flush()?;
        }
        self.play_requested = false;
        Ok(())
    }

    fn is_drained(&mut self) -> Result<bool, AudioOutputError> {
        if let Some(configured) = self.configured.as_mut() {
            return configured.output.is_drained();
        }
        Ok(true)
    }

    fn playback_position(&mut self) -> Result<AudioPlaybackPosition, AudioOutputError> {
        if let Some(configured) = self.configured.as_mut() {
            return configured.output.playback_position();
        }
        Ok(AudioPlaybackPosition::new(0, Instant::now()))
    }

    fn submit(
        &mut self,
        frame: DecodedFrame<crate::media::AudioBuffer>,
    ) -> Result<AudioSubmitResult, AudioOutputError> {
        if frame.payload().samples().is_empty() {
            return Ok(AudioSubmitResult::Accepted);
        }

        let format = frame.payload().format();
        self.ensure_output(format)?;
        if let Some(configured) = self.configured.as_mut() {
            return configured.output.submit(frame);
        }
        Err(AudioOutputError::InvalidState)
    }

    fn format(&self) -> Option<AudioPcmFormat> {
        let configured_output = self.configured.as_ref()?;
        Some(configured_output.format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backends::SimulatedAudioOutputFactory,
        media::{AudioBuffer, AudioSamples, MediaTime, TrackId},
    };

    fn frame(sample_rate: u32, samples: Vec<i16>) -> DecodedFrame<AudioBuffer> {
        DecodedFrame::new(
            TrackId::new(1),
            MediaTime::from_nanoseconds(0),
            AudioBuffer::new(sample_rate, 1, AudioSamples::I16(samples)).unwrap(),
        )
    }

    #[test]
    fn format_is_established_by_nonempty_pcm_and_survives_flush() {
        let mut output = DeferredAudioOutput::new(SimulatedAudioOutputFactory::new(4));
        output.start().unwrap();
        output.pause().unwrap();
        assert_eq!(output.format(), None);

        assert!(matches!(
            output.submit(frame(44_100, vec![])).unwrap(),
            AudioSubmitResult::Accepted
        ));
        assert_eq!(output.format(), None);

        let pcm = frame(48_000, vec![1, 2]);
        let format = pcm.payload().format();
        assert!(matches!(
            output.submit(pcm).unwrap(),
            AudioSubmitResult::Accepted
        ));
        assert_eq!(output.format(), Some(format));
        assert_eq!(
            output.configured.as_ref().unwrap().output.format(),
            Some(format)
        );
        assert!(matches!(
            output.submit(frame(44_100, vec![3])),
            Err(AudioOutputError::InvalidFormat)
        ));

        output.pause().unwrap();
        output.flush().unwrap();
        assert_eq!(output.format(), Some(format));
        assert!(output.is_drained().unwrap());
    }

    #[test]
    fn backpressure_returns_original_pcm_for_retry_after_flush() {
        let mut output = DeferredAudioOutput::new(SimulatedAudioOutputFactory::new(4));
        assert!(matches!(
            output.submit(frame(48_000, vec![1; 4])).unwrap(),
            AudioSubmitResult::Accepted
        ));

        let samples = vec![2, 3];
        let original_ptr = samples.as_ptr();
        let AudioSubmitResult::Backpressure(pcm) = output.submit(frame(48_000, samples)).unwrap()
        else {
            panic!("full output must return the unaccepted frame");
        };
        let AudioSamples::I16(samples) = pcm.payload().samples() else {
            panic!("sample format must be preserved");
        };
        assert_eq!(samples.as_ptr(), original_ptr);
        let format = output.format();

        output.flush().unwrap();
        assert_eq!(output.format(), format);
        assert!(matches!(
            output.submit(pcm).unwrap(),
            AudioSubmitResult::Accepted
        ));
        assert_eq!(output.playback_position().unwrap().played_frames(), 0);
        assert!(!output.is_drained().unwrap());
    }
}
