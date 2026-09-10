//! 负责
//! 工作线程、控制循环、取消、关闭和 join 顺序
mod control_loop;
mod effect_executor;
mod shutdown;
mod worker;

pub(crate) use control_loop::{
    ControlEffect, ControlLoop, ControlLoopExit, ControlMessage, ControlResult,
};
pub(crate) use effect_executor::{EffectExecutor, EffectLoopExit, run_effect_loop};
pub(crate) use shutdown::{ShutdownReport, ThreadTermination};
