use std::sync::mpsc::{Receiver, Sender};

use crate::{
    media::MediaTime,
    pipeline::{EpochError, PlaybackEpoch},
    player::{InvalidTransition, PlayerCommand, PlayerEvent, PlayerState},
    runtime::{
        control_types::{ControlError, ControlMessage, ControlOutcome, ControlResult},
        player_controller::PlayerController,
        worker::WorkerEvent,
    },
};

pub struct ControlLoop {
    controller: PlayerController,
}

impl ControlLoop {
    pub fn new() -> Self {
        Self {
            controller: PlayerController::new(),
        }
    }

    // 这里使用 mut self 而不是 &mut self 很重要：
    // 启动控制线程时，把整个 ControlLoop 所有权移动进线程，从此只有该线程能访问状态。
    // 接管控制循环，在当前线程持续处理消息
    // 控制器由该循环独占，通过消息接收外部请求
    pub fn run(
        mut self,
        message_receiver: Receiver<ControlMessage>,
        result_sender: Sender<ControlResult>,
    ) -> ControlLoopExit {
        loop {
            let message = match message_receiver.recv() {
                Ok(message) => message,
                Err(_) => {
                    return ControlLoopExit::InputDisconnected;
                }
            };

            let result = self.handle_message(message);

            let fatal = matches!(
                &result,
                Err(ControlError::UnexpectedWorkEvent { .. } | ControlError::Epoch(_))
            );

            let released = self.controller.state() == PlayerState::Released;

            if result_sender.send(result).is_err() {
                return ControlLoopExit::OutputDisconnected;
            }

            if fatal {
                return ControlLoopExit::FatalControlError;
            }

            if released {
                return ControlLoopExit::Released;
            }
        }
    }

    pub fn handle_message(
        &mut self,
        message: ControlMessage,
    ) -> Result<ControlOutcome, ControlError> {
        match message {
            ControlMessage::Command(command) => self.controller.handle_command(command),
            ControlMessage::WorkerEvent(event) => self.controller.handle_worker_event(event),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlLoopExit {
    Released,
    InputDisconnected,
    OutputDisconnected,
    FatalControlError,
}
