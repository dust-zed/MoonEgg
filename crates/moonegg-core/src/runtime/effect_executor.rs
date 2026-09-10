use std::sync::mpsc::{Receiver, Sender};

use crate::{
    error::PlaybackError,
    player::PlayerEvent,
    runtime::{
        ControlMessage, ControlResult,
        control_loop::{ControlEffect, ControlError},
        worker::WorkerEvent,
    },
};

pub trait EffectExecutor: Send {
    fn execute(
        &mut self,
        effect: ControlEffect,
        feedback: &Sender<ControlMessage>,
    ) -> Result<(), PlaybackError>;
}

#[derive(Debug)]
pub enum EffectLoopExit {
    Released,
    ResultsDisconnected,
    EventsDisconnected,
    FeedbackDisconnected,
    ControlFailed(ControlError),
    TerminalEffectFailed(PlaybackError),
}

pub fn run_effect_loop<E>(
    result_receiver: Receiver<ControlResult>,
    event_sender: Sender<PlayerEvent>,
    feedback_sender: Sender<ControlMessage>,
    mut executor: E,
) -> EffectLoopExit
where
    E: EffectExecutor,
{
    loop {
        let result = match result_receiver.recv() {
            Ok(result) => result,
            Err(_) => {
                return EffectLoopExit::ResultsDisconnected;
            }
        };

        let outcome = match result {
            Ok(outcome) => outcome,

            Err(ControlError::CommandRejected {
                command,
                transition,
            }) => {
                let event = PlayerEvent::CommandRejected {
                    command,
                    state: transition.state(),
                };

                if event_sender.send(event).is_err() {
                    return EffectLoopExit::EventsDisconnected;
                }
                continue;
            }
            Err(error) => {
                return EffectLoopExit::ControlFailed(error);
            }
        };

        let event_disconnected = match outcome.event {
            Some(event) => event_sender.send(event).is_err(),
            None => false,
        };

        if let Some(effect) = outcome.effect {
            let epoch = effect.epoch();

            let releasing = matches!(&effect, ControlEffect::Release { .. });

            let cleanup_after_failure =
                matches!(&effect, ControlEffect::CleanupAfterFailure { .. });

            match executor.execute(effect, &feedback_sender) {
                Ok(()) => {
                    if event_disconnected {
                        return EffectLoopExit::EventsDisconnected;
                    }

                    if releasing {
                        return EffectLoopExit::Released;
                    }
                }
                Err(error) if releasing || cleanup_after_failure => {
                    return EffectLoopExit::TerminalEffectFailed(error);
                }

                Err(error) => {
                    let message = ControlMessage::WorkerEvent(WorkerEvent::Failed { epoch, error });
                    if feedback_sender.send(message).is_err() {
                        return EffectLoopExit::FeedbackDisconnected;
                    }

                    if event_disconnected {
                        return EffectLoopExit::EventsDisconnected;
                    }
                }
            }
        } else if event_disconnected {
            return EffectLoopExit::EventsDisconnected;
        }
    }
}
