use crate::{
    media::{
        AudioBuffer, AudioBufferError, AudioPcmFormat, DecodedFrame, MediaTime, Packet, TimeError,
        TrackId,
    },
    pipeline::{BoundedQueue, EpochItem, PlaybackEpoch, QueueError, QueuePushResult},
    ports::{
        AudioOutput, AudioOutputError, AudioPlaybackPosition, AudioSubmitResult, DecodeError,
        DecodeInput, Decoder, ReceiveResult, SubmitResult,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioPhase {
    Feeding,
    InputEnded,
    Draining,
    DecoderDrained,
}

#[derive(Debug)]
pub enum AudioEnqueueResult {
    Accepted,
    Backpressure(EpochItem<Packet>),
    EpochMismatch(EpochItem<Packet>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioStepResult {
    Progress,
    Blocked,
    DecoderDrained,
}

#[derive(Debug)]
pub enum AudioPipelineError {
    Queue(QueueError),
    Decode(DecodeError),
    Output(AudioOutputError),
    Time(TimeError),
    Buffer(AudioBufferError),
    WrongTrack,
    InputClosed,
    UnexpectedDecoderEos,
}

pub struct AudioPipeline<D, O> {
    decoder: D,
    output: O,

    track_id: TrackId,
    epoch: PlaybackEpoch,

    // 缓冲不应该是各自的 decoder， demuxer，output自己维护？
    packets: BoundedQueue<Packet>,
    pending_input: Option<DecodeInput>,
    pending_frame: Option<DecodedFrame<AudioBuffer>>,

    phase: AudioPhase,
    presentation_boundary: MediaTime,
}

impl<D, O> AudioPipeline<D, O>
where
    D: Decoder<Output = AudioBuffer>,
    O: AudioOutput,
{
    pub fn new(
        decoder: D,
        output: O,
        track_id: TrackId,
        epoch: PlaybackEpoch,
        packet_capacity: usize,
        presentation_boundary: MediaTime,
    ) -> Result<Self, AudioPipelineError> {
        let packets = BoundedQueue::new(packet_capacity).map_err(AudioPipelineError::Queue)?;

        Ok(Self {
            decoder,
            output,
            track_id,
            epoch,
            packets,
            pending_input: None,
            pending_frame: None,
            phase: AudioPhase::Feeding,
            presentation_boundary,
        })
    }

    pub fn try_push_packet(
        &mut self,
        item: EpochItem<Packet>,
    ) -> Result<AudioEnqueueResult, AudioPipelineError> {
        if item.epoch() != self.epoch {
            return Ok(AudioEnqueueResult::EpochMismatch(item));
        }

        if item.value().track_id() != self.track_id {
            return Err(AudioPipelineError::WrongTrack);
        }

        if self.phase != AudioPhase::Feeding {
            return Err(AudioPipelineError::InputClosed);
        }

        let (epoch, packet) = item.into_parts();
        match self.packets.push(packet) {
            QueuePushResult::Accepted => Ok(AudioEnqueueResult::Accepted),
            QueuePushResult::Full(packet) => Ok(AudioEnqueueResult::Backpressure(EpochItem::new(
                epoch, packet,
            ))),
        }
    }

    pub fn end_input(&mut self, epoch: PlaybackEpoch) -> bool {
        if epoch != self.epoch {
            return false;
        }

        if self.phase == AudioPhase::Feeding {
            self.phase = AudioPhase::InputEnded
        }
        true
    }

    pub fn submit_one_input(&mut self) -> Result<bool, AudioPipelineError> {
        if matches!(
            self.phase,
            AudioPhase::Draining | AudioPhase::DecoderDrained
        ) {
            return Ok(false);
        }

        let mut progressed = false;

        if self.pending_input.is_none() {
            self.pending_input = match self.packets.pop() {
                Some(packet) => Some(DecodeInput::Packet(packet)),
                None if self.phase == AudioPhase::InputEnded => Some(DecodeInput::EndOfStream),
                None => None,
            };

            progressed = self.pending_input.is_some();
        }

        let Some(input) = self.pending_input.take() else {
            return Ok(false);
        };

        let is_eos = matches!(&input, DecodeInput::EndOfStream);

        match self
            .decoder
            .submit(input)
            .map_err(AudioPipelineError::Decode)?
        {
            SubmitResult::Accepted => {
                if is_eos {
                    self.phase = AudioPhase::Draining;
                }
                Ok(true)
            }
            SubmitResult::Backpressure(input) => {
                self.pending_input = Some(input);
                Ok(progressed)
            }
        }
    }

    fn submit_pending_frame(&mut self) -> Result<bool, AudioPipelineError> {
        let Some(frame) = self.pending_frame.take() else {
            return Ok(false);
        };

        match self
            .output
            .submit(frame)
            .map_err(AudioPipelineError::Output)?
        {
            AudioSubmitResult::Accepted => Ok(true),

            AudioSubmitResult::Backpressure(frame) => {
                self.pending_frame = Some(frame);
                Ok(false)
            }
        }
    }

    pub fn step(&mut self) -> Result<AudioStepResult, AudioPipelineError> {
        let mut progressed = false;

        // 1. 优先提交上一轮留下的 PCM
        if self.pending_frame.is_some() {
            if !self.submit_pending_frame()? {
                return Ok(AudioStepResult::Blocked);
            }
            progressed = true;
        }

        // 2. 每轮最多接收一个 decoder 输出。
        if self.phase != AudioPhase::DecoderDrained {
            match self.decoder.receive().map_err(AudioPipelineError::Decode)? {
                ReceiveResult::Frame(frame) => {
                    if frame.track_id() != self.track_id {
                        return Err(AudioPipelineError::WrongTrack);
                    }
                    self.pending_frame =
                        Self::trim_audio_before(frame, self.presentation_boundary)?;
                    progressed = true;
                }
                ReceiveResult::NotReady => {}

                ReceiveResult::EndOfStream => {
                    if self.phase != AudioPhase::Draining {
                        return Err(AudioPipelineError::UnexpectedDecoderEos);
                    }
                    self.phase = AudioPhase::DecoderDrained;
                    progressed = true;
                }
            }
        }
        if self.pending_frame.is_some() {
            if !self.submit_pending_frame()? {
                return Ok(if progressed {
                    AudioStepResult::Progress
                } else {
                    AudioStepResult::Blocked
                });
            }
            progressed = true;
        }

        if self.phase == AudioPhase::DecoderDrained {
            return Ok(AudioStepResult::DecoderDrained);
        }

        progressed |= self.submit_one_input()?;

        Ok(if progressed {
            AudioStepResult::Progress
        } else {
            AudioStepResult::Blocked
        })
    }

    pub fn start(&mut self) -> Result<(), AudioPipelineError> {
        self.output.start().map_err(AudioPipelineError::Output)
    }

    pub fn pause(&mut self) -> Result<(), AudioPipelineError> {
        self.output.pause().map_err(AudioPipelineError::Output)
    }

    pub fn reset(&mut self, new_epoch: PlaybackEpoch) -> Result<(), AudioPipelineError> {
        self.output.pause().map_err(AudioPipelineError::Output)?;

        self.pending_frame = None;
        self.pending_input = None;
        self.packets.drain().for_each(drop);

        self.decoder.flush().map_err(AudioPipelineError::Decode)?;

        self.output.flush().map_err(AudioPipelineError::Output)?;

        self.epoch = new_epoch;
        self.phase = AudioPhase::Feeding;

        Ok(())
    }

    pub const fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    pub fn playback_position(&mut self) -> Result<AudioPlaybackPosition, AudioPipelineError> {
        self.output
            .playback_position()
            .map_err(AudioPipelineError::Output)
    }

    pub fn is_finished(&mut self) -> Result<bool, AudioPipelineError> {
        if self.phase != AudioPhase::DecoderDrained {
            return Ok(false);
        }

        self.output.is_drained().map_err(AudioPipelineError::Output)
    }

    pub fn output_format(&self) -> Option<AudioPcmFormat> {
        self.output.format()
    }

    pub(super) fn set_presentation_boundary(&mut self, boundary: MediaTime) {
        self.presentation_boundary = boundary
    }

    fn trim_audio_before(
        frame: DecodedFrame<AudioBuffer>,
        boundary: MediaTime,
    ) -> Result<Option<DecodedFrame<AudioBuffer>>, AudioPipelineError> {
        const NANOS_PER_SECOND: u128 = 1_000_000_000;

        let buffer = frame.payload();
        let avaiable_frames = buffer.frame_count();

        if avaiable_frames == 0 {
            return Ok(None);
        }

        if frame.pts() >= boundary {
            return Ok(Some(frame));
        }

        let pts_ns = i128::from(frame.pts().nanoseconds());
        let boundary_pts = i128::from(boundary.nanoseconds());
        let delta_ns = (boundary_pts - pts_ns) as u128;

        let sample_rate = u128::from(buffer.sample_rate());

        let frames_to_drop = (delta_ns * sample_rate).div_ceil(NANOS_PER_SECOND);

        if frames_to_drop >= avaiable_frames as u128 {
            return Ok(None);
        }

        let advance_ns = frames_to_drop * NANOS_PER_SECOND / sample_rate;
        let advance_ns = i128::try_from(advance_ns)
            .map_err(|_| AudioPipelineError::Time(TimeError::Overflow))?;

        let new_pts_ns = i64::try_from(pts_ns + advance_ns)
            .map_err(|_| AudioPipelineError::Time(TimeError::Overflow))?;
        let frames_to_drop = frames_to_drop as usize;
        let (track_id, _, mut buffer) = frame.into_parts();
        buffer
            .discard_prefix_frames(frames_to_drop)
            .map_err(AudioPipelineError::Buffer)?;
        Ok(Some(DecodedFrame::new(
            track_id,
            MediaTime::from_nanoseconds(new_pts_ns),
            buffer,
        )))
    }
}
