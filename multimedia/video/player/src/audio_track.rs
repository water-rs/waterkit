//! Selectable presentation audio-track metadata.

/// One user-selectable audio track in stable presentation order.
///
/// The descriptor is independent of any manifest or container format. Its
/// zero-based position in the returned track list is the value accepted by
/// playback audio-track selection APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectableAudioTrack {
    label: String,
    language: Option<String>,
    roles: Vec<String>,
}

impl SelectableAudioTrack {
    /// Creates one selectable audio-track descriptor.
    ///
    /// # Panics
    ///
    /// Panics when the display label is empty or whitespace-only.
    #[must_use]
    pub fn new(label: impl Into<String>, language: Option<String>, roles: Vec<String>) -> Self {
        let label = label.into();
        assert!(
            !label.trim().is_empty(),
            "selectable audio-track label must not be empty"
        );
        Self {
            label,
            language,
            roles,
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

    /// Returns semantic roles such as `main`, `alternate`, or `commentary`.
    #[must_use]
    pub fn roles(&self) -> &[String] {
        &self.roles
    }
}
