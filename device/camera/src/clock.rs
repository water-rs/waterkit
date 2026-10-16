//! The capture clock that turns platform capture times into frame
//! timestamps.

use std::time::Duration;

/// One reading of a camera's capture clock.
pub trait CaptureTime: Copy + Send + Sync {
    /// How long after `earlier` this reading is, or `None` when it precedes
    /// it.
    fn since(self, earlier: Self) -> Option<Duration>;
}

impl CaptureTime for Duration {
    fn since(self, earlier: Self) -> Option<Duration> {
        self.checked_sub(earlier)
    }
}

impl CaptureTime for std::time::Instant {
    fn since(self, earlier: Self) -> Option<Duration> {
        self.checked_duration_since(earlier)
    }
}

/// Turns a camera's capture-clock readings into frame timestamps: the first
/// frame the camera delivered after it opened reads zero, every later one
/// the capture time since it.
pub struct StreamClock<T> {
    first: std::sync::OnceLock<T>,
}

impl<T: CaptureTime> StreamClock<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            first: std::sync::OnceLock::new(),
        }
    }

    /// The timestamp of a frame captured at `capture_time`.
    ///
    /// # Panics
    ///
    /// Panics when `capture_time` precedes the first frame's: capture clocks
    /// are monotonic, so that is a platform defect.
    pub fn timestamp(&self, capture_time: T) -> Duration {
        let first = *self.first.get_or_init(|| capture_time);
        capture_time
            .since(first)
            .expect("a camera's capture clock is monotonic, so no frame precedes its first")
    }
}

#[cfg(test)]
mod tests {
    use super::StreamClock;
    use std::time::{Duration, Instant};

    #[test]
    fn the_first_capture_reading_is_zero() {
        let clock = StreamClock::new();
        assert_eq!(clock.timestamp(Duration::from_secs(42)), Duration::ZERO);
    }

    #[test]
    fn later_readings_measure_from_the_first_frame() {
        let clock = StreamClock::new();
        clock.timestamp(Duration::from_secs(5));
        assert_eq!(
            clock.timestamp(Duration::from_millis(5_033)),
            Duration::from_millis(33)
        );
    }

    #[test]
    fn instants_measure_from_the_first_frame() {
        let clock = StreamClock::new();
        let first = Instant::now();
        assert_eq!(clock.timestamp(first), Duration::ZERO);
        assert_eq!(
            clock.timestamp(first + Duration::from_millis(50)),
            Duration::from_millis(50)
        );
    }

    #[test]
    #[should_panic(expected = "a camera's capture clock is monotonic")]
    fn a_reading_before_the_first_frame_panics() {
        let clock = StreamClock::new();
        clock.timestamp(Duration::from_secs(5));
        clock.timestamp(Duration::from_secs(4));
    }
}
