use std::sync::mpsc::{Receiver, Sender};

use crate::{
    error::PlaybackError,
    media::MediaTime,
    pipeline::PlaybackEpoch,
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
        feedback: &EffectFeedback,
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

#[derive(Debug, Clone)]
pub struct EffectFeedback {
    epoch: PlaybackEpoch,
    sender: Sender<ControlMessage>,
}

impl EffectFeedback {
    fn new(epoch: PlaybackEpoch, sender: Sender<ControlMessage>) -> Self {
        Self { epoch, sender }
    }

    pub(super) fn preparation_completed(&self) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::PreparationCompleted { epoch: self.epoch })
    }

    pub(super) fn seek_completed(
        &self,
        requested: MediaTime,
        landed: MediaTime,
    ) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::SeekCompleted {
            epoch: self.epoch,
            requested,
            landed,
        })
    }

    pub(super) fn failed(&self, error: PlaybackError) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::Failed {
            epoch: self.epoch,
            error,
        })
    }

    fn send(&self, event: WorkerEvent) -> Result<(), FeedbackDisconnected> {
        self.sender
            .send(ControlMessage::WorkerEvent(event))
            .map_err(|_| FeedbackDisconnected)
    }

    pub(super) const fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    pub(super) fn audio_progress(&self, media_time: MediaTime) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::AudioProgress {
            epoch: self.epoch,
            media_time,
        })
    }

    pub(super) fn duration_changed(
        &self,
        duration_ms: Option<i64>,
    ) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::DurationChanged {
            epoch: self.epoch,
            duration_ms,
        })
    }

    pub(super) fn playback_completed(&self) -> Result<(), FeedbackDisconnected> {
        self.send(WorkerEvent::PlaybackCompleted { epoch: self.epoch })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedbackDisconnected;

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

            let effect_feedback = EffectFeedback::new(epoch, feedback_sender.clone());

            match executor.execute(effect, &effect_feedback) {
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
