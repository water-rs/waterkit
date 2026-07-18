//! Live-presentation seek window and recommended playback position.

use std::time::Duration;

use waterkit_video_core::Error;

/// Manifest-authorized playback-rate bounds for correcting live latency.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivePlaybackRateRange {
    minimum: f32,
    maximum: f32,
}

impl LivePlaybackRateRange {
    /// Normal playback without live-latency correction.
    pub const NORMAL: Self = Self {
        minimum: 1.0,
        maximum: 1.0,
    };

    /// Creates validated correction bounds that contain normal playback speed.
    ///
    /// # Errors
    ///
    /// Returns an error unless both bounds are finite, positive, ordered, and
    /// contain `1.0`.
    pub fn new(minimum: f32, maximum: f32) -> Result<Self, Error> {
        if !minimum.is_finite()
            || !maximum.is_finite()
            || minimum <= 0.0
            || minimum > 1.0
            || maximum < 1.0
            || minimum > maximum
        {
            return Err(Error::Streaming(format!(
                "live playback-rate range {minimum}..={maximum} must be finite, positive, ordered, and contain 1.0"
            )));
        }
        Ok(Self { minimum, maximum })
    }

    /// Returns the slowest permitted playback multiplier.
    #[must_use]
    pub const fn minimum(self) -> f32 {
        self.minimum
    }

    /// Returns the fastest permitted playback multiplier.
    #[must_use]
    pub const fn maximum(self) -> f32 {
        self.maximum
    }
}

/// One coherent snapshot of a live presentation timeline.
///
/// All positions use the presentation's monotonic media-time origin. The
/// seekable range is inclusive, while [`Self::target_position`] is the
/// manifest-recommended position used by a "go live" command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveWindow {
    seekable_start: Duration,
    seekable_end: Duration,
    live_edge: Duration,
    target_position: Duration,
}

impl LiveWindow {
    /// Creates a validated live-window snapshot.
    ///
    /// # Panics
    ///
    /// Panics unless `seekable_start <= target_position <= seekable_end <= live_edge`.
    #[must_use]
    pub fn new(
        seekable_start: Duration,
        seekable_end: Duration,
        live_edge: Duration,
        target_position: Duration,
    ) -> Self {
        assert!(
            seekable_start <= target_position,
            "live target position must not precede the seekable window"
        );
        assert!(
            target_position <= seekable_end,
            "live target position must not exceed the seekable window"
        );
        assert!(
            seekable_end <= live_edge,
            "live edge must not precede the seekable window"
        );
        Self {
            seekable_start,
            seekable_end,
            live_edge,
            target_position,
        }
    }

    /// Returns the earliest currently seekable presentation position.
    #[must_use]
    pub const fn seekable_start(self) -> Duration {
        self.seekable_start
    }

    /// Returns the latest currently seekable presentation position.
    #[must_use]
    pub const fn seekable_end(self) -> Duration {
        self.seekable_end
    }

    /// Returns the presentation's current live edge.
    #[must_use]
    pub const fn live_edge(self) -> Duration {
        self.live_edge
    }

    /// Returns the manifest-recommended playback position near the live edge.
    #[must_use]
    pub const fn target_position(self) -> Duration {
        self.target_position
    }

    /// Returns the recommended offset behind the live edge.
    #[must_use]
    pub const fn target_live_offset(self) -> Duration {
        self.live_edge.saturating_sub(self.target_position)
    }

    /// Returns whether `position` is inside the current seekable window.
    #[must_use]
    pub fn contains(self, position: Duration) -> bool {
        position >= self.seekable_start && position <= self.seekable_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_live_offset_is_derived_from_one_atomic_snapshot() {
        let window = LiveWindow::new(
            Duration::from_secs(30),
            Duration::from_secs(90),
            Duration::from_secs(92),
            Duration::from_secs(86),
        );

        assert_eq!(window.target_live_offset(), Duration::from_secs(6));
        assert!(window.contains(Duration::from_secs(30)));
        assert!(window.contains(Duration::from_secs(90)));
        assert!(!window.contains(Duration::from_secs(29)));
    }

    #[test]
    fn live_playback_rate_range_must_contain_normal_speed() {
        let range = LivePlaybackRateRange::new(0.95, 1.05)
            .expect("ordinary live correction bounds must be valid");
        assert!((range.minimum() - 0.95).abs() <= f32::EPSILON);
        assert!((range.maximum() - 1.05).abs() <= f32::EPSILON);
        assert!(LivePlaybackRateRange::new(1.01, 1.05).is_err());
        assert!(LivePlaybackRateRange::new(0.95, 0.99).is_err());
    }
}
