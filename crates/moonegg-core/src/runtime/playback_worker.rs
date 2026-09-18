use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use crate::{
    error::{self, PlaybackError, RuntimeError},
    media::{AudioBuffer, MediaTime},
    pipeline::{self, PlaybackEpoch, PlaybackPipeline, PlaybackStepResult},
    ports::{AudioOutput, Decoder, Demuxer},
    runtime::{
        effect_executor::EffectFeedback,
        worker::{CancellationToken, WorkerHandle},
    },
};

#[derive(Debug)]
pub(crate) enum PlaybackWorkerAction {
    Start,
    Pause,
    Seek { target: MediaTime },
}

#[derive(Debug)]
struct PlaybackWorkerCommand {
    action: PlaybackWorkerAction,
    feedback: EffectFeedback,
}

struct PlaybackWorker<X, D, O> {
    pipeline: Option<PlaybackPipeline<X, D, O>>,
    commands: Receiver<PlaybackWorkerCommand>,
    cancel: CancellationToken,
    feedback: EffectFeedback,

    running: bool,
    decoder_drained: bool,
    completion_reported: bool,
}

impl<X, D, O> PlaybackWorker<X, D, O>
where
    X: Demuxer,
    D: Decoder<Output = AudioBuffer>,
    O: AudioOutput,
{
    fn apply_command(&mut self, command: PlaybackWorkerCommand) -> Result<(), PlaybackError> {
        // 在执行操作前更新反馈上下文
        self.feedback = command.feedback;
        let command_epoch = self.feedback.epoch();

        let pipeline = self
            .pipeline
            .as_mut()
            .ok_or(PlaybackError::Runtime(RuntimeError::SessionFailed))?;

        match command.action {
            PlaybackWorkerAction::Start => {
                if command_epoch != pipeline.epoch() {
                    return Err(PlaybackError::Runtime(RuntimeError::EpochMismatch {
                        expected: pipeline.epoch(),
                        actual: command_epoch,
                    }));
                }
                if self.completion_reported {
                    return Ok(());
                }
                pipeline.start().map_err(PlaybackError::Pipeline)?;
                self.running = true;
            }

            PlaybackWorkerAction::Pause => {
                if command_epoch != pipeline.epoch() {
                    return Err(PlaybackError::Runtime(RuntimeError::EpochMismatch {
                        expected: pipeline.epoch(),
                        actual: command_epoch,
                    }));
                }
                pipeline.pause().map_err(PlaybackError::Pipeline)?;
                self.running = false;
            }

            PlaybackWorkerAction::Seek { target } => {
                pipeline
                    .seek(target, command_epoch)
                    .map_err(PlaybackError::Pipeline)?;

                self.decoder_drained = false;
                self.completion_reported = false;

                if self.running {
                    pipeline.start().map_err(PlaybackError::Pipeline)?;
                }
            }
        }
        Ok(())
    }

    fn close_pipeline(&mut self) {
        self.running = false;
        self.decoder_drained = false;
        self.completion_reported = false;

        if let Some(mut pipeline) = self.pipeline.take() {
            let _ = pipeline.pause();
            drop(pipeline);
        }
    }

    fn report_failure(&mut self, error: PlaybackError) -> bool {
        self.close_pipeline();

        self.feedback.failed(error).is_ok()
    }

    fn run(mut self, preparation_error: Option<PlaybackError>) {
        const RETRY_INTERVAL: Duration = Duration::from_millis(10);
        const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

        let mut last_progress_report = Instant::now();

        if self.cancel.is_canceled() {
            self.close_pipeline();
            return;
        }

        let feedback_connected = match preparation_error {
            Some(error) => self.report_failure(error),

            None => self.feedback.preparation_completed().is_ok(),
        };

        if !feedback_connected {
            self.close_pipeline();
            return;
        }

        let mut should_wait = true;

        loop {
            if self.cancel.is_canceled() {
                break;
            }

            let command = if should_wait {
                match self.commands.recv_timeout(RETRY_INTERVAL) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match self.commands.try_recv() {
                    Ok(command) => Some(command),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            };

            if self.cancel.is_canceled() {
                break;
            }

            let now = Instant::now();

            if let Some(command) = command {
                if let Err(error) = self.apply_command(command) {
                    if !self.report_failure(error) {
                        break;
                    }
                    should_wait = true;
                    continue;
                }
            }

            if now.duration_since(last_progress_report) >= PROGRESS_INTERVAL {
                last_progress_report = now;

                if let Some(pipeline) = self.pipeline.as_mut() {
                    let snapshot = match pipeline.clock_snapshot() {
                        Ok(snapshot) => snapshot,

                        Err(error) => {
                            if !self.report_failure(PlaybackError::Pipeline(error)) {
                                break;
                            }
                            should_wait = true;
                            continue;
                        }
                    };

                    if self.feedback.audio_progress(snapshot.media_time()).is_err() {
                        break;
                    }
                }
            }

            match self.poll_completion() {
                Ok(true) => {
                    if self.feedback.playback_completed().is_err() {
                        break;
                    }
                }
                Ok(false) => {}

                Err(error) => {
                    if !self.report_failure(error) {
                        break;
                    }
                    should_wait = true;
                    continue;
                }
            }

            if !self.running || self.decoder_drained {
                should_wait = true;
                continue;
            }

            let Some(pipeline) = self.pipeline.as_mut() else {
                should_wait = true;
                continue;
            };

            if self.cancel.is_canceled() {
                break;
            }

            match pipeline.step() {
                Ok(PlaybackStepResult::Progress) => {
                    should_wait = false;
                }

                Ok(PlaybackStepResult::Blocked) => {
                    should_wait = true;
                }

                Ok(PlaybackStepResult::DecoderDrained) => {
                    self.decoder_drained = true;
                    should_wait = true;
                }

                Err(error) => {
                    if !self.report_failure(PlaybackError::Pipeline(error)) {
                        break;
                    }
                    should_wait = true;
                }
            }
        }
        self.close_pipeline();
    }

    fn poll_completion(&mut self) -> Result<bool, PlaybackError> {
        if !self.running || !self.decoder_drained || self.completion_reported {
            return Ok(false);
        }

        let pipeline = self
            .pipeline
            .as_mut()
            .ok_or(PlaybackError::Runtime(RuntimeError::SessionFailed))?;

        if !pipeline.is_finished().map_err(PlaybackError::Pipeline)? {
            return Ok(false);
        }

        pipeline.pause().map_err(PlaybackError::Pipeline)?;

        self.running = false;
        self.completion_reported = true;

        Ok(true)
    }
}

pub(crate) struct PlaybackWorkerHandle {
    commands: Sender<PlaybackWorkerCommand>,
    worker: Option<WorkerHandle>,
}

impl PlaybackWorkerHandle {
    pub(crate) fn send(
        &self,
        action: PlaybackWorkerAction,
        feedback: EffectFeedback,
    ) -> Result<(), PlaybackError> {
        self.commands
            .send(PlaybackWorkerCommand { action, feedback })
            .map_err(|_| PlaybackError::Runtime(RuntimeError::WorkerDisconnected))
    }

    fn stop_worker(&mut self) -> Result<(), PlaybackError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };

        worker.cancel();

        worker
            .join()
            .map_err(|_| PlaybackError::Runtime(RuntimeError::WorkerPanicked))
    }

    pub(crate) fn shutdown(mut self) -> Result<(), PlaybackError> {
        self.stop_worker()
    }
}

impl Drop for PlaybackWorkerHandle {
    fn drop(&mut self) {
        // 防止 executor 异常退出时把工作线程遗留在后台
        // 正常关闭应显式调用 shutdown，以便上报 join 错误
        let _ = self.stop_worker();
    }
}

pub(crate) fn spawn_playback_worker<X, D, O, F>(
    feedback: EffectFeedback,
    build: F,
) -> Result<PlaybackWorkerHandle, PlaybackError>
where
    X: Demuxer + 'static,
    D: Decoder<Output = AudioBuffer> + 'static,
    O: AudioOutput + 'static,
    F: FnOnce(
            PlaybackEpoch,
            &CancellationToken,
        ) -> Result<PlaybackPipeline<X, D, O>, PlaybackError>
        + Send
        + 'static,
{
    let (command_sender, command_receiver) = mpsc::channel();

    let cancel = CancellationToken::new();
    let thread_cancel = cancel.clone();
    let epoch = feedback.epoch();

    let thread = thread::Builder::new()
        .name("moonegg-playback".to_owned())
        .spawn(move || {
            if thread_cancel.is_canceled() {
                return;
            }

            let (pipeline, preparation_error) = match build(epoch, &thread_cancel) {
                Ok(pipeline) => (Some(pipeline), None),
                Err(error) => (None, Some(error)),
            };
            let worker = PlaybackWorker {
                pipeline,
                commands: command_receiver,
                cancel: thread_cancel,
                feedback,
                running: false,
                decoder_drained: false,
                completion_reported: false,
            };

            worker.run(preparation_error);
        })
        .map_err(|error| PlaybackError::Runtime(RuntimeError::THreadSpawn(error)))?;

    Ok(PlaybackWorkerHandle {
        commands: command_sender,
        worker: Some(WorkerHandle::new(cancel, thread)),
    })
}
