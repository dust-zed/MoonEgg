//! 负责
//! 装配其他组件，提供播放引擎入口
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvError, RecvTimeoutError, SendError, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    backends::WavPlaybackFactory,
    player::{PlayerCommand, PlayerEvent},
    runtime::{
        AudioPipelineFactory, ControlLoop, ControlLoopExit, ControlMessage, ControlResult,
        EffectExecutor, EffectLoopExit, PlaybackEffectExecutor, ShutdownReport, ThreadTermination,
        run_effect_loop,
    },
};

pub(crate) struct PlayerEngine {
    message_sender: Sender<ControlMessage>,
    event_receiver: Receiver<PlayerEvent>,
    control_thread: JoinHandle<ControlLoopExit>,
    effect_thread: JoinHandle<EffectLoopExit>,
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
            message_sender,
            event_receiver,
            control_thread,
            effect_thread,
        })
    }

    pub fn new_wav_simulated(path: PathBuf) -> io::Result<Self> {
        Self::new_audio(WavPlaybackFactory::new(path))
    }

    pub(crate) fn send_command(
        &self,
        command: PlayerCommand,
    ) -> Result<(), SendError<ControlMessage>> {
        self.message_sender.send(ControlMessage::Command(command))
    }

    pub(crate) fn recv_event(&self) -> Result<PlayerEvent, RecvError> {
        self.event_receiver.recv()
    }

    pub(crate) fn shutdown(self) -> ShutdownReport {
        let PlayerEngine {
            message_sender,
            event_receiver,
            control_thread,
            effect_thread,
        } = self;

        let release_sent = message_sender
            .send(ControlMessage::Command(PlayerCommand::Release))
            .is_ok();
        let effect = match effect_thread.join() {
            Ok(exit) => ThreadTermination::Exited(exit),
            Err(_) => ThreadTermination::Panicked,
        };

        let control = match control_thread.join() {
            Ok(exit) => ThreadTermination::Exited(exit),
            Err(_) => ThreadTermination::Panicked,
        };

        drop(event_receiver);

        ShutdownReport {
            release_sent,
            control,
            effect,
        }
    }

    pub(crate) fn new_audio<F>(factory: F) -> io::Result<Self>
    where
        F: AudioPipelineFactory,
    {
        Self::new(PlaybackEffectExecutor::new(factory))
    }

    pub(crate) fn recv_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<PlayerEvent, RecvTimeoutError> {
        self.event_receiver.recv_timeout(timeout)
    }
}
