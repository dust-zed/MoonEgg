use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc::TryRecvError},
};

use moonegg_core::{PlayerCommand, PlayerEngine};

use crate::event::{self, NativeEvent};

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
}

#[derive(uniffi::Object)]
pub struct NativePlayer {
    engine: Mutex<Option<PlayerEngine>>,
}

#[uniffi::export]
impl NativePlayer {
    #[uniffi::constructor]
    pub fn new(path: String) -> Result<Arc<Self>, PlayerBridgeError> {
        let engine = PlayerEngine::new_wav_simulated(PathBuf::from(path)).map_err(|err| {
            PlayerBridgeError::CreatedFailed {
                reason: err.to_string(),
            }
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
