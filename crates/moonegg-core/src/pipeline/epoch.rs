#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaybackEpoch(u64);

impl PlaybackEpoch {
    pub const INITIAL: Self = Self(0);

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Result<Self, EpochError> {
        let value = self.0.checked_add(1).ok_or(EpochError::Overflow)?;

        Ok(Self(value))
    }
}

pub struct EpochItem<T> {
    epoch: PlaybackEpoch,
    value: T,
}

impl<T> EpochItem<T> {
    pub const fn new(epoch: PlaybackEpoch, value: T) -> Self {
        Self { epoch, value }
    }

    pub const fn epoch(&self) -> PlaybackEpoch {
        self.epoch
    }

    pub const fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn into_parts(self) -> (PlaybackEpoch, T) {
        (self.epoch, self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochError {
    Overflow,
}
