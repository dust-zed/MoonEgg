#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Preparing,
    Ready,
    Playing,
    Paused,
    Error,
    Released,
}

impl PlayerState {
    pub fn transition(self, action: StateAction) -> Result<Self, InvalidTransition> {
        match (self, action) {
            (Self::Released, StateAction::Release) => Ok(Self::Released),

            (Self::Released, _) => Err(InvalidTransition::new(self, action)),

            (_, StateAction::Release) => Ok(Self::Released),

            (Self::Idle, StateAction::BeginPrepare) => Ok(Self::Preparing),
            (Self::Preparing, StateAction::PreparationCompleted) => Ok(Self::Ready),

            (Self::Ready | Self::Paused, StateAction::Play) => Ok(Self::Playing),

            (Self::Playing, StateAction::Pause) => Ok(Self::Paused),

            (Self::Playing, StateAction::Play) => Ok(Self::Playing),

            (Self::Paused, StateAction::Pause) => Ok(Self::Paused),

            (Self::Ready | Self::Playing | Self::Paused, StateAction::Seek) => Ok(self),

            (
                Self::Idle
                | Self::Preparing
                | Self::Ready
                | Self::Playing
                | Self::Paused
                | Self::Error,
                StateAction::Stop,
            ) => Ok(Self::Idle),

            (
                Self::Idle | Self::Preparing | Self::Ready | Self::Playing | Self::Paused,
                StateAction::FatalError,
            ) => Ok(Self::Error),

            (Self::Error, StateAction::FatalError) => Ok(Self::Error),

            _ => Err(InvalidTransition {
                state: self,
                action,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateAction {
    BeginPrepare,
    PreparationCompleted,
    Play,
    Pause,
    Stop,
    Seek,
    Release,
    FatalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    state: PlayerState,
    action: StateAction,
}

impl InvalidTransition {
    pub const fn new(state: PlayerState, action: StateAction) -> Self {
        Self { state, action }
    }

    pub const fn state(self) -> PlayerState {
        self.state
    }

    pub const fn action(self) -> StateAction {
        self.action
    }
}
