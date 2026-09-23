use crate::{
    media::{AudioBuffer, MediaTime, Packet, Rounding, TimeError, TrackFormat, TrackId},
    pipeline::{
        EpochItem, PlaybackEpoch,
        audio::{AudioEnqueueResult, AudioPipeline, AudioPipelineError, AudioStepResult},
    },
    ports::{AudioOutput, AudioPlaybackPosition, Decoder, DemuxError, Demuxer, ReadPacketResult},
    timing::{AudioClock, ClockError, ClockSnapshot},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourcePhase {
    Reading,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStepResult {
    Progress,
    Blocked,
    DecoderDrained,
}

#[derive(Debug)]
pub enum PlaybackPipelineError {
    Demux(DemuxError),
    Audio(AudioPipelineError),
    Clock(ClockError),
    Time(TimeError),

    TrackNotFound,
    NotAudioTrack,
    NoAudioTrack,

    EpochMismatch {
        expected: PlaybackEpoch,
        actual: PlaybackEpoch,
    },

    InvalidSeekEpoch {
        current: PlaybackEpoch,
        requested: PlaybackEpoch,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SeekOutcome {
    pub requested: MediaTime,
    pub landed: MediaTime,
    pub epoch: PlaybackEpoch,
}

pub struct PlaybackPipeline<X, D, O> {
    demuxer: X,
    audio: AudioPipeline<D, O>,

    audio_track: TrackId,
    epoch: PlaybackEpoch,
    clock: AudioClock,

    pending_packet: Option<EpochItem<Packet>>,
    source_phase: SourcePhase,
    duration_ms: Option<i64>,
}

impl<X, D, O> PlaybackPipeline<X, D, O>
where
    X: Demuxer,
    D: Decoder<Output = AudioBuffer>,
    O: AudioOutput,
{
    pub fn new(
        demuxer: X,
        decoder: D,
        output: O,
        audio_track: TrackId,
        epoch: PlaybackEpoch,
        packet_capacity: usize,
    ) -> Result<Self, PlaybackPipelineError> {
        let track = demuxer
            .tracks()
            .iter()
            .find(|track| track.id() == audio_track)
            .ok_or(PlaybackPipelineError::TrackNotFound)?;

        let TrackFormat::Audio(format) = track.format() else {
            return Err(PlaybackPipelineError::NoAudioTrack);
        };

        let sample_rate = format.sample_rate();

        let start_time = match track.start_time() {
            Some(timestamp) => timestamp
                .to_media_time(Rounding::TowardZero)
                .map_err(PlaybackPipelineError::Time)?,

            None => MediaTime::from_nanoseconds(0),
        };

        let duration_ms = match track.duration() {
            Some(duration) => Some(
                duration
                    .to_milliseconds()
                    .map_err(PlaybackPipelineError::Time)?,
            ),
            None => None,
        };

        let mut audio = AudioPipeline::new(decoder, output, audio_track, epoch, packet_capacity)
            .map_err(PlaybackPipelineError::Audio)?;

        let position = audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)?;

        let clock = AudioClock::new(sample_rate, start_time, position.played_frames())
            .map_err(PlaybackPipelineError::Clock)?;

        Ok(Self {
            demuxer,
            audio,
            audio_track,
            epoch,
            clock,
            pending_packet: None,
            source_phase: SourcePhase::Reading,
            duration_ms,
        })
    }

    fn dispatch_pending_packet(&mut self) -> Result<bool, PlaybackPipelineError> {
        let Some(item) = self.pending_packet.take() else {
            return Ok(false);
        };

        match self
            .audio
            .try_push_packet(item)
            .map_err(PlaybackPipelineError::Audio)?
        {
            AudioEnqueueResult::Accepted => Ok(true),

            AudioEnqueueResult::Backpressure(item) => {
                self.pending_packet = Some(item);
                Ok(false)
            }

            AudioEnqueueResult::EpochMismatch(item) => Err(PlaybackPipelineError::EpochMismatch {
                expected: self.epoch,
                actual: item.epoch(),
            }),
        }
    }

    fn step_source(&mut self) -> Result<bool, PlaybackPipelineError> {
        // 上一份数据还没有交接成功，先重试
        if self.pending_packet.is_some() {
            return self.dispatch_pending_packet();
        }

        // 正常 EOF 后不再调用 read_packet
        if self.source_phase == SourcePhase::Ended {
            return Ok(false);
        }

        match self
            .demuxer
            .read_packet()
            .map_err(PlaybackPipelineError::Demux)?
        {
            ReadPacketResult::Packet(packet) => {
                if packet.track_id() != self.audio_track {
                    return Ok(true);
                }

                self.pending_packet = Some(EpochItem::new(self.epoch, packet));

                // 尝试立即交给音频分支。
                // 如果背压，函数会把 packet 返回 pending
                self.dispatch_pending_packet()?;

                // 即使分发遇到背压，本轮也确实读出了 packet
                Ok(true)
            }

            ReadPacketResult::NotReady => Ok(false),

            ReadPacketResult::EndOfStream => {
                if !self.audio.end_input(self.epoch) {
                    return Err(PlaybackPipelineError::EpochMismatch {
                        expected: self.epoch,
                        actual: self.audio.epoch(),
                    });
                }
                self.source_phase = SourcePhase::Ended;
                Ok(true)
            }
        }
    }

    pub fn step(&mut self) -> Result<PlaybackStepResult, PlaybackPipelineError> {
        // 先推进下游，让音频队列有机会腾出空间。
        let mut progressed = match self.audio.step().map_err(PlaybackPipelineError::Audio)? {
            AudioStepResult::Progress => true,
            AudioStepResult::Blocked => false,
            AudioStepResult::DecoderDrained => {
                return Ok(PlaybackStepResult::DecoderDrained);
            }
        };

        // 无论音频本轮是否推进，都尝试推进源
        progressed |= self.step_source()?;

        Ok(if progressed {
            PlaybackStepResult::Progress
        } else {
            PlaybackStepResult::Blocked
        })
    }

    pub fn start(&mut self) -> Result<(), PlaybackPipelineError> {
        self.audio.start().map_err(PlaybackPipelineError::Audio)
    }

    pub fn pause(&mut self) -> Result<(), PlaybackPipelineError> {
        self.audio.pause().map_err(PlaybackPipelineError::Audio)
    }

    pub const fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    pub fn seek(
        &mut self,
        target: MediaTime,
        new_epoch: PlaybackEpoch,
    ) -> Result<SeekOutcome, PlaybackPipelineError> {
        if new_epoch.value() <= self.epoch.value() {
            return Err(PlaybackPipelineError::InvalidSeekEpoch {
                current: self.epoch,
                requested: new_epoch,
            });
        }

        self.pending_packet = None;
        self.audio
            .reset(new_epoch)
            .map_err(PlaybackPipelineError::Audio)?;

        let landed = self
            .demuxer
            .seek(target)
            .map_err(PlaybackPipelineError::Demux)?;

        let position = self
            .audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)?;

        self.clock.reanchor(landed, position.played_frames());

        self.epoch = new_epoch;
        self.source_phase = SourcePhase::Reading;

        Ok(SeekOutcome {
            requested: target,
            landed,
            epoch: new_epoch,
        })
    }

    pub fn clock_snapshot(&mut self) -> Result<ClockSnapshot, PlaybackPipelineError> {
        let position = self.playback_position()?;

        self.clock
            .snapshot(position)
            .map_err(PlaybackPipelineError::Clock)
    }

    pub fn playback_position(&mut self) -> Result<AudioPlaybackPosition, PlaybackPipelineError> {
        self.audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)
    }

    pub fn duration_ms(&self) -> Option<i64> {
        self.duration_ms
    }

    pub fn is_finished(&mut self) -> Result<bool, PlaybackPipelineError> {
        self.audio
            .is_finished()
            .map_err(PlaybackPipelineError::Audio)
    }
}
