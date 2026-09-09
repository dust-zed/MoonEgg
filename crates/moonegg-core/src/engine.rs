//! 负责
//! 装配其他组件，提供播放引擎入口
use std::{
    io,
    sync::mpsc::{self, Receiver, RecvError, SendError, Sender},
    thread::{self, JoinHandle},
};

use crate::{
    player::PlayerCommand,
    runtime::{ControlLoop, ControlLoopExit, ControlMessage, ControlResult},
};

pub(crate) struct PlayerEngine {
    message_sender: Sender<ControlMessage>,
    result_receiver: Receiver<ControlResult>,
    control_thread: JoinHandle<ControlLoopExit>,
}

impl PlayerEngine {
    pub(crate) fn new() -> io::Result<Self> {
        let (message_sender, message_receiver) = mpsc::channel::<ControlMessage>();
        let (result_sender, result_receiver) = mpsc::channel::<ControlResult>();

        let control_thread = thread::Builder::new()
            .name("moonegg-control".to_owned())
            .spawn(move || ControlLoop::new().run(message_receiver, result_sender))?;

        Ok(Self {
            message_sender,
            result_receiver,
            control_thread,
        })
    }

    pub(crate) fn send_command(
        &self,
        command: PlayerCommand,
    ) -> Result<(), SendError<ControlMessage>> {
        self.message_sender.send(ControlMessage::Command(command))
    }

    pub(crate) fn message_sender(&self) -> Sender<ControlMessage> {
        self.message_sender.clone()
    }

    pub(crate) fn recv_result(&self) -> Result<ControlResult, RecvError> {
        self.result_receiver.recv()
    }
}
