use std::time::{Duration, Instant};

use crate::{
    media::{AudioPcmFormat, MediaTime, Packet, Rounding, TimeError, TrackFormat, TrackId},
    pipeline::{
        EpochItem, PlaybackEpoch,
        audio::{AudioBranch, AudioEnqueueResult, AudioPipelineError, AudioStepResult},
        video::{VideoBranch, VideoEnqueueResult, VideoPipelineError, VideoPipelineStepResult},
    },
    ports::{AudioPlaybackPosition, DemuxError, Demuxer, ReadPacketResult},
    timing::{
        AudioClock, AudioProgressTracker, AvSyncError, ClockError, ClockSnapshot, MonotonicClock,
    },
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
    NoVideoTrack,

    Sync(AvSyncError),

    EpochMismatch {
        expected: PlaybackEpoch,
        actual: PlaybackEpoch,
    },

    InvalidSeekEpoch {
        current: PlaybackEpoch,
        requested: PlaybackEpoch,
    },
    MissingAudioClockObservation,
}

/// seek 成功后建立的位置关系。
///
/// 成功表示管线已经完成重定位，
/// 不表示第一份音频或视频已经实际呈现
#[derive(Debug, Clone, Copy)]
pub struct SeekOutcome {
    /// 用户原始请求
    pub requested: MediaTime,
    /// demuxer 实际返回的读取落点
    pub landed: MediaTime,
    /// 最早允许呈现内容的媒体时间
    pub presentation_boundary: MediaTime,
    /// 本次重定位所属的播放代次
    pub epoch: PlaybackEpoch,
}

/// pipeline 本轮从音频输出端收集到的观测
///
/// format 可能尚未确定，例如 decoder 还没有输出第一份 PCM。
/// position 表示设备 observed_at 时刻已经播放了多少 PCM frame
#[derive(Debug, Clone, Copy)]
struct AudioClockObservation {
    format: Option<AudioPcmFormat>,
    /// 当前代次首份被输出端接收的非空 PCM 的 PTS
    first_submitted_pts: Option<MediaTime>,
    position: AudioPlaybackPosition,
}

/// 音频设备帧数到媒体时间的映射
///
/// 未知采样率时不能进行换算；格式确定后，
/// 将两个锚点交给 AudioClock；并保留这个映射。
#[derive(Debug)]
enum AudioTimeMapping {
    WaitingForAudio {
        // 尚未确定实际 PCM 起点时，对外保持的逻辑位置
        provisional_media: MediaTime,
        /// 新位置开始前读取的设备计数基准
        based_played_frames: u64,
    },
    Ready(AudioClock),
}

impl AudioTimeMapping {
    fn new(provisional_media: MediaTime, based_played_frames: u64) -> Self {
        Self::WaitingForAudio {
            provisional_media,
            based_played_frames,
        }
    }

    fn snapshot(
        &mut self,
        observation: AudioClockObservation,
    ) -> Result<ClockSnapshot, ClockError> {
        match self {
            Self::Ready(clock) => clock.snapshot(observation.position),
            Self::WaitingForAudio {
                provisional_media,
                based_played_frames,
            } => {
                let (Some(format), Some(first_pts)) =
                    (observation.format, observation.first_submitted_pts)
                else {
                    // 采样率未知时冻结媒体时间
                    // 但快照的观察时刻使用本轮设备观测
                    return Ok(ClockSnapshot::new(
                        *provisional_media,
                        observation.position.observed_at(),
                    ));
                };

                let clock = AudioClock::new(format.sample_rate(), first_pts, *based_played_frames)?;

                let snapshot = clock.snapshot(observation.position)?;

                *self = Self::Ready(clock);

                Ok(snapshot)
            }
        }
    }
}

/// 音频会话内部的计时阶段。
///
/// 音频暂时或永久交出计时职责
/// 不会改变外层 PlaybackClockState::Audio 的会话身份。
#[derive(Debug)]
enum AudioClockPhase {
    FollowingAudio,

    Stalled {
        clock: MonotonicClock,
        stalled_at_frames: u64,
    },

    Ended {
        clock: MonotonicClock,
    },
}

/// 以音频设备为主要依据的会话时钟。
///
/// mapping 保存音频时间关系，临时接管期间也不修改它。
/// phase 决定本轮使用设备时钟还是单调时间
#[derive(Debug)]
struct AudioDrivenClock {
    mapping: AudioTimeMapping,
    phase: AudioClockPhase,

    /// 记录会话的运行意图。
    ///
    /// 音频设备自身由 pipeline 启停。
    /// 此字段决定新建立的临时/尾部时钟是否立即运行
    running: bool,
}

impl AudioDrivenClock {
    fn new(anchor_media: MediaTime, anchor_played_frames: u64) -> Self {
        Self {
            mapping: AudioTimeMapping::new(anchor_media, anchor_played_frames),
            phase: AudioClockPhase::FollowingAudio,
            running: false,
        }
    }

    /// 只读取时间，不在读取过程中决定停滞、恢复或结束
    fn snapshot(
        &mut self,
        observation: Option<AudioClockObservation>,
        now: Instant,
    ) -> Result<ClockSnapshot, PlaybackPipelineError> {
        match &mut self.phase {
            AudioClockPhase::FollowingAudio => {
                let observation =
                    observation.ok_or(PlaybackPipelineError::MissingAudioClockObservation)?;

                if observation.position.observed_at() > now {
                    return Err(PlaybackPipelineError::Clock(
                        ClockError::ObservationFromFuture,
                    ));
                }
                self.mapping
                    .snapshot(observation)
                    .map_err(PlaybackPipelineError::Clock)
            }
            AudioClockPhase::Stalled { clock, .. } | AudioClockPhase::Ended { clock } => {
                clock.snapshot(now).map_err(PlaybackPipelineError::Clock)
            }
        }
    }

    /// seek 后重新建立音频映射
    ///
    /// 旧的停滞或结束状态属于上一次播放位置
    fn reanchor(&mut self, anchor_media: MediaTime, anchor_played_frames: u64) {
        *self = Self::new(anchor_media, anchor_played_frames)
    }

    /// 因音频停滞而临时接管时间。
    ///
    /// 重复调用不会重建正在运行的单调时钟
    fn switch_to_fallback(&mut self, snapshot: ClockSnapshot, played_frames: u64) {
        if !matches!(&self.phase, AudioClockPhase::FollowingAudio) {
            return;
        }

        let clock = Self::monotonic_from_snapshot(snapshot, self.running);

        self.phase = AudioClockPhase::Stalled {
            clock,
            stalled_at_frames: played_frames,
        }
    }

    /// 只有设备真正多播放了 PCM，才恢复音频
    ///
    /// decoder 产出新帧或 output 接收新帧，
    /// 都不能证明声音已经继续播放。
    fn try_restore_audio_clock(&mut self, played_frames: u64) -> bool {
        let should_restore = match &self.phase {
            AudioClockPhase::Stalled {
                stalled_at_frames, ..
            } => played_frames > *stalled_at_frames,
            AudioClockPhase::FollowingAudio | AudioClockPhase::Ended { .. } => false,
        };
        if should_restore {
            // mapping 时钟保存原来的设备时间关系
            self.phase = AudioClockPhase::FollowingAudio;
        }
        should_restore
    }

    /// 音频真正排空后，让单调时钟继续驱动视频尾部
    fn finish_audio(&mut self, snapshot: ClockSnapshot) {
        if matches!(&self.phase, AudioClockPhase::Ended { .. }) {
            return;
        }

        let replacement = AudioClockPhase::Ended {
            clock: Self::monotonic_from_snapshot(snapshot, self.running),
        };

        let previous = std::mem::replace(&mut self.phase, replacement);
        if let AudioClockPhase::Stalled { clock, .. } = previous {
            // 已经存在临时时钟，保留它的锚点和暂停状态。
            // 变化的是计时阶段，而不是时间本身。
            self.phase = AudioClockPhase::Ended { clock }
        }
    }

    fn resume(&mut self, now: Instant) {
        match &mut self.phase {
            AudioClockPhase::FollowingAudio => {}
            AudioClockPhase::Stalled { clock, .. } | AudioClockPhase::Ended { clock } => {
                clock.resume(now);
            }
        }
        self.running = true
    }

    fn pause(&mut self, now: Instant) -> Result<(), ClockError> {
        match &mut self.phase {
            // 设备已经由 pipeline 暂停
            AudioClockPhase::FollowingAudio => {}
            AudioClockPhase::Stalled { clock, .. } | AudioClockPhase::Ended { clock } => {
                clock.pause(now)?;
            }
        }
        self.running = false;
        Ok(())
    }

    /// 使用同一份快照中的媒体时间和观察时刻建立锚点
    /// 使计时来源切换时保持关系
    fn monotonic_from_snapshot(snapshot: ClockSnapshot, running: bool) -> MonotonicClock {
        let mut clock = MonotonicClock::new(snapshot.media_time());

        if running {
            clock.resume(snapshot.observed_at());
        }
        clock
    }
}

/// 会话级的两种计时模式
///
///  Audio 内部即使暂时使用单调计时，仍是音频会话；
/// Monotonic 表示独立的单调计时模式，例如存视频
#[derive(Debug)]
enum PlaybackClockState {
    Audio(AudioDrivenClock),
    Monotonic(MonotonicClock),
}

impl PlaybackClockState {
    fn new_audio(anchor_media: MediaTime, anchor_played_frames: u64) -> Self {
        Self::Audio(AudioDrivenClock::new(anchor_media, anchor_played_frames))
    }

    fn new_monotonic(anchor_media: MediaTime) -> Self {
        Self::Monotonic(MonotonicClock::new(anchor_media))
    }

    fn snapshot(
        &mut self,
        observation: Option<AudioClockObservation>,
        now: Instant,
    ) -> Result<ClockSnapshot, PlaybackPipelineError> {
        match self {
            Self::Audio(clock) => clock.snapshot(observation, now),

            Self::Monotonic(clock) => clock.snapshot(now).map_err(PlaybackPipelineError::Clock),
        }
    }

    /// seek 保留会话模式，但重建该模式的时间映射
    /// 两种模式都会回到暂停状态。
    fn reanchor(
        &mut self,
        anchor_media: MediaTime,
        anchor_played_frames: Option<u64>,
    ) -> Result<(), PlaybackPipelineError> {
        match self {
            Self::Audio(clock) => {
                let played_frames = anchor_played_frames
                    .ok_or(PlaybackPipelineError::MissingAudioClockObservation)?;
                clock.reanchor(anchor_media, played_frames);
            }
            Self::Monotonic(clock) => *clock = MonotonicClock::new(anchor_media),
        }
        Ok(())
    }

    fn switch_to_fallback(&mut self, snapshot: ClockSnapshot, played_frames: u64) {
        if let Self::Audio(clock) = self {
            clock.switch_to_fallback(snapshot, played_frames);
        }
    }

    fn try_restore_audio_clock(&mut self, played_frames: u64) -> bool {
        match self {
            Self::Audio(clock) => clock.try_restore_audio_clock(played_frames),
            Self::Monotonic(_) => false,
        }
    }

    fn finish_audio(&mut self, snapshot: ClockSnapshot) {
        if let Self::Audio(clock) = self {
            clock.finish_audio(snapshot);
        }
    }

    fn resume(&mut self, now: Instant) {
        match self {
            Self::Audio(clock) => clock.resume(now),
            Self::Monotonic(clock) => clock.resume(now),
        }
    }

    fn pause(&mut self, now: Instant) -> Result<(), ClockError> {
        match self {
            Self::Audio(clock) => clock.pause(now),
            Self::Monotonic(clock) => clock.pause(now),
        }
    }
}

pub struct PlaybackPipeline<X> {
    demuxer: X,
    audio: Box<dyn AudioBranch>,
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

impl<X> PlaybackPipeline<X>
where
    X: Demuxer,
{
    const AUDIO_STALL_TIMEOUT: Duration = Duration::from_millis(200);

    pub fn new<A>(
        demuxer: X,
        mut audio: A,
        epoch: PlaybackEpoch,
    ) -> Result<Self, PlaybackPipelineError>
    where
        A: AudioBranch + 'static,
    {
        if audio.epoch() != epoch {
            return Err(PlaybackPipelineError::EpochMismatch {
                expected: epoch,
                actual: audio.epoch(),
            });
        }

        let audio_track = audio.track_id();

        let track = demuxer
            .tracks()
            .iter()
            .find(|track| track.id() == audio_track)
            .cloned()
            .ok_or(PlaybackPipelineError::TrackNotFound)?;

        let TrackFormat::Audio(_) = track.format() else {
            return Err(PlaybackPipelineError::NotAudioTrack);
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

        audio.set_presentation_boundary(start_time);

        let position = audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)?;

        let clock = PlaybackClockState::new_audio(start_time, position.played_frames());

        Ok(Self {
            demuxer,
            audio: Box::new(audio),
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

    pub(crate) fn new_with_video<A, V>(
        demuxer: X,
        audio: A,
        epoch: PlaybackEpoch,
        mut video: V,
    ) -> Result<Self, PlaybackPipelineError>
    where
        A: AudioBranch + 'static,
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

        let mut playback = Self::new(demuxer, audio, epoch)?;
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
        let observation = self.observe_audio_clock()?;

        let audio_stalled = self
            .audio_progress
            .observe(position)
            .map_err(PlaybackPipelineError::Clock)?;

        // 音频尚未结束时，允许临时时钟恢复到音频时钟。
        // 必须在生成快照前恢复，让本轮视频立即使用正确的来源。
        if !audio_finished {
            self.clock.try_restore_audio_clock(position.played_frames());
        }

        let clock_snapshot = self.clock.snapshot(Some(observation), Instant::now())?;

        // 真正播放结束后，进入视频尾部的持续计时状态。
        if audio_finished {
            self.clock.finish_audio(clock_snapshot);
        }

        // 每轮只推进一次视频分支
        let video_result = match self.video.as_mut() {
            Some(video) => video
                .step(clock_snapshot, Instant::now())
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

        let presentation_boundary = self.media_start.max(target);

        self.pending_packet = None;

        self.audio
            .reset(new_epoch)
            .map_err(PlaybackPipelineError::Audio)?;
        if let Some(video) = self.video.as_mut() {
            video
                .reset(new_epoch)
                .map_err(PlaybackPipelineError::Video)?;
        }

        // demuxer 可以落到更早的位置，为解码提供必要的参考数据。
        let landed = self
            .demuxer
            .seek(presentation_boundary)
            .map_err(PlaybackPipelineError::Demux)?;
        // reset 后重新读取设备计数，不假定 flush 会将它归零。
        let position = self
            .audio
            .playback_position()
            .map_err(PlaybackPipelineError::Audio)?;

        self.audio.set_presentation_boundary(presentation_boundary);
        if let Some(video) = self.video.as_mut() {
            video.set_presentation_boundary(presentation_boundary);
        }

        self.clock
            .reanchor(presentation_boundary, Some(position.played_frames()))?;

        self.audio_progress.reset();

        self.epoch = new_epoch;
        self.source_phase = SourcePhase::Reading;

        Ok(SeekOutcome {
            requested: target,
            landed,
            presentation_boundary,
            epoch: new_epoch,
        })
    }

    pub fn clock_snapshot(&mut self) -> Result<ClockSnapshot, PlaybackPipelineError> {
        let observation = self.observe_audio_clock()?;
        self.clock.snapshot(Some(observation), Instant::now())
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

    fn observe_audio_clock(&mut self) -> Result<AudioClockObservation, PlaybackPipelineError> {
        let position = self.playback_position()?;
        Ok(AudioClockObservation {
            format: self.audio.output_format(),
            first_submitted_pts: self.audio.first_submitted_pts(),
            position,
        })
    }
}
