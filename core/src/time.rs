//! Standard time types used across the waterkit workspace.
//!
//! Every time-bearing API in waterkit uses one of these. Strings, `u64`
//! milliseconds, `u64` nanoseconds — none of those are acceptable in public
//! signatures. Use:
//!
//! - [`Timestamp`] for absolute instants. Use [`Zoned`] when a wall-clock
//!   time with timezone is needed.
//! - [`Duration`] for elapsed time / delta values. Use [`Span`] for
//!   calendar-aware differences (months, years).

use std::collections::{BTreeMap, VecDeque};

#[doc(no_inline)]
pub use core::time::Duration;
#[doc(no_inline)]
pub use jiff::{Span, Timestamp, Zoned};

/// Carries exact timestamps through a clock that only keeps whole microseconds.
///
/// Android's `MediaCodec` stores a presentation timestamp as an `i64` of
/// microseconds, so a container timestamp such as 1/24 s (41 666 666 ns) comes
/// back from the codec truncated to 41 666 µs. [`submit`](Self::submit) records
/// the exact [`Duration`] under the microsecond value the platform will return
/// and [`restore`](Self::restore) maps that value back, so callers never see
/// the truncated timestamp.
///
/// Timestamps that share a microsecond value are restored in submission order.
#[derive(Debug, Default)]
pub struct MicrosecondTimestamps {
    submitted: BTreeMap<u64, VecDeque<Duration>>,
}

/// An error from [`MicrosecondTimestamps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MicrosecondTimestampError {
    /// The timestamp does not fit the platform's microsecond counter.
    #[error("timestamp {0:?} exceeds the platform's microsecond range")]
    OutOfRange(Duration),
    /// The platform returned a timestamp that was never submitted.
    #[error("platform returned timestamp {0} us that was not submitted")]
    Unknown(i64),
}

impl MicrosecondTimestamps {
    /// Creates an empty mapping.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            submitted: BTreeMap::new(),
        }
    }

    /// Records `exact` and returns the microsecond value to hand the platform.
    ///
    /// # Errors
    ///
    /// Returns [`MicrosecondTimestampError::OutOfRange`] when the value does
    /// not fit a non-negative `i64` of microseconds.
    pub fn submit(&mut self, exact: Duration) -> Result<i64, MicrosecondTimestampError> {
        let micros = i64::try_from(exact.as_micros())
            .map_err(|_| MicrosecondTimestampError::OutOfRange(exact))?;
        self.submitted
            .entry(micros.cast_unsigned())
            .or_default()
            .push_back(exact);
        Ok(micros)
    }

    /// Returns the exact timestamp submitted as `micros`.
    ///
    /// # Errors
    ///
    /// Returns [`MicrosecondTimestampError::Unknown`] when no pending
    /// submission carries that microsecond value.
    pub fn restore(&mut self, micros: i64) -> Result<Duration, MicrosecondTimestampError> {
        let unknown = MicrosecondTimestampError::Unknown(micros);
        let key = u64::try_from(micros).map_err(|_| unknown)?;
        let queue = self.submitted.get_mut(&key).ok_or(unknown)?;
        let exact = queue.pop_front().ok_or(unknown)?;
        if queue.is_empty() {
            self.submitted.remove(&key);
        }
        Ok(exact)
    }

    /// Forgets every pending submission, as after a codec flush.
    pub fn clear(&mut self) {
        self.submitted.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1/24 s is not a whole number of microseconds.
    const FRAME_24: Duration = Duration::from_nanos(41_666_666);

    #[test]
    fn exact_timestamp_survives_the_microsecond_round_trip() {
        let mut timestamps = MicrosecondTimestamps::new();
        let mut submitted = BTreeMap::new();
        for frame in 0..48u32 {
            let exact = FRAME_24 * frame;
            let micros = timestamps.submit(exact).unwrap();
            submitted.insert(exact, frame);
            // What MediaCodec hands back.
            let restored = timestamps.restore(micros).unwrap();
            assert_eq!(restored, exact);
            assert!(
                submitted.contains_key(&restored),
                "exact-keyed lookup must find {restored:?}"
            );
        }
    }

    #[test]
    fn reordered_output_restores_each_timestamp() {
        let mut timestamps = MicrosecondTimestamps::new();
        let a = timestamps.submit(FRAME_24).unwrap();
        let b = timestamps.submit(FRAME_24 * 2).unwrap();
        assert_eq!(timestamps.restore(b).unwrap(), FRAME_24 * 2);
        assert_eq!(timestamps.restore(a).unwrap(), FRAME_24);
    }

    #[test]
    fn unsubmitted_timestamp_is_an_error() {
        let mut timestamps = MicrosecondTimestamps::new();
        assert_eq!(
            timestamps.restore(41_666),
            Err(MicrosecondTimestampError::Unknown(41_666))
        );
        let micros = timestamps.submit(FRAME_24).unwrap();
        timestamps.restore(micros).unwrap();
        assert!(timestamps.restore(micros).is_err());
    }

    #[test]
    fn clear_drops_pending_timestamps() {
        let mut timestamps = MicrosecondTimestamps::new();
        let micros = timestamps.submit(FRAME_24).unwrap();
        timestamps.clear();
        assert!(timestamps.restore(micros).is_err());
    }
}
