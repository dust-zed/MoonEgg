//! 负责
//! 对外命令、事件、配置和状态机
mod command;
mod config;
mod event;
mod state;

pub(crate) use command::PlayerCommand;
pub(crate) use state::{InvalidTransition, PlayerState, StateAction};
