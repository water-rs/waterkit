//! Selectable presentation video-track metadata.

use std::num::NonZeroU64;

/// One user-selectable encoded video representation in stable quality order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectableVideoTrack {
    id: String,
    label: String,
    bandwidth: NonZeroU64,
    dimensions: Option<(u32, u32)>,
    codecs: Vec<String>,
    hdr: bool,
}

impl SelectableVideoTrack {
    /// Creates one selectable video representation.
    ///
    /// # Panics
    ///
    /// Panics when the manifest representation identity is empty or whitespace-only.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        bandwidth: NonZeroU64,
        dimensions: Option<(u32, u32)>,
        codecs: Vec<String>,
        hdr: bool,
    ) -> Self {
        let id = id.into();
        assert!(
            !id.trim().is_empty(),
            "selectable video-track identity must not be empty"
        );
        let label = dimensions.map_or_else(
            || format_bandwidth(bandwidth),
            |(width, height)| format!("{width}×{height} · {}", format_bandwidth(bandwidth)),
        );
        Self {
            id,
            label,
            bandwidth,
            dimensions,
            codecs,
            hdr,
        }
    }

    /// Returns the manifest representation identity.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns a concise quality label derived from dimensions and bitrate.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns declared encoded bandwidth in bits per second.
    #[must_use]
    pub const fn bandwidth(&self) -> NonZeroU64 {
        self.bandwidth
    }

    /// Returns encoded dimensions when declared by the manifest.
    #[must_use]
    pub const fn dimensions(&self) -> Option<(u32, u32)> {
        self.dimensions
    }

    /// Returns RFC 6381 codec identifiers.
    #[must_use]
    pub fn codecs(&self) -> &[String] {
        &self.codecs
    }

    /// Returns whether the representation explicitly signals HDR.
    #[must_use]
    pub const fn is_hdr(&self) -> bool {
        self.hdr
    }
}

fn format_bandwidth(bits_per_second: NonZeroU64) -> String {
    let whole_megabits = bits_per_second.get() / 1_000_000;
    let decimal_megabits = (bits_per_second.get() % 1_000_000) / 100_000;
    format!("{whole_megabits}.{decimal_megabits} Mbps")
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::SelectableVideoTrack;

    #[test]
    fn quality_label_preserves_dimensions_and_declared_bandwidth() {
        let track = SelectableVideoTrack::new(
            "main-2160p",
            NonZeroU64::new(15_600_000).expect("test bandwidth must be non-zero"),
            Some((3840, 2160)),
            vec![String::from("hvc1.2.4.L153.B0")],
            true,
        );
        assert_eq!(track.label(), "3840×2160 · 15.6 Mbps");
        assert!(track.is_hdr());
    }
}
