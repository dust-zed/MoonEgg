use crate::media::MediaTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerCommand {
    Prepare,
    Play,
    Pause,
    Seek(MediaTime),
    Stop,
    Release,
}
