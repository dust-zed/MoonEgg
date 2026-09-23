use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc::TryRecvError},
};

use moonegg_core::{PlayerCommand, PlayerEngine, media::MediaTime};

use crate::event::NativeEvent;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum PlayerBridgeError {
    #[error("创建播放器失败: {reason}")]
    CreatedFailed { reason: String },
    #[error("播放器内部锁已损坏")]
    LockPoisoned,
    #[error("播放器未正常关闭: {reason}")]
    ShutdownFailed { reason: String },
    #[error("播放器已经释放")]
    Released,
    #[error("播放器命令通道已关闭")]
    CommandChannelClosed,
    #[error("播放器事件通道已关闭")]
    EventChannelClosed,
    #[error("跳转位置不能为负数: {position_ms} ms")]
    NegativeSeekPosition { position_ms: i64 },
    #[error("跳转位置超出支持范围： {position_ms} ms")]
    SeekPositionOverflow { position_ms: i64 },
}

#[derive(uniffi::Object)]
pub struct NativePlayer {
    engine: Mutex<Option<PlayerEngine>>,
}

#[uniffi::export]
impl NativePlayer {
    #[uniffi::constructor]
    pub fn new(path: String) -> Result<Arc<Self>, PlayerBridgeError> {
        let path = PathBuf::from(path);

        #[cfg(target_os = "android")]
        let result = PlayerEngine::new_wav(path, moonegg_android::AndroidAudioOutputFactory);

        #[cfg(not(target_os = "android"))]
        let result = PlayerEngine::new_wav_simulated(path);

        let engine = result.map_err(|error| PlayerBridgeError::CreatedFailed {
            reason: error.to_string(),
        })?;

        Ok(Arc::new(Self {
            engine: Mutex::new(Some(engine)),
        }))
    }

    pub fn release(&self) -> Result<(), PlayerBridgeError> {
        let mut guard = self
            .engine
            .lock()
            .map_err(|_| PlayerBridgeError::LockPoisoned)?;

        let Some(engine) = guard.take() else {
            return Ok(());
        };

        let report = engine.shutdown();

        if report.is_clean() {
            Ok(())
        } else {
            Err(PlayerBridgeError::ShutdownFailed {
                reason: format!("{report:?}"),
            })
        }
    }

    pub fn prepare(&self) -> Result<(), PlayerBridgeError> {
        self.send(PlayerCommand::Prepare)
    }

    pub fn play(&self) -> Result<(), PlayerBridgeError> {
        self.send(PlayerCommand::Play)
    }

    pub fn pause(&self) -> Result<(), PlayerBridgeError> {
        self.send(PlayerCommand::Pause)
    }

    pub fn stop(&self) -> Result<(), PlayerBridgeError> {
        self.send(PlayerCommand::Stop)
    }

    pub fn poll_event(&self) -> Result<Option<NativeEvent>, PlayerBridgeError> {
        let guard = self
            .engine
            .lock()
            .map_err(|_| PlayerBridgeError::LockPoisoned)?;
        let engine = guard.as_ref().ok_or(PlayerBridgeError::Released)?;

        match engine.try_recv_event() {
            Ok(event) => Ok(Some(event.into())),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(PlayerBridgeError::EventChannelClosed),
        }
    }

    /// 请求跳转到指定位置，单位为毫秒。
    ///
    /// 负数或无法转换为纳秒的位置会返回参数错误。
    /// 返回成功仅表示命令已提交，不代表跳转已完成。
    /// 当前状态是否允许跳转，由核心状态机判断。
    pub fn seek(&self, position_ms: i64) -> Result<(), PlayerBridgeError> {
        if position_ms < 0 {
            return Err(PlayerBridgeError::NegativeSeekPosition { position_ms });
        }
        let position_ns_wide = position_ms as i128 * 1_000_000;
        let position_ns = i64::try_from(position_ns_wide)
            .map_err(|_| PlayerBridgeError::SeekPositionOverflow { position_ms })?;
        let media_time = MediaTime::from_nanoseconds(position_ns);
        self.send(PlayerCommand::Seek(media_time))
    }
}

impl NativePlayer {
    fn send(&self, command: PlayerCommand) -> Result<(), PlayerBridgeError> {
        let guard = self
            .engine
            .lock()
            .map_err(|_| PlayerBridgeError::LockPoisoned)?;
        let engine = guard.as_ref().ok_or(PlayerBridgeError::Released)?;
        engine
            .send_command(command)
            .map_err(|_| PlayerBridgeError::CommandChannelClosed)
    }
}

impl Drop for NativePlayer {
    fn drop(&mut self) {
        let slot = self
            .engine
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(engine) = slot.take() {
            let _ = engine.shutdown();
        }
    }
}
