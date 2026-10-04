use std::sync::Arc;

use crate::{
    error::{PlaybackError, RuntimeError},
    media::{AudioBuffer, MediaDelta, MediaTime, Rounding, TrackFormat, TrackInfo},
    pipeline::{
        DeferredAudioOutput, PlaybackEpoch, PlaybackPipeline, PlaybackPipelineError, VideoPipeline,
    },
    ports::{
        AudioBackendFactory, AudioOutput, AudioOutputFactory, Decoder, Demuxer, VideoBackendError,
        VideoBackendFactory,
    },
    runtime::worker::CancellationToken,
    timing::AvSync,
};

pub(crate) trait PlaybackPipelineFactory: Send + Sync + 'static {
    type Demux: Demuxer + 'static;
    type Decode: Decoder<Output = AudioBuffer> + 'static;
    type Output: AudioOutput + 'static;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError>;

    fn create_audio_components(
        &self,
        track: &TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError>;

    fn packet_capacity(&self) -> usize;

    /// 默认实现：创建纯音频播放管线
    fn build(
        &self,
        epoch: PlaybackEpoch,
        cancel: &CancellationToken,
    ) -> Result<PlaybackPipeline<Self::Demux, Self::Decode, Self::Output>, PlaybackError> {
        Self::check_canceled(cancel)?;
        let demuxer = self.open_demuxer(cancel)?;

        Self::check_canceled(cancel)?;

        let track = demuxer
            .tracks()
            .iter()
            .find(|track| matches!(track.format(), TrackFormat::Audio(_)))
            .cloned()
            .ok_or(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack))?;

        let (decoder, output) = self.create_audio_components(&track, cancel)?;

        Self::check_canceled(cancel)?;

        PlaybackPipeline::new(
            demuxer,
            decoder,
            output,
            track.id(),
            epoch,
            self.packet_capacity(),
        )
        .map_err(PlaybackError::Pipeline)
    }

    fn check_canceled(cancel: &CancellationToken) -> Result<(), PlaybackError> {
        if cancel.is_canceled() {
            Err(PlaybackError::Runtime(RuntimeError::Cancelled))
        } else {
            Ok(())
        }
    }
}

/// 现有的音频管线工厂。
///
/// B 负责解复用器和音频解码器
/// F 负责在 PCM 格式确定后创建音频输出。
pub(crate) struct BackendPlaybackFactory<B, F> {
    backend: B,
    output_factory: Arc<F>,
}

impl<B, F> BackendPlaybackFactory<B, F> {
    pub(crate) fn new(backend: B, output_factory: F) -> Self {
        Self {
            backend,
            output_factory: Arc::new(output_factory),
        }
    }
}

impl<B, F> PlaybackPipelineFactory for BackendPlaybackFactory<B, F>
where
    B: AudioBackendFactory,
    F: AudioOutputFactory,
{
    type Decode = B::Decode;
    type Demux = B::Demux;
    type Output = DeferredAudioOutput<Arc<F>>;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        Self::check_canceled(cancel)?;

        let demuxer = self.backend.open_demuxer().map_err(PlaybackError::Demux)?;
        Self::check_canceled(cancel)?;
        Ok(demuxer)
    }

    fn create_audio_components(
        &self,
        track: &TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError> {
        Self::check_canceled(cancel)?;
        let decoder = self
            .backend
            .create_audio_decoder(track)
            .map_err(PlaybackError::Decode)?;
        Self::check_canceled(cancel)?;

        let deferred_output = DeferredAudioOutput::new(Arc::clone(&self.output_factory));
        Ok((decoder, deferred_output))
    }

    fn packet_capacity(&self) -> usize {
        8
    }
}

/// 音视频管线工厂
///
/// 通过组合 BackendPlaybackFactory 复用音频构建逻辑。
/// audio_factory.backend 仍然是 B，保留完整的视频创建能力。
pub(crate) struct AvBackendPlaybackFactory<B, F> {
    audio_factory: BackendPlaybackFactory<B, F>,
}

impl<B, F> AvBackendPlaybackFactory<B, F>
where
    B: VideoBackendFactory,
    F: AudioOutputFactory,
{
    pub(crate) fn new(backend: B, output_factory: F) -> Self {
        let audio_factory = BackendPlaybackFactory::new(backend, output_factory);
        Self { audio_factory }
    }

    /// 创建一条视频管线。
    ///
    /// 调用方负责提供轨道、epoch 和初始展示边界。
    fn create_video_pipeline(
        &self,
        track: &TrackInfo,
        epoch: PlaybackEpoch,
        presentation_boundary: MediaTime,
        cancel: &CancellationToken,
    ) -> Result<VideoPipeline<B::VideoDecode, B::VideoOutput>, PlaybackError> {
        Self::check_canceled(cancel)?;

        // 平台后端决定怎样创建视频解码器和输出端。
        // core 不需要了解 MediaCodec 或 NativeWindow。
        let (video_decoder, video_output) = self
            .audio_factory
            .backend
            .create_video_components(track)
            .map_err(|error| match error {
                VideoBackendError::Decode(error) => PlaybackError::Decode(error),
                VideoBackendError::Output(error) => PlaybackError::VideoOutput(error),
            })?;
        // 创建平台组件可能花费时间，完成后重新检查取消状态。
        // 如果已经取消，上面的局部组件会自动被丢弃。
        Self::check_canceled(cancel)?;

        // 当前阶段的初始同步策略：
        // 不提前展示；落后超过 100ms 时允许丢帧。
        let early_tolerance = MediaDelta::from_nanoseconds(0);
        let late_tolerance = MediaDelta::from_nanoseconds(100_000_000);

        let sync = AvSync::new(early_tolerance, late_tolerance)
            .map_err(|error| PlaybackError::Pipeline(PlaybackPipelineError::Sync(error)))?;

        // 将平台组件和同步策略交给视频管线管理。
        let video_pipeline = VideoPipeline::new(
            video_decoder,
            video_output,
            track.id(),
            epoch,
            self.packet_capacity(),
            sync,
            presentation_boundary,
        )
        .map_err(|error| PlaybackError::Pipeline(PlaybackPipelineError::Video(error)))?;

        Self::check_canceled(cancel)?;
        Ok(video_pipeline)
    }
}

impl<B, F> PlaybackPipelineFactory for AvBackendPlaybackFactory<B, F>
where
    B: VideoBackendFactory,
    F: AudioOutputFactory,
{
    type Demux = B::Demux;
    type Decode = B::Decode;
    type Output = DeferredAudioOutput<Arc<F>>;

    fn open_demuxer(&self, cancel: &CancellationToken) -> Result<Self::Demux, PlaybackError> {
        self.audio_factory.open_demuxer(cancel)
    }

    fn create_audio_components(
        &self,
        track: &TrackInfo,
        cancel: &CancellationToken,
    ) -> Result<(Self::Decode, Self::Output), PlaybackError> {
        self.audio_factory.create_audio_components(track, cancel)
    }

    fn packet_capacity(&self) -> usize {
        self.audio_factory.packet_capacity()
    }

    /// 覆盖默认实现，创建包含音视和视频的播放管线。
    fn build(
        &self,
        epoch: PlaybackEpoch,
        cancel: &CancellationToken,
    ) -> Result<PlaybackPipeline<Self::Demux, Self::Decode, Self::Output>, PlaybackError> {
        Self::check_canceled(cancel)?;

        // 音视频共用一个解复用器。
        let demuxer = self.open_demuxer(cancel)?;

        Self::check_canceled(cancel)?;

        // 取出独立拥有的轨道信息。
        // 后面可以把 demuxer 移入播放管线，不留下对它的借用。
        let audio_track = demuxer
            .tracks()
            .iter()
            .find(|track| matches!(track.format(), TrackFormat::Audio(_)))
            .cloned()
            .ok_or(PlaybackError::Pipeline(PlaybackPipelineError::NoAudioTrack))?;

        let video_track = demuxer
            .tracks()
            .iter()
            .find(|track| matches!(track.format(), TrackFormat::Video(_)))
            .cloned()
            .ok_or(PlaybackError::Pipeline(PlaybackPipelineError::NoVideoTrack))?;
        // 与 PlaybackPipeline::new 使用相同的起始时间规则。
        // 这里确定展示边界，不修改音视频帧本身的 PTS。
        let presentation_boundary = match audio_track.start_time() {
            Some(timestamp) => timestamp
                .to_media_time(Rounding::TowardZero)
                .map_err(|error| PlaybackError::Pipeline(PlaybackPipelineError::Time(error)))?,
            None => MediaTime::from_nanoseconds(0),
        };
        let (audio_decoder, audio_output) = self.create_audio_components(&audio_track, cancel)?;

        // 视频分支使用同一个 epoch 和 统一的展示边界。
        let video_pipeline =
            self.create_video_pipeline(&video_track, epoch, presentation_boundary, cancel)?;

        Self::check_canceled(cancel)?;

        // 到这里，局部组件的所有权转移给总播放管线。
        // new_with_video 内部负责核对视频轨道和 epoch，
        // 并将视频分支保存到播放管线中。
        let playback = PlaybackPipeline::new_with_video(
            demuxer,
            audio_decoder,
            audio_output,
            audio_track.id(),
            epoch,
            self.packet_capacity(),
            video_pipeline,
        )
        .map_err(PlaybackError::Pipeline)?;

        Self::check_canceled(cancel)?;
        Ok(playback)
    }
}
