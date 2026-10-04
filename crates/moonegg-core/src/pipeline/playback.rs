use std::time::{Duration, Instant};

use crate::{
    media::{
        AudioBuffer, AudioPcmFormat, MediaTime, Packet, Rounding, TimeError, TrackFormat, TrackId,
    },
    pipeline::{
        EpochItem, PlaybackEpoch, VideoStepResult,
        audio::{AudioEnqueueResult, AudioPipeline, AudioPipelineError, AudioStepResult},
        video::{
            self, VideoBranch, VideoEnqueueResult, VideoPipelineError, VideoPipelineStepResult,
        },
    },
    ports::{AudioOutput, AudioPlaybackPosition, Decoder, DemuxError, Demuxer, ReadPacketResult},
    timing::{AudioClock, AudioProgressTracker, ClockError, ClockSnapshot, MonotonicClock},
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
    WaitUntil { deadline: Instant },
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
enum AudioClockState {
    WaitingForFormat {
        anchor_media: MediaTime,
        anchor_played_frames: u64,
    },
    Ready(AudioClock),
}

impl AudioClockState {
    fn new(anchor_media: MediaTime, anchor_played_frames: u64) -> Self {
        Self::WaitingForFormat {
            anchor_media,
            anchor_played_frames,
        }
    }

    fn snapshot(
        &mut self,
        format: Option<AudioPcmFormat>,
        position: AudioPlaybackPosition,
    ) -> Result<ClockSnapshot, ClockError> {
        match self {
            Self::Ready(clock) => clock.snapshot(position),
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
        }
    }
}

#[derive(Debug)]
enum PlaybackClockSource {
    Audio,
    AudioStalled {
        clock: MonotonicClock,
        stalled_at_frames: u64,
    },
    AudioEnded {
        clock: MonotonicClock,
    },
}

#[derive(Debug)]
struct PlaybackClockState {
    audio_clock: AudioClockState,
    source: PlaybackClockSource,
}

impl PlaybackClockState {
    fn new(anchor_media: MediaTime, anchor_played_frames: u64) -> Self {
        Self {
            audio_clock: AudioClockState::new(anchor_media, anchor_played_frames),
            source: PlaybackClockSource::Audio,
        }
    }

    fn reanchor(&mut self, anchor_media: MediaTime, anchor_played_frames: u64) {
        // seek 后重新建立音频时间映射
        // 同时丢弃上一轮的临时时钟或尾部播放时钟
        *self = Self::new(anchor_media, anchor_played_frames);
    }

    fn snapshot(
        &mut self,
        format: Option<AudioPcmFormat>,
        position: AudioPlaybackPosition,
        now: Instant,
    ) -> Result<ClockSnapshot, ClockError> {
        // 快照只根据当前来源读取时间，不在这里决定切换策略
        match &mut self.source {
            PlaybackClockSource::Audio => self.audio_clock.snapshot(format, position),
            PlaybackClockSource::AudioStalled { clock, .. }
            | PlaybackClockSource::AudioEnded { clock } => clock.snapshot(now),
        }
    }

    fn switch_to_fallback(&mut self, snapshot: ClockSnapshot, played_frames: u64) {
        // 只有在使用音频时钟时，才进入临时接管状态。
        // 重复调用不能重置已经运行的单调时钟。
        if !matches!(&self.source, PlaybackClockSource::Audio) {
            return;
        }

        // 两个锚点来自同一份快照，保持时间对应关系。
        let mut clock = MonotonicClock::new(snapshot.media_time());
        clock.resume(snapshot.observed_at());

        self.source = PlaybackClockSource::AudioStalled {
            clock,
            stalled_at_frames: played_frames,
        };
        // audio_clock 没有被修改，恢复音频时还会使用它。
    }

    fn try_restore_audio_clock(&mut self, played_frames: u64) -> bool {
        // 只有临时停滞且音频帧数实际增加，才允许恢复。
        let should_restore = match &self.source {
            PlaybackClockSource::AudioStalled {
                stalled_at_frames, ..
            } => played_frames > *stalled_at_frames,
            PlaybackClockSource::Audio | PlaybackClockSource::AudioEnded { .. } => false,
        };

        if should_restore {
            // 恢复使用原有音频时钟。
            // 不修改它的锚点，也不使用单调时钟的位置重新对齐它。
            self.source = PlaybackClockSource::Audio;
        }
        should_restore
    }

    fn switch_to_monotonic(&mut self, snapshot: ClockSnapshot) {
        // 把旧状态取出来，以便把其中的 clock 移动到新状态中。
        // 临时放入 Audio，保证字段始终有一个合法值。
        let previous_source = std::mem::replace(&mut self.source, PlaybackClockSource::Audio);
        let clock = match previous_source {
            PlaybackClockSource::Audio => {
                // 之前由音频计时，现在根据最后的快照建立单调时钟
                let mut clock = MonotonicClock::new(snapshot.media_time());
                clock.resume(snapshot.observed_at());
                clock
            }
            PlaybackClockSource::AudioStalled { clock, .. }
            | PlaybackClockSource::AudioEnded { clock } => {
                // 已经有单调时钟，直接保留
                // 不重置锚点，也不改变暂停状态
                clock
            }
        };
        self.source = PlaybackClockSource::AudioEnded { clock };
    }

    fn pause(&mut self, now: Instant) -> Result<(), ClockError> {
        match &mut self.source {
            PlaybackClockSource::Audio => {
                // AudioClock 依据设备播放帧数计算时间
                // 音频设备的暂停由 AudioPipeline 负责。
                Ok(())
            }
            PlaybackClockSource::AudioEnded { clock }
            | PlaybackClockSource::AudioStalled { clock, .. } => clock.pause(now),
        }
    }

    fn resume(&mut self, now: Instant) {
        match &mut self.source {
            PlaybackClockSource::Audio => {}
            PlaybackClockSource::AudioEnded { clock }
            | PlaybackClockSource::AudioStalled { clock, .. } => {
                clock.resume(now);
            }
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

    audio_progress: AudioProgressTracker,
}

impl<X, D, O> PlaybackPipeline<X, D, O>
where
    X: Demuxer,
    D: Decoder<Output = AudioBuffer>,
    O: AudioOutput,
{
    const AUDIO_STALL_TIMEOUT: Duration = Duration::from_millis(200);

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
            audio_progress: AudioProgressTracker::new(Self::AUDIO_STALL_TIMEOUT),
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

    fn is_source_blocked_on_video(&self) -> bool {
        let Some(video) = self.video.as_ref() else {
            return false;
        };
        let Some(epoch_item) = self.pending_packet.as_ref() else {
            return false;
        };
        let pending_packet = epoch_item.value();
        pending_packet.track_id() == video.track_id()
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

    fn step_video(&mut self) -> Result<VideoPipelineStepResult, PlaybackPipelineError> {
        if self.video.is_none() {
            return Ok(VideoPipelineStepResult::DecoderDrained);
        };
        // 先检查音频是否真正播放完成。
        // 这个调用也会帮助 Android 输出端继续写出剩余 PCM
        let audio_finished = self
            .audio
            .is_finished()
            .map_err(PlaybackPipelineError::Audio)?;
        // 本轮只读取一次音频位置。
        // 停滞判断、恢复判断和时钟快照，都使用这份观测
        let position = self.playback_position()?;
        let format = self.audio.output_format();

        let audio_stalled = self
            .audio_progress
            .observe(position)
            .map_err(PlaybackPipelineError::Clock)?;

        // 音频尚未结束时，允许临时时钟恢复到音频时钟。
        // 必须在生成快照前恢复，让本轮视频立即使用正确的来源。
        if !audio_finished {
            self.clock.try_restore_audio_clock(position.played_frames());
        }

        let clock_snapshot = self
            .clock
            .snapshot(format, position, Instant::now())
            .map_err(PlaybackPipelineError::Clock)?;

        // 真正播放结束后，进入视频尾部的持续计时状态。
        if audio_finished {
            self.clock.switch_to_monotonic(clock_snapshot);
        }
        // 同步判断的时间不能早于快照的观察时刻。
        let now = Instant::now();

        // 每轮只推进一次视频分支
        let video_result = match self.video.as_mut() {
            Some(video) => video
                .step(clock_snapshot, now)
                .map_err(PlaybackPipelineError::Video)?,
            None => return Ok(VideoPipelineStepResult::DecoderDrained),
        };

        let source_blocked_on_video = self.is_source_blocked_on_video();

        // 到这里，对视频分支的可变借用已经结束，
        // 音频持续停滞、视频等待时间、视频背压挡住数据源
        if !audio_finished
            && audio_stalled
            && matches!(&video_result, VideoPipelineStepResult::WaitUntil { .. })
            && source_blocked_on_video
        {
            self.clock
                .switch_to_fallback(clock_snapshot, position.played_frames());
        }
        // 临时切换的效果由下一轮体现，不在这里重复推进视频
        Ok(video_result)
    }

    pub fn step(&mut self) -> Result<PlaybackStepResult, PlaybackPipelineError> {
        let audio_result = self.audio.step().map_err(PlaybackPipelineError::Audio)?;
        let video_result = self.step_video()?;

        let audio_drained = matches!(&audio_result, AudioStepResult::DecoderDrained);
        let video_drained = matches!(&video_result, VideoPipelineStepResult::DecoderDrained);

        let mut progressed = matches!(&audio_result, AudioStepResult::Progress)
            || matches!(&video_result, VideoPipelineStepResult::Progress);

        // 无论音频本轮是否推进，都尝试推进源
        progressed |= self.step_source()?;

        if audio_drained && video_drained {
            return Ok(PlaybackStepResult::DecoderDrained);
        }

        Ok(if progressed {
            PlaybackStepResult::Progress
        } else if let VideoPipelineStepResult::WaitUntil { deadline } = video_result {
            PlaybackStepResult::WaitUntil { deadline }
        } else {
            PlaybackStepResult::Blocked
        })
    }

    pub fn start(&mut self) -> Result<(), PlaybackPipelineError> {
        match self.audio.start() {
            Ok(_) => {
                let now = Instant::now();
                self.clock.resume(now);
                self.audio_progress.reset();
                Ok(())
            }
            Err(error) => Err(PlaybackPipelineError::Audio(error)),
        }
    }

    pub fn pause(&mut self) -> Result<(), PlaybackPipelineError> {
        match self.audio.pause() {
            Ok(_) => {
                let now = Instant::now();
                self.clock
                    .pause(now)
                    .map_err(PlaybackPipelineError::Clock)?;
                self.audio_progress.reset();
                Ok(())
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
        self.audio_progress.reset();

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
