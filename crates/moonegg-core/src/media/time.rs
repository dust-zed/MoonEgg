#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeBase {
    // 一个 tick 等于 numerator / denominator 秒
    numerator: u32,
    denominator: u32,
}

impl TimeBase {
    pub const NANOSECONDS: Self = Self {
        numerator: 1,
        denominator: 1_000_000_000,
    };

    pub fn new(numerator: u32, denominator: u32) -> Result<Self, TimeError> {
        if numerator == 0 || denominator == 0 {
            return Err(TimeError::InvalidTimeBase);
        }
        let divisor = gcd(numerator, denominator);

        Ok(TimeBase {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub fn from_hz(hz: u32) -> Result<Self, TimeError> {
        Self::new(1, hz)
    }

    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[derive(Debug, Clone, Copy)]
pub struct Timestamp {
    ticks: i64,
    time_base: TimeBase,
}

impl Timestamp {
    pub const fn new(ticks: i64, time_base: TimeBase) -> Self {
        Self { ticks, time_base }
    }

    pub const fn ticks(self) -> i64 {
        self.ticks
    }

    pub const fn time_base(self) -> TimeBase {
        self.time_base
    }

    pub fn rescale(self, target: TimeBase, rounding: Rounding) -> Result<Self, TimeError> {
        let numerator = i128::from(self.ticks)
            .checked_mul(i128::from(self.time_base.numerator()))
            .and_then(|value| value.checked_mul(i128::from(target.denominator())))
            .ok_or(TimeError::Overflow)?;
        let denominator = i128::from(self.time_base.denominator())
            .checked_mul(i128::from(target.numerator))
            .ok_or(TimeError::Overflow)?;

        let ticks = match rounding {
            Rounding::TowardZero => numerator / denominator,
            Rounding::Nearest => {
                let quotient = numerator / denominator;
                let remainder = numerator % denominator;
                let remainder_magnitude = remainder.abs();

                let should_round_away_from_zero = remainder_magnitude
                    .checked_mul(2)
                    .ok_or(TimeError::Overflow)?
                    >= denominator;
                let adjustment = if numerator.is_negative() { -1 } else { 1 };
                if should_round_away_from_zero {
                    quotient
                        .checked_add(adjustment)
                        .ok_or(TimeError::Overflow)?
                } else {
                    quotient
                }
            }
        };
        let target_ticks = i64::try_from(ticks).map_err(|_| TimeError::Overflow)?;
        Ok(Timestamp::new(target_ticks, target))
    }

    pub fn to_media_time(self, rounding: Rounding) -> Result<MediaTime, TimeError> {
        let timestamp = self.rescale(TimeBase::NANOSECONDS, rounding)?;
        Ok(MediaTime::from_nanoseconds(timestamp.ticks))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MediaTime {
    nanoseconds: i64,
}

impl MediaTime {
    pub const fn from_nanoseconds(nanoseconds: i64) -> Self {
        Self { nanoseconds }
    }

    pub const fn nanoseconds(self) -> i64 {
        self.nanoseconds
    }

    pub fn checked_delta(self, other: Self) -> Result<MediaDelta, TimeError> {
        let nanoseconds = self
            .nanoseconds
            .checked_sub(other.nanoseconds)
            .ok_or(TimeError::Overflow)?;

        Ok(MediaDelta { nanoseconds })
    }

    pub fn checked_add(self, delta: MediaDelta) -> Result<Self, TimeError> {
        let nanoseconds = self
            .nanoseconds
            .checked_add(delta.nanoseconds())
            .ok_or(TimeError::Overflow)?;

        Ok(Self::from_nanoseconds(nanoseconds))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MediaDelta {
    nanoseconds: i64,
}

impl MediaDelta {
    pub const fn from_nanoseconds(nanoseconds: i64) -> Self {
        Self { nanoseconds }
    }

    pub const fn nanoseconds(self) -> i64 {
        self.nanoseconds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    TowardZero,
    Nearest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpan {
    ticks: u64,
    time_base: TimeBase,
}

impl TimeSpan {
    pub const fn new(ticks: u64, time_base: TimeBase) -> TimeSpan {
        Self { ticks, time_base }
    }

    pub const fn ticks(self) -> u64 {
        self.ticks
    }

    pub const fn time_base(self) -> TimeBase {
        self.time_base
    }

    pub fn to_milliseconds(self) -> Result<i64, TimeError> {
        let numerator = u128::from(self.time_base.numerator);
        let denominator = u128::from(self.time_base.denominator);
        let duration_ms_wide = self.ticks as u128 * numerator * 1000 / denominator;

        i64::try_from(duration_ms_wide).map_err(|_| TimeError::Overflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    InvalidTimeBase,
    Overflow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_common_time_bases_to_nanoseconds() {
        let cases = [
            (45_000, 90_000, 500_000_000),
            (44_100, 44_100, 1_000_000_000),
            (-45_000, 90_000, -500_000_000),
        ];

        for (ticks, frequency, expected_nanoseconds) in cases {
            let timestamp = Timestamp::new(ticks, TimeBase::from_hz(frequency).unwrap());
            let media_time = timestamp.to_media_time(Rounding::TowardZero).unwrap();

            assert_eq!(media_time.nanoseconds(), expected_nanoseconds);
        }
    }

    #[test]
    fn applies_rounding_symmetrically_to_positive_and_negative_timestamps() {
        let source = TimeBase::new(1, 3).unwrap();
        let target = TimeBase::new(1, 2).unwrap();

        for (ticks, rounding, expected_ticks) in [
            (1, Rounding::TowardZero, 0),
            (-1, Rounding::TowardZero, 0),
            (1, Rounding::Nearest, 1),
            (-1, Rounding::Nearest, -1),
        ] {
            let result = Timestamp::new(ticks, source)
                .rescale(target, rounding)
                .unwrap();

            assert_eq!(result.ticks(), expected_ticks);
        }
    }

    #[test]
    fn rejects_invalid_time_bases_and_reports_unrepresentable_results() {
        assert!(matches!(
            TimeBase::new(0, 48_000),
            Err(TimeError::InvalidTimeBase)
        ));
        assert!(matches!(
            TimeBase::new(1, 0),
            Err(TimeError::InvalidTimeBase)
        ));

        let source = TimeBase::new(u32::MAX, 1).unwrap();
        let target = TimeBase::new(1, u32::MAX).unwrap();
        let timestamp = Timestamp::new(i64::MAX, source);

        assert!(matches!(
            timestamp.rescale(target, Rounding::Nearest),
            Err(TimeError::Overflow)
        ));
    }
}
