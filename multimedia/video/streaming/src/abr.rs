use std::{num::NonZeroU64, time::Duration};

use num_traits::ToPrimitive as _;
use url::Url;
use waterkit_video_core::Error;

/// One bitrate/resolution choice that an adaptive presentation can select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamVariant {
    /// Manifest URL for this rendition.
    pub url: Url,
    /// Peak advertised bandwidth in bits per second.
    pub peak_bandwidth: NonZeroU64,
    /// Average advertised bandwidth in bits per second when supplied.
    pub average_bandwidth: Option<NonZeroU64>,
    /// Encoded dimensions when supplied by the manifest.
    pub dimensions: Option<(u32, u32)>,
    /// RFC 6381 codec identifiers.
    pub codecs: Vec<String>,
    /// Alternate audio rendition group referenced by this variant.
    pub audio_group_id: Option<String>,
    /// Alternate video rendition group referenced by this variant.
    pub video_group_id: Option<String>,
    /// Subtitle rendition group referenced by this variant.
    pub subtitle_group_id: Option<String>,
    /// Closed-caption rendition group referenced by this variant.
    pub closed_caption_group_id: Option<String>,
}

/// Representation properties consumed by the protocol-independent adaptive selector.
pub trait AdaptiveVariant {
    /// Returns the bandwidth used for adaptive selection.
    fn selection_bandwidth(&self) -> NonZeroU64;

    /// Returns encoded dimensions when the representation carries video.
    fn dimensions(&self) -> Option<(u32, u32)>;
}

impl StreamVariant {
    /// Returns the bandwidth used for adaptive selection.
    #[must_use]
    pub const fn selection_bandwidth(&self) -> NonZeroU64 {
        match self.average_bandwidth {
            Some(average) => average,
            None => self.peak_bandwidth,
        }
    }
}

impl AdaptiveVariant for StreamVariant {
    fn selection_bandwidth(&self) -> NonZeroU64 {
        self.selection_bandwidth()
    }

    fn dimensions(&self) -> Option<(u32, u32)> {
        self.dimensions
    }
}

/// Deterministic dual-EWMA throughput estimator.
///
/// Callers provide transfer durations explicitly, so tests and offline
/// simulations never depend on wall-clock sampling.
#[derive(Debug, Clone)]
pub struct BandwidthEstimator {
    fast_bits_per_second: f64,
    slow_bits_per_second: f64,
    fast_half_life: Duration,
    slow_half_life: Duration,
}

impl BandwidthEstimator {
    /// Creates an estimator with a conservative initial bandwidth.
    #[must_use]
    pub const fn new(initial_bits_per_second: NonZeroU64) -> Self {
        let initial = bandwidth_to_f64(initial_bits_per_second.get());
        Self {
            fast_bits_per_second: initial,
            slow_bits_per_second: initial,
            fast_half_life: Duration::from_secs(2),
            slow_half_life: Duration::from_secs(5),
        }
    }

    /// Adds one completed network transfer sample.
    ///
    /// # Errors
    ///
    /// Returns an error when the elapsed duration is zero.
    pub fn add_sample(
        &mut self,
        transferred_bytes: NonZeroU64,
        elapsed: Duration,
    ) -> Result<(), Error> {
        if elapsed.is_zero() {
            return Err(Error::Streaming(String::from(
                "bandwidth sample duration must be greater than zero",
            )));
        }
        let sample_bits_per_second =
            bandwidth_to_f64(transferred_bytes.get()) * 8.0 / elapsed.as_secs_f64();
        self.fast_bits_per_second = update_ewma(
            self.fast_bits_per_second,
            sample_bits_per_second,
            elapsed,
            self.fast_half_life,
        );
        self.slow_bits_per_second = update_ewma(
            self.slow_bits_per_second,
            sample_bits_per_second,
            elapsed,
            self.slow_half_life,
        );
        Ok(())
    }

    /// Returns the conservative lower estimate from the fast and slow windows.
    #[must_use]
    pub fn estimate(&self) -> NonZeroU64 {
        let estimate = self
            .fast_bits_per_second
            .min(self.slow_bits_per_second)
            .round()
            .max(1.0)
            .to_u64()
            .unwrap_or(u64::MAX);
        NonZeroU64::MIN.saturating_add(estimate.saturating_sub(1))
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "EWMA bandwidth estimation intentionally operates in floating point"
)]
const fn bandwidth_to_f64(value: u64) -> f64 {
    value as f64
}

fn update_ewma(current: f64, sample: f64, elapsed: Duration, half_life: Duration) -> f64 {
    let retained = 0.5_f64.powf(elapsed.as_secs_f64() / half_life.as_secs_f64());
    current.mul_add(retained, sample * (1.0 - retained))
}

/// Buffer and safety policy for adaptive variant switching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveSelectionPolicy {
    bandwidth_fraction_per_mille: u16,
    minimum_buffer_for_upgrade: Duration,
    emergency_buffer: Duration,
}

impl AdaptiveSelectionPolicy {
    /// Creates a validated adaptive switching policy.
    ///
    /// # Errors
    ///
    /// Returns an error unless the bandwidth fraction is in `1..=1000` and
    /// the emergency threshold is below the upgrade threshold.
    pub fn new(
        bandwidth_fraction_per_mille: u16,
        minimum_buffer_for_upgrade: Duration,
        emergency_buffer: Duration,
    ) -> Result<Self, Error> {
        if !(1..=1_000).contains(&bandwidth_fraction_per_mille) {
            return Err(Error::Streaming(format!(
                "adaptive bandwidth fraction must be in 1..=1000, got {bandwidth_fraction_per_mille}"
            )));
        }
        if emergency_buffer >= minimum_buffer_for_upgrade {
            return Err(Error::Streaming(String::from(
                "adaptive emergency buffer must be below the upgrade threshold",
            )));
        }
        Ok(Self {
            bandwidth_fraction_per_mille,
            minimum_buffer_for_upgrade,
            emergency_buffer,
        })
    }

    fn usable_bandwidth(self, estimate: NonZeroU64) -> u64 {
        estimate
            .get()
            .saturating_mul(u64::from(self.bandwidth_fraction_per_mille))
            / 1_000
    }
}

impl Default for AdaptiveSelectionPolicy {
    fn default() -> Self {
        Self {
            bandwidth_fraction_per_mille: 750,
            minimum_buffer_for_upgrade: Duration::from_secs(10),
            emergency_buffer: Duration::from_secs(2),
        }
    }
}

/// Stateful adaptive selector with upgrade hysteresis and emergency downgrade.
#[derive(Debug)]
pub struct AdaptiveTrackSelector<Variant = StreamVariant> {
    variants: Vec<Variant>,
    policy: AdaptiveSelectionPolicy,
    current_index: Option<usize>,
    manual_index: Option<usize>,
}

impl<Variant: AdaptiveVariant> AdaptiveTrackSelector<Variant> {
    /// Creates a selector and sorts variants by selection bandwidth.
    ///
    /// # Errors
    ///
    /// Returns an error when no variants are supplied.
    pub fn new(mut variants: Vec<Variant>, policy: AdaptiveSelectionPolicy) -> Result<Self, Error> {
        if variants.is_empty() {
            return Err(Error::Streaming(String::from(
                "adaptive selection requires at least one variant",
            )));
        }
        variants.sort_by_key(AdaptiveVariant::selection_bandwidth);
        Ok(Self {
            variants,
            policy,
            current_index: None,
            manual_index: None,
        })
    }

    /// Selects a fixed variant in ascending-bandwidth order, or restores ABR.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested index is outside the variant list.
    pub fn set_manual_selection(&mut self, index: Option<usize>) -> Result<(), Error> {
        if let Some(index) = index
            && index >= self.variants.len()
        {
            return Err(Error::Streaming(format!(
                "adaptive variant index {index} is out of range for {} variants",
                self.variants.len(),
            )));
        }
        self.manual_index = index;
        Ok(())
    }

    /// Selects a playable variant for current bandwidth, buffer, and viewport.
    ///
    /// `supports` lets the codec/platform layer reject unsupported codec,
    /// profile, bit-depth, HDR, or DRM combinations without coupling those
    /// capabilities into the streaming crate.
    ///
    /// # Errors
    ///
    /// Returns an error when no supplied variant satisfies the constraints.
    pub fn select(
        &mut self,
        estimate: NonZeroU64,
        buffered: Duration,
        viewport: Option<(u32, u32)>,
        mut supports: impl FnMut(&Variant) -> bool,
    ) -> Result<&Variant, Error> {
        if let Some(index) = self.manual_index {
            let selected = &self.variants[index];
            if !supports(selected) {
                return Err(Error::Unsupported(format!(
                    "manually selected adaptive variant {index} is unsupported by the active codec pipeline",
                )));
            }
            self.current_index = Some(index);
            return Ok(selected);
        }
        let eligible = self
            .variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| supports(variant))
            .filter(|(_, variant)| dimensions_fit(variant.dimensions(), viewport))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let first = *eligible.first().ok_or_else(|| {
            Error::Unsupported(String::from(
                "no adaptive variant satisfies codec and viewport constraints",
            ))
        })?;
        let usable_bandwidth = self.policy.usable_bandwidth(estimate);
        let candidate = eligible
            .iter()
            .copied()
            .take_while(|index| {
                self.variants[*index].selection_bandwidth().get() <= usable_bandwidth
            })
            .last()
            .unwrap_or(first);

        let selected = match self.current_index.filter(|index| eligible.contains(index)) {
            None => candidate,
            Some(current) if candidate > current => {
                if buffered >= self.policy.minimum_buffer_for_upgrade {
                    candidate
                } else {
                    current
                }
            }
            Some(current) if buffered <= self.policy.emergency_buffer => candidate.min(current),
            Some(current)
                if self.variants[current].selection_bandwidth().get() > usable_bandwidth =>
            {
                candidate.min(current)
            }
            Some(current) => current,
        };
        self.current_index = Some(selected);
        Ok(&self.variants[selected])
    }

    /// Returns the currently selected variant.
    #[must_use]
    pub fn current(&self) -> Option<&Variant> {
        self.current_index.map(|index| &self.variants[index])
    }

    /// Returns variants in the same ascending-bandwidth order used by manual selection.
    #[must_use]
    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }

    /// Returns the fixed variant index, or `None` while ABR owns selection.
    #[must_use]
    pub const fn manual_selection(&self) -> Option<usize> {
        self.manual_index
    }
}

const fn dimensions_fit(dimensions: Option<(u32, u32)>, viewport: Option<(u32, u32)>) -> bool {
    match (dimensions, viewport) {
        (Some((width, height)), Some((max_width, max_height))) => {
            width <= max_width && height <= max_height
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, time::Duration};

    use url::Url;

    use super::{
        AdaptiveSelectionPolicy, AdaptiveTrackSelector, BandwidthEstimator, StreamVariant,
    };

    fn variant(name: &str, bandwidth: u64, dimensions: (u32, u32)) -> StreamVariant {
        StreamVariant {
            url: Url::parse(&format!("https://waterui.dev/video/{name}.m3u8"))
                .expect("test URL must be valid"),
            peak_bandwidth: NonZeroU64::new(bandwidth).expect("test bandwidth must be non-zero"),
            average_bandwidth: None,
            dimensions: Some(dimensions),
            codecs: vec![String::from("avc1.640028")],
            audio_group_id: None,
            video_group_id: None,
            subtitle_group_id: None,
            closed_caption_group_id: None,
        }
    }

    #[test]
    fn estimator_reacts_quickly_to_loss_and_recovers_conservatively() {
        let mut estimator = BandwidthEstimator::new(
            NonZeroU64::new(8_000_000).expect("initial bandwidth must be non-zero"),
        );
        estimator
            .add_sample(
                NonZeroU64::new(125_000).expect("sample bytes must be non-zero"),
                Duration::from_secs(1),
            )
            .expect("sample must be valid");
        assert!(estimator.estimate().get() < 8_000_000);

        let after_loss = estimator.estimate();
        estimator
            .add_sample(
                NonZeroU64::new(2_000_000).expect("sample bytes must be non-zero"),
                Duration::from_secs(1),
            )
            .expect("sample must be valid");
        assert!(estimator.estimate() > after_loss);
        assert!(estimator.estimate().get() < 16_000_000);
    }

    #[test]
    fn selector_requires_buffer_before_upgrade_and_downgrades_on_loss() {
        let mut selector = AdaptiveTrackSelector::new(
            vec![
                variant("low", 1_000_000, (640, 360)),
                variant("medium", 3_000_000, (1280, 720)),
                variant("high", 6_000_000, (1920, 1080)),
            ],
            AdaptiveSelectionPolicy::default(),
        )
        .expect("variants must be valid");

        let selected = selector
            .select(
                NonZeroU64::new(2_000_000).expect("estimate must be non-zero"),
                Duration::ZERO,
                Some((1920, 1080)),
                |_| true,
            )
            .expect("initial selection must succeed");
        assert_eq!(selected.peak_bandwidth.get(), 1_000_000);

        let selected = selector
            .select(
                NonZeroU64::new(10_000_000).expect("estimate must be non-zero"),
                Duration::from_secs(3),
                Some((1920, 1080)),
                |_| true,
            )
            .expect("selection must succeed");
        assert_eq!(selected.peak_bandwidth.get(), 1_000_000);

        let selected = selector
            .select(
                NonZeroU64::new(10_000_000).expect("estimate must be non-zero"),
                Duration::from_secs(12),
                Some((1920, 1080)),
                |_| true,
            )
            .expect("selection must succeed");
        assert_eq!(selected.peak_bandwidth.get(), 6_000_000);

        let selected = selector
            .select(
                NonZeroU64::new(2_000_000).expect("estimate must be non-zero"),
                Duration::from_secs(1),
                Some((1920, 1080)),
                |_| true,
            )
            .expect("selection must succeed");
        assert_eq!(selected.peak_bandwidth.get(), 1_000_000);
    }

    #[test]
    fn manual_selection_bypasses_bandwidth_but_preserves_codec_capability_checks() {
        let mut selector = AdaptiveTrackSelector::new(
            vec![
                variant("low", 1_000_000, (640, 360)),
                variant("high", 6_000_000, (3840, 2160)),
            ],
            AdaptiveSelectionPolicy::default(),
        )
        .expect("test selector must be valid");
        selector
            .set_manual_selection(Some(1))
            .expect("manual index must be valid");
        let selected = selector
            .select(
                NonZeroU64::new(500_000).expect("test bandwidth must be non-zero"),
                Duration::ZERO,
                Some((640, 360)),
                |_| true,
            )
            .expect("manual quality must bypass bandwidth and viewport constraints");
        assert_eq!(selected.dimensions, Some((3840, 2160)));
        assert!(
            selector
                .select(
                    NonZeroU64::new(500_000).expect("test bandwidth must be non-zero"),
                    Duration::ZERO,
                    None,
                    |_| false,
                )
                .is_err()
        );
    }
}
