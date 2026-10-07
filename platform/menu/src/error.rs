use keyboard_types::Key;

/// Failure while building or attaching a [`MenuBar`](crate::MenuBar).
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum MenuError {
    /// A shortcut key has no menu key equivalent on this platform.
    ///
    /// Returned by [`MenuBar::new`](crate::MenuBar::new); the key is reported,
    /// never silently dropped.
    #[error("key {0:?} has no menu key equivalent on this platform")]
    UnmappableKey(Key),
    /// A [`StandardItem`](crate::StandardItem) was used on a platform without
    /// the macOS application menu. Standard items are macOS-only.
    #[error("standard menu items exist only on macOS")]
    StandardItemUnsupported,
    /// The platform has no application menu-bar object at all.
    ///
    /// `waterkit-menu` provides a menu bar on macOS and Windows only.
    #[error("no application menu bar on this platform")]
    Unsupported,
    /// The platform call itself failed; the message names the failing call.
    #[error("platform error: {0}")]
    Platform(String),
}
