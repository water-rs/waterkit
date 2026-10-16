use keyboard_types::Key;

use crate::CommandId;

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
    /// Two [`Command`](crate::Command)s in one bar carry the same
    /// [`CommandId`]. Ids are caller-supplied and must be unique per bar.
    #[error("duplicate CommandId {0}")]
    DuplicateCommandId(CommandId),
    /// More than one [`Submenu`](crate::Submenu) in the bar is marked
    /// [`Submenu::windows_menu`](crate::Submenu::windows_menu): the
    /// application has a single windows menu.
    #[error("more than one submenu is marked as the windows menu")]
    DuplicateWindowsMenu,
    /// The bar holds more items than the platform can address.
    ///
    /// On Windows, `WM_COMMAND` identifies a menu item through a 16-bit id,
    /// so one bar holds at most 65,535 commands.
    #[error("menu bar holds at most 65,535 items")]
    ItemLimitExceeded,
    /// A [`StandardItem`](crate::StandardItem) was used on a platform without
    /// the macOS application menu. Standard items are macOS-only.
    #[error("standard menu items exist only on macOS")]
    StandardItemUnsupported,
    /// The platform call itself failed; the message names the failing call.
    #[error("platform error: {0}")]
    Platform(String),
}
