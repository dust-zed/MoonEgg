use std::sync::Arc;

use crate::{
    error::{PlaybackError, RuntimeError},
    runtime::{
        ControlEffect, EffectExecutor,
        effect_executor::EffectFeedback,
        pipeline_factory::AudioPipelineFactory,
        playback_worker::{PlaybackWorkerAction, PlaybackWorkerHandle, spawn_playback_worker},
        worker,
    },
};

pub(crate) struct PlaybackEffectExecutor<F> {
    factory: Arc<F>,
    worker: Option<PlaybackWorkerHandle>,
}

impl<F> PlaybackEffectExecutor<F>
where
    F: AudioPipelineFactory,
{
    pub(crate) fn new(factory: F) -> Self {
        Self {
            factory: Arc::new(factory),
            worker: None,
        }
    }

    fn begin_preapre(&mut self, feedback: &EffectFeedback) -> Result<(), PlaybackError> {
        if self.worker.is_some() {
            return Err(PlaybackError::Runtime(RuntimeError::WorkerAlreadyExists));
        }

        let factory = Arc::clone(&self.factory);

        let worker = spawn_playback_worker(feedback.clone(), move |epoch, cancel| {
            factory.build(epoch, cancel)
        })?;

        self.worker = Some(worker);

        Ok(())
    }

    fn send_to_worker(
        &self,
        action: PlaybackWorkerAction,
        feedback: &EffectFeedback,
    ) -> Result<(), PlaybackError> {
        let worker = self
            .worker
            .as_ref()
            .ok_or(PlaybackError::Runtime(RuntimeError::WorkerNotPrepared))?;
        worker.send(action, feedback.clone())
    }

    fn shutdown_worker(&mut self) -> Result<(), PlaybackError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };

        worker.shutdown()
    }
}

impl<F> EffectExecutor for PlaybackEffectExecutor<F>
where
    F: AudioPipelineFactory,
{
    fn execute(
        &mut self,
        effect: super::ControlEffect,
        feedback: &EffectFeedback,
    ) -> Result<(), PlaybackError> {
        if effect.epoch() != feedback.epoch() {
            return Err(PlaybackError::Runtime(RuntimeError::EpochMismatch {
                expected: effect.epoch(),
                actual: feedback.epoch(),
            }));
        }

        match effect {
            ControlEffect::BeginPrepare { .. } => self.begin_preapre(feedback),

            ControlEffect::StartPlayback { .. } => {
                self.send_to_worker(PlaybackWorkerAction::Start, feedback)
            }

            ControlEffect::PausePlayback { .. } => {
                self.send_to_worker(PlaybackWorkerAction::Pause, feedback)
            }

            ControlEffect::Seek { target, .. } => {
                self.send_to_worker(PlaybackWorkerAction::Seek { target }, feedback)
            }

            ControlEffect::Stop { .. }
            | ControlEffect::Release { .. }
            | ControlEffect::CleanupAfterFailure { .. } => self.shutdown_worker(),
        }
    }
}
