use crate::runtime::{ControlLoopExit, EffectLoopExit};

#[derive(Debug)]
pub(crate) enum ThreadTermination<T> {
    Exited(T),
    Paniced,
}

#[derive(Debug)]
pub(crate) struct ShutdownReport {
    pub(crate) release_sent: bool,
    pub(crate) control: ThreadTermination<ControlLoopExit>,
    pub(crate) effect: ThreadTermination<EffectLoopExit>,
}
