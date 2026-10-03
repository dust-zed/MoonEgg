use std::time::Instant;

use crate::{
    media::{DecodedFrame, MediaTime, Packet, TrackId},
    pipeline::{
        BoundedQueue, CoordinateVideoError, EpochItem, PlaybackEpoch, QueueError, QueuePushResult,
    },
    ports::{
        DecodeError, DecodeInput, Decoder, ReceiveResult, SubmitResult, VideoOutput,
        VideoOutputError, VideoSubmitResult,
    },
    timing::{AvSync, ClockSnapshot, VideoSyncDecision},
};

#[derive(Debug)]
pub enum VideoPipelineError {
    Queue(QueueError),
    Decode(DecodeError),
    Output(VideoOutputError),
    Coordinate(CoordinateVideoError),
    WrongTrack,
    InputClosed,
    UnexpectedDecoderEos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPhase {
    Feeding,
    InputEnded,
    Draining,
    DecoderDrained,
}

pub enum VideoEnqueueResult {
    Accepted,
    Backpressure(EpochItem<Packet>),
    EpochMismatch(EpochItem<Packet>),
}

pub enum VideoPipelineStepResult {
    Progress,
    Blocked,
    WaitUntil { deadline: Instant },
    DecoderDrained,
}

pub struct VideoPipeline<D, O>
where
    D: Decoder,
    O: VideoOutput<FramePayload = D::Output>,
{
    decoder: D,
    output: O,

    track_id: TrackId,
    epoch: PlaybackEpoch,

    packets: BoundedQueue<Packet>,
    pending_input: Option<DecodeInput>,
    pending_frame: Option<DecodedFrame<D::Output>>,

    sync: AvSync,

    phase: VideoPhase,
    presentation_boundary: MediaTime,
}

impl<D, O> VideoPipeline<D, O>
where
    D: Decoder,
    O: VideoOutput<FramePayload = D::Output>,
{
    pub fn new(
        decoder: D,
        output: O,
        track_id: TrackId,
        epoch: PlaybackEpoch,
        packet_capacity: usize,
        sync: AvSync,
        presentation_boundary: MediaTime,
    ) -> Result<Self, VideoPipelineError> {
        let packets = BoundedQueue::new(packet_capacity).map_err(VideoPipelineError::Queue)?;
        let video_pipeline = Self {
            decoder,
            output,
            track_id,
            epoch,
            packets,
            pending_input: None,
            pending_frame: None,
            sync,
            phase: VideoPhase::Feeding,
            presentation_boundary,
        };
        Ok(video_pipeline)
    }

    fn submit_one_input(&mut self) -> Result<bool, VideoPipelineError> {
        if matches!(
            self.phase,
            VideoPhase::Draining | VideoPhase::DecoderDrained
        ) {
            return Ok(false);
        }

        let mut progressed = false;

        if self.pending_input.is_none() {
            self.pending_input = match self.packets.pop() {
                Some(packet) => Some(DecodeInput::Packet(packet)),
                None if self.phase == VideoPhase::InputEnded => Some(DecodeInput::EndOfStream),
                None => None,
            };
            progressed = self.pending_input.is_some();
        }

        let Some(packet) = self.pending_input.take() else {
            return Ok(false);
        };

        let is_eos = matches!(packet, DecodeInput::EndOfStream);
        match self
            .decoder
            .submit(packet)
            .map_err(VideoPipelineError::Decode)?
        {
            SubmitResult::Accepted => {
                if is_eos {
                    self.phase = VideoPhase::Draining
                }
                Ok(true)
            }
            SubmitResult::Backpressure(input) => {
                self.pending_input = Some(input);
                Ok(progressed)
            }
        }
    }

    fn receive_one_frame(&mut self) -> Result<bool, VideoPipelineError> {
        if self.phase == VideoPhase::DecoderDrained {
            return Ok(false);
        }

        if self.pending_frame.is_some() {
            return Ok(false);
        }
        let receive_result = self.decoder.receive().map_err(VideoPipelineError::Decode)?;
        match receive_result {
            ReceiveResult::Frame(frame) => {
                if frame.track_id() != self.track_id {
                    return Err(VideoPipelineError::WrongTrack);
                }
                if frame.pts() < self.presentation_boundary {
                    self.output
                        .discard(frame)
                        .map_err(VideoPipelineError::Output)?;
                    return Ok(true);
                }
                self.pending_frame = Some(frame);
                Ok(true)
            }
            ReceiveResult::EndOfStream => {
                if self.phase != VideoPhase::Draining {
                    return Err(VideoPipelineError::UnexpectedDecoderEos);
                }
                self.phase = VideoPhase::DecoderDrained;
                Ok(true)
            }
            ReceiveResult::NotReady => Ok(false),
        }
    }

    fn present_pending_frame(
        &mut self,
        audio_snapshot: ClockSnapshot,
        now: Instant,
    ) -> Result<VideoPipelineStepResult, VideoPipelineError> {
        let Some(frame) = self.pending_frame.take() else {
            return Ok(VideoPipelineStepResult::Blocked);
        };
        match self
            .sync
            .decide(frame.pts(), audio_snapshot, now)
            .map_err(|error| VideoPipelineError::Coordinate(CoordinateVideoError::Sync(error)))?
        {
            VideoSyncDecision::Drop => {
                self.output
                    .discard(frame)
                    .map_err(VideoPipelineError::Output)?;
                Ok(VideoPipelineStepResult::Progress)
            }
            VideoSyncDecision::PresentNow => {
                let result = self
                    .output
                    .present(frame)
                    .map_err(VideoPipelineError::Output)?;
                match result {
                    VideoSubmitResult::Accepted => Ok(VideoPipelineStepResult::Progress),
                    VideoSubmitResult::Backpressure(frame) => {
                        self.pending_frame = Some(frame);
                        Ok(VideoPipelineStepResult::Blocked)
                    }
                }
            }
            VideoSyncDecision::WaitUntil(deadline) => {
                self.pending_frame = Some(frame);
                Ok(VideoPipelineStepResult::WaitUntil { deadline })
            }
        }
    }
}

impl<D, O> VideoBranch for VideoPipeline<D, O>
where
    D: Decoder,
    O: VideoOutput<FramePayload = D::Output>,
{
    fn track_id(&self) -> TrackId {
        self.track_id
    }

    fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    fn try_push_packet(
        &mut self,
        item: EpochItem<Packet>,
    ) -> Result<VideoEnqueueResult, VideoPipelineError> {
        if item.epoch() != self.epoch {
            return Ok(VideoEnqueueResult::EpochMismatch(item));
        }

        if item.value().track_id() != self.track_id {
            return Err(VideoPipelineError::WrongTrack);
        }

        if self.phase != VideoPhase::Feeding {
            return Err(VideoPipelineError::InputClosed);
        }

        let (epoch, packet) = item.into_parts();
        match self.packets.push(packet) {
            QueuePushResult::Accepted => Ok(VideoEnqueueResult::Accepted),
            QueuePushResult::Full(packet) => Ok(VideoEnqueueResult::Backpressure(EpochItem::new(
                epoch, packet,
            ))),
        }
    }

    fn end_input(&mut self, epoch: PlaybackEpoch) -> bool {
        if epoch != self.epoch {
            return false;
        }
        if self.phase == VideoPhase::Feeding {
            self.phase = VideoPhase::InputEnded
        }
        true
    }
    fn step(
        &mut self,
        audio_snapshot: ClockSnapshot,
        now: Instant,
    ) -> Result<VideoPipelineStepResult, VideoPipelineError> {
        // 收帧、丢弃边界前的帧，收到 EOS，都可能产生推进。
        let mut progressed = self.receive_one_frame()?;

        // 尝试处理已有帧，可能呈现、丢弃、等待或遇到背压。
        let presentation_result = self.present_pending_frame(audio_snapshot, now)?;

        progressed |= matches!(&presentation_result, VideoPipelineStepResult::Progress);

        // 即使帧正在等待呈现，也允许继续尝试提交一个输入
        progressed |= self.submit_one_input()?;

        // 完成状态优先于本轮是否发生推进。
        if self.phase == VideoPhase::DecoderDrained && self.pending_frame.is_none() {
            return Ok(VideoPipelineStepResult::DecoderDrained);
        }

        if progressed {
            return Ok(VideoPipelineStepResult::Progress);
        }
        if let VideoPipelineStepResult::WaitUntil { deadline } = presentation_result {
            return Ok(VideoPipelineStepResult::WaitUntil { deadline });
        }

        Ok(VideoPipelineStepResult::Blocked)
    }

    fn reset(&mut self, new_epoch: PlaybackEpoch) -> Result<(), VideoPipelineError> {
        let _ = self.packets.drain();
        self.decoder.flush().map_err(VideoPipelineError::Decode)?;
        self.output.flush().map_err(VideoPipelineError::Output)?;
        if let Some(pending_frame) = self.pending_frame.take() {
            self.output
                .discard(pending_frame)
                .map_err(VideoPipelineError::Output)?;
        }
        self.pending_input = None;
        self.phase = VideoPhase::Feeding;
        self.epoch = new_epoch;
        Ok(())
    }

    fn set_presentation_boundary(&mut self, boundary: MediaTime) {
        self.presentation_boundary = boundary
    }

    fn is_decoder_drained(&self) -> bool {
        self.phase == VideoPhase::DecoderDrained && self.pending_frame.is_none()
    }
}

pub(crate) trait VideoBranch {
    fn track_id(&self) -> TrackId;
    fn epoch(&self) -> PlaybackEpoch;
    fn try_push_packet(
        &mut self,
        item: EpochItem<Packet>,
    ) -> Result<VideoEnqueueResult, VideoPipelineError>;

    fn end_input(&mut self, epoch: PlaybackEpoch) -> bool;
    fn step(
        &mut self,
        audio_snapshot: ClockSnapshot,
        now: Instant,
    ) -> Result<VideoPipelineStepResult, VideoPipelineError>;

    fn reset(&mut self, new_epoch: PlaybackEpoch) -> Result<(), VideoPipelineError>;

    fn set_presentation_boundary(&mut self, boundary: MediaTime);

    fn is_decoder_drained(&self) -> bool;
}
