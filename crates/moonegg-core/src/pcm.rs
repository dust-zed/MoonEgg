use crate::{
    media::{
        AudioBuffer, AudioCodecId, AudioSamples, AudioTrackFormat, DecodedFrame, Packet, Rounding,
        TrackId,
    },
    ports::{DecodeError, DecodeInput, Decoder, ReceiveResult, SubmitResult},
};

pub(crate) fn decode_pcm_packet(
    packet: Packet,
    format: &AudioTrackFormat,
) -> Result<DecodedFrame<AudioBuffer>, DecodeError> {
    // 1. 检查是否属于当前支持的格式
    if format.codec() != AudioCodecId::PcmS16Le {
        return Err(DecodeError::Unsupported);
    }

    let channels = format.channel_count();
    let sample_rate = format.sample_rate();

    if channels == 0 || sample_rate == 0 {
        return Err(DecodeError::InvalidData);
    }

    if !matches!(channels, 1 | 2) {
        return Err(DecodeError::Unsupported);
    }

    let bytes = packet.data();
    let bytes_per_frame = usize::from(channels) * 2;

    if bytes.len() % bytes_per_frame != 0 {
        return Err(DecodeError::InvalidData);
    }

    let pts = packet
        .pts()
        .ok_or(DecodeError::InvalidData)?
        .to_media_time(Rounding::TowardZero)
        .map_err(|_| DecodeError::InvalidData)?;
    let mut samples = Vec::with_capacity(bytes.len() / 2);

    for pair in bytes.chunks_exact(2) {
        let sample = i16::from_le_bytes([pair[0], pair[1]]);
        samples.push(sample);
    }

    let buffer = AudioBuffer::new(sample_rate, channels, AudioSamples::I16(samples))
        .map_err(|_| DecodeError::InvalidData)?;

    Ok(DecodedFrame::new(packet.track_id(), pts, buffer))
}

pub(crate) struct PcmDecoder {
    track_id: TrackId,
    format: AudioTrackFormat,

    pending_output: Option<DecodedFrame<AudioBuffer>>,
    input_ended: bool,
}

impl PcmDecoder {
    pub(crate) fn new(track_id: TrackId, format: AudioTrackFormat) -> Result<Self, DecodeError> {
        if format.codec() != AudioCodecId::PcmS16Le {
            return Err(DecodeError::Unsupported);
        }

        if format.sample_rate() == 0 || format.channel_count() == 0 {
            return Err(DecodeError::InvalidData);
        }

        if !matches!(format.channel_count(), 1 | 2) {
            return Err(DecodeError::Unsupported);
        }

        Ok(Self {
            track_id,
            format,
            pending_output: None,
            input_ended: false,
        })
    }
}

impl Decoder for PcmDecoder {
    type Output = AudioBuffer;

    fn submit(&mut self, input: DecodeInput) -> Result<SubmitResult, DecodeError> {
        if self.input_ended {
            return Err(DecodeError::InvalidState);
        }

        match input {
            DecodeInput::Packet(packet) => {
                if packet.track_id() != self.track_id {
                    return Err(DecodeError::InvalidData);
                }

                if self.pending_output.is_some() {
                    return Ok(SubmitResult::Backpressure(DecodeInput::Packet(packet)));
                }

                let frame = decode_pcm_packet(packet, &self.format)?;

                self.pending_output = Some(frame);

                Ok(SubmitResult::Accepted)
            }

            DecodeInput::EndOfStream => {
                self.input_ended = true;
                Ok(SubmitResult::Accepted)
            }
        }
    }

    fn receive(&mut self) -> Result<ReceiveResult<Self::Output>, DecodeError> {
        if let Some(frame) = self.pending_output.take() {
            return Ok(ReceiveResult::Frame(frame));
        }

        if self.input_ended {
            Ok(ReceiveResult::EndOfStream)
        } else {
            Ok(ReceiveResult::NotReady)
        }
    }

    fn flush(&mut self) -> Result<(), DecodeError> {
        self.pending_output = None;
        self.input_ended = false;
        Ok(())
    }
}
