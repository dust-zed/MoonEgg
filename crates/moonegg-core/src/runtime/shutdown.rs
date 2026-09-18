use crate::runtime::{ControlLoopExit, EffectLoopExit};

#[derive(Debug)]
pub(crate) enum ThreadTermination<T> {
    Exited(T),
    Panicked,
}

#[derive(Debug)]
pub struct ShutdownReport {
    pub(crate) release_sent: bool,
    pub(crate) control: ThreadTermination<ControlLoopExit>,
    pub(crate) effect: ThreadTermination<EffectLoopExit>,
}

impl ShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.release_sent
            && matches!(
                &self.control,
                ThreadTermination::Exited(ControlLoopExit::Released)
            )
            && matches!(
                &self.effect,
                ThreadTermination::Exited(EffectLoopExit::Released)
            )
    }
}
