//! 负责
//! 对外命令、事件、配置和状态机
mod command;
mod config;
mod event;
mod state;

pub use command::PlayerCommand;
pub use event::PlayerEvent;
pub use state::PlayerState;
pub(crate) use state::{InvalidTransition, StateAction};
