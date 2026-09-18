use crate::{
    media::{AudioBuffer, DecodedFrame, Packet, TrackId},
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
                    self.pending_frame = Some(frame);
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
}
