//! 负责
//! 装配其他组件，提供播放引擎入口
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    backends::{SimulatedAudioOutputFactory, WavPlaybackFactory},
    player::{PlayerCommand, PlayerEvent},
    ports::{AudioBackendFactory, AudioOutputFactory, VideoBackendFactory},
    runtime::{
        self, AvBackendPlaybackFactory, BackendPlaybackFactory, ControlLoop, ControlLoopExit,
        ControlMessage, ControlResult, EffectExecutor, EffectLoopExit, PlaybackEffectExecutor,
        PlaybackPipelineFactory, ShutdownReport, ThreadTermination, run_effect_loop,
    },
    source::FileSource,
};

struct EngineRuntime {
    message_sender: Sender<ControlMessage>,
    control_thread: JoinHandle<ControlLoopExit>,
    effect_thread: JoinHandle<EffectLoopExit>,
}

impl EngineRuntime {
    fn shutdown(self) -> ShutdownReport {
        let EngineRuntime {
            message_sender,
            control_thread,
            effect_thread,
        } = self;

        let release_sent = message_sender
            .send(ControlMessage::Command(PlayerCommand::Release))
            .is_ok();
        let control = match control_thread.join() {
            Err(_) => ThreadTermination::Panicked,
            Ok(exit) => ThreadTermination::Exited(exit),
        };
        let effect = match effect_thread.join() {
            Err(_) => ThreadTermination::Panicked,
            Ok(exit) => ThreadTermination::Exited(exit),
        };

        ShutdownReport {
            release_sent,
            control,
            effect,
        }
    }
}

pub struct PlayerEngine {
    runtime: Option<EngineRuntime>,
    event_receiver: Receiver<PlayerEvent>,
}

impl PlayerEngine {
    pub(crate) fn new<E>(executor: E) -> io::Result<Self>
    where
        E: EffectExecutor + 'static,
    {
        let (message_sender, message_receiver) = mpsc::channel::<ControlMessage>();
        let (result_sender, result_receiver) = mpsc::channel::<ControlResult>();
        let (event_sender, event_receiver) = mpsc::channel::<PlayerEvent>();

        let control_thread = thread::Builder::new()
            .name("moonegg-control".to_owned())
            .spawn(move || ControlLoop::new().run(message_receiver, result_sender))?;

        let feedback_sender = message_sender.clone();
        let effect_thread_result = thread::Builder::new()
            .name("moonegg-effect".to_owned())
            .spawn(move || {
                run_effect_loop(result_receiver, event_sender, feedback_sender, executor)
            });

        let effect_thread = match effect_thread_result {
            Ok(thread) => thread,
            Err(error) => {
                // 为什么 drop(message_sender) 能让控制线程退出？
                // 因为 EffectLoop 启动失败时，其闭包也会被销毁，闭包里的 feedback_sender 随之销毁。此时再销毁原始 message_sender：
                // 所有 ControlMessage.Sender消失，message_recv()返回错误，ControlLoop 返回 InputDisconnected，join 完成
                drop(message_sender);

                let _ = control_thread.join();
                return Err(error);
            }
        };

        Ok(Self {
            runtime: Some(EngineRuntime {
                message_sender,
                control_thread,
                effect_thread,
            }),
            event_receiver,
        })
    }

    pub fn new_wav<F>(path: PathBuf, output_factory: F) -> io::Result<Self>
    where
        F: AudioOutputFactory,
    {
        Self::new_wav_source(FileSource::Path(path), output_factory)
    }

    pub fn new_wav_source<F>(source: FileSource, output_factory: F) -> io::Result<Self>
    where
        F: AudioOutputFactory,
    {
        Self::new_audio(WavPlaybackFactory::new(source, output_factory))
    }

    pub fn new_wav_simulated(path: PathBuf) -> io::Result<Self> {
        Self::new_wav(path, SimulatedAudioOutputFactory::new(4096))
    }

    pub fn new_with_backend<B, F>(backend: B, output_factory: F) -> io::Result<Self>
    where
        B: AudioBackendFactory,
        F: AudioOutputFactory,
    {
        let factory = BackendPlaybackFactory::new(backend, output_factory);
        Self::new_audio(factory)
    }

    pub fn new_with_av_backend<B, F>(backend: B, output_factory: F) -> io::Result<Self>
    where
        B: VideoBackendFactory,
        F: AudioOutputFactory,
    {
        let factory = AvBackendPlaybackFactory::new(backend, output_factory);
        let engine = Self::new_audio(factory)?;
        Ok(engine)
    }

    pub fn send_command(&self, command: PlayerCommand) -> Result<(), SendError<ControlMessage>> {
        let message = ControlMessage::Command(command);
        let Some(runtime) = self.runtime.as_ref() else {
            return Err(SendError(message));
        };
        runtime.message_sender.send(message)
    }

    pub(crate) fn recv_event(&self) -> Result<PlayerEvent, RecvError> {
        self.event_receiver.recv()
    }

    pub fn try_recv_event(&self) -> Result<PlayerEvent, TryRecvError> {
        self.event_receiver.try_recv()
    }

    pub fn shutdown(mut self) -> ShutdownReport {
        let runtime = self
            .runtime
            .take()
            .expect("尚未析构的 PlayerEngine 必须持有运行时");
        runtime.shutdown()
    }

    pub(crate) fn new_audio<F>(factory: F) -> io::Result<Self>
    where
        F: PlaybackPipelineFactory,
    {
        Self::new(PlaybackEffectExecutor::new(factory))
    }

    pub fn recv_event_timeout(&self, timeout: Duration) -> Result<PlayerEvent, RecvTimeoutError> {
        self.event_receiver.recv_timeout(timeout)
    }
}

impl Drop for PlayerEngine {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let _ = runtime.shutdown();
        }
    }
}
