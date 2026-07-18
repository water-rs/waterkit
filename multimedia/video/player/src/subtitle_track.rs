//! Selectable presentation subtitle-track metadata.

/// One user-selectable subtitle track in stable presentation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectableSubtitleTrack {
    label: String,
    language: Option<String>,
    roles: Vec<String>,
    forced: bool,
}

impl SelectableSubtitleTrack {
    /// Creates one selectable subtitle-track descriptor.
    ///
    /// # Panics
    ///
    /// Panics when the display label is empty or whitespace-only.
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        language: Option<String>,
        roles: Vec<String>,
        forced: bool,
    ) -> Self {
        let label = label.into();
        assert!(
            !label.trim().is_empty(),
            "selectable subtitle-track label must not be empty"
        );
        Self {
            label,
            language,
            roles,
            forced,
        }
    }

    /// Returns the human-readable track label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the BCP-47 or container language tag when declared.
    #[must_use]
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Returns semantic roles such as `main`, `caption`, or `forced-subtitle`.
    #[must_use]
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Returns whether the track carries forced narrative text.
    #[must_use]
    pub const fn is_forced(&self) -> bool {
        self.forced
    }
}
