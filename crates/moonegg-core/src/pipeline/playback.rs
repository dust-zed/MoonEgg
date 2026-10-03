use std::time::Instant;

use crate::{
    media::{
        AudioBuffer, AudioPcmFormat, MediaTime, Packet, Rounding, TimeError, TrackFormat, TrackId,
    },
    pipeline::{
        EpochItem, PlaybackEpoch,
        audio::{AudioEnqueueResult, AudioPipeline, AudioPipelineError, AudioStepResult},
        playback,
        video::{self, VideoBranch, VideoEnqueueResult, VideoPipelineError},
    },
    ports::{AudioOutput, AudioPlaybackPosition, Decoder, DemuxError, Demuxer, ReadPacketResult},
    timing::{AudioClock, ClockError, ClockSnapshot, MonotonicClock},
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
    Video(VideoPipelineError),
    Clock(ClockError),
    Time(TimeError),

    TrackNotFound,
    NotAudioTrack,
    NoAudioTrack,

    NotVideoTrack,

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

#[derive(Debug)]
enum PlaybackClockState {
    WaitingForFormat {
        anchor_media: MediaTime,
        anchor_played_frames: u64,
    },
    Ready(AudioClock),
    Monotonic(MonotonicClock),
}

impl PlaybackClockState {
    fn new(anchor_media: MediaTime, anchor_played_frames: u64) -> Self {
        Self::WaitingForFormat {
            anchor_media,
            anchor_played_frames,
        }
    }

    fn reanchor(&mut self, anchor_media: MediaTime, anchor_played_frames: u64) {
        *self = Self::new(anchor_media, anchor_played_frames)
    }

    fn switch_to_monotonic(&mut self, snapshot: ClockSnapshot) {
        *self = match self {
            PlaybackClockState::Monotonic(_) => return,
            _ => {
                let mut monotonic_clock = MonotonicClock::new(snapshot.media_time());
                monotonic_clock.resume(snapshot.observed_at());
                PlaybackClockState::Monotonic(monotonic_clock)
            }
        };
    }

    fn snapshot(
        &mut self,
        format: Option<AudioPcmFormat>,
        position: AudioPlaybackPosition,
        now: Instant,
    ) -> Result<ClockSnapshot, ClockError> {
        match self {
            Self::WaitingForFormat {
                anchor_media,
                anchor_played_frames,
            } => {
                let Some(format) = format else {
                    return Ok(ClockSnapshot::new(*anchor_media, position.observed_at()));
                };
                let clock =
                    AudioClock::new(format.sample_rate(), *anchor_media, *anchor_played_frames)?;
                let snapshot = clock.snapshot(position)?;
                *self = Self::Ready(clock);
                Ok(snapshot)
            }
            Self::Ready(clock) => clock.snapshot(position),
            Self::Monotonic(clock) => clock.snapshot(now),
        }
    }

    fn pause(&mut self, now: Instant) -> Result<(), ClockError> {
        match self {
            PlaybackClockState::Monotonic(monotonic) => monotonic.pause(now),
            _ => return Ok(()),
        }
    }

    fn resume(&mut self, now: Instant) {
        match self {
            PlaybackClockState::Monotonic(monotonic) => monotonic.resume(now),
            _ => {}
        }
    }
}

pub struct PlaybackPipeline<X, D, O> {
    demuxer: X,
    audio: AudioPipeline<D, O>,
    video: Option<Box<dyn VideoBranch>>,

    audio_track: TrackId,
    epoch: PlaybackEpoch,
    clock: PlaybackClockState,

    pending_packet: Option<EpochItem<Packet>>,
    source_phase: SourcePhase,
    duration_ms: Option<i64>,
    media_start: MediaTime,
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

        let TrackFormat::Audio(_) = track.format() else {
            return Err(PlaybackPipelineError::NoAudioTrack);
        };

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

        let mut audio = AudioPipeline::new(
            decoder,
            output,
            audio_track,
            epoch,
            packet_capacity,
            start_time,
        )
        .map_err(PlaybackPipelineError::Audio)?;

        let position = audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)?;

        let clock = PlaybackClockState::new(start_time, position.played_frames());

        Ok(Self {
            demuxer,
            audio,
            video: None,
            audio_track,
            epoch,
            clock,
            pending_packet: None,
            source_phase: SourcePhase::Reading,
            duration_ms,
            media_start: start_time,
        })
    }

    pub(crate) fn new_with_video<V>(
        demuxer: X,
        decoder: D,
        output: O,
        audio_track: TrackId,
        epoch: PlaybackEpoch,
        packet_capacity: usize,
        mut video: V,
    ) -> Result<Self, PlaybackPipelineError>
    where
        V: VideoBranch + 'static,
    {
        let track = demuxer
            .tracks()
            .iter()
            .find(|&track| track.id() == video.track_id())
            .ok_or(PlaybackPipelineError::TrackNotFound)?;
        if epoch != video.epoch() {
            return Err(PlaybackPipelineError::EpochMismatch {
                expected: epoch,
                actual: video.epoch(),
            });
        }

        let TrackFormat::Video(_) = track.format() else {
            return Err(PlaybackPipelineError::NotVideoTrack);
        };

        let mut playback = Self::new(
            demuxer,
            decoder,
            output,
            audio_track,
            epoch,
            packet_capacity,
        )?;
        video.set_presentation_boundary(playback.media_start);
        playback.video = Some(Box::new(video));
        Ok(playback)
    }

    fn dispatch_pending_packet(&mut self) -> Result<bool, PlaybackPipelineError> {
        let Some(item) = self.pending_packet.take() else {
            return Ok(false);
        };

        let track_id = item.value().track_id();

        if self.audio_track == track_id {
            return match self
                .audio
                .try_push_packet(item)
                .map_err(PlaybackPipelineError::Audio)?
            {
                AudioEnqueueResult::Accepted => Ok(true),

                AudioEnqueueResult::Backpressure(item) => {
                    self.pending_packet = Some(item);
                    Ok(false)
                }

                AudioEnqueueResult::EpochMismatch(item) => {
                    Err(PlaybackPipelineError::EpochMismatch {
                        expected: self.epoch,
                        actual: item.epoch(),
                    })
                }
            };
        }
        let Some(video) = self.video.as_mut() else {
            return Ok(true);
        };

        if track_id != video.track_id() {
            return Ok(true);
        }

        match video
            .try_push_packet(item)
            .map_err(PlaybackPipelineError::Video)?
        {
            VideoEnqueueResult::Accepted => Ok(true),
            VideoEnqueueResult::Backpressure(item) => {
                self.pending_packet = Some(item);
                Ok(false)
            }
            VideoEnqueueResult::EpochMismatch(item) => Err(PlaybackPipelineError::EpochMismatch {
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
                self.pending_packet = Some(EpochItem::new(self.epoch, packet));

                // 尝试立即交给音频分支。
                // 如果背压，函数会把 packet 返回 pending
                self.dispatch_pending_packet()?;

                // 即使分发遇到背压，本轮也确实读出了 packet
                Ok(true)
            }

            ReadPacketResult::NotReady => Ok(false),

            ReadPacketResult::EndOfStream => {
                self.end_selected_inputs()?;

                self.source_phase = SourcePhase::Ended;
                Ok(true)
            }
        }
    }

    fn end_selected_inputs(&mut self) -> Result<(), PlaybackPipelineError> {
        if !self.audio.end_input(self.epoch) {
            return Err(PlaybackPipelineError::EpochMismatch {
                expected: self.epoch,
                actual: self.audio.epoch(),
            });
        }

        if let Some(video) = self.video.as_mut()
            && !video.end_input(self.epoch)
        {
            return Err(PlaybackPipelineError::EpochMismatch {
                expected: self.epoch,
                actual: video.epoch(),
            });
        }

        Ok(())
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
        match self.audio.start() {
            Ok(_) => {
                let now = Instant::now();
                self.clock.resume(now);
                Ok(())
            }
            Err(error) => Err(PlaybackPipelineError::Audio(error)),
        }
    }

    pub fn pause(&mut self) -> Result<(), PlaybackPipelineError> {
        match self.audio.pause() {
            Ok(_) => {
                let now = Instant::now();
                self.clock.pause(now).map_err(PlaybackPipelineError::Clock)
            }
            Err(error) => Err(PlaybackPipelineError::Audio(error)),
        }
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
        let presentation_boundary = self.media_start.max(landed);

        if let Some(video) = self.video.as_mut() {
            video
                .reset(new_epoch)
                .map_err(PlaybackPipelineError::Video)?;
            video.set_presentation_boundary(presentation_boundary);
        }
        self.audio.set_presentation_boundary(presentation_boundary);
        self.clock
            .reanchor(presentation_boundary, position.played_frames());

        self.epoch = new_epoch;
        self.source_phase = SourcePhase::Reading;

        Ok(SeekOutcome {
            requested: target,
            landed: presentation_boundary,
            epoch: new_epoch,
        })
    }

    pub fn clock_snapshot(&mut self) -> Result<ClockSnapshot, PlaybackPipelineError> {
        let position = self.playback_position()?;
        let format = self.audio.output_format();
        self.clock
            .snapshot(format, position, Instant::now())
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
        let audio_finished = self
            .audio
            .is_finished()
            .map_err(PlaybackPipelineError::Audio)?;
        let video_drained = match self.video.as_ref() {
            Some(video) => video.is_decoder_drained(),
            None => true,
        };
        Ok(audio_finished && video_drained)
    }
}
