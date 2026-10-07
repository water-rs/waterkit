//! Native application menu bar for macOS and Windows.
//!
//! [`MenuBar`] projects a platform-neutral menu description
//! ([`Submenu`]s of [`Entry`] items) onto the platform's own menu-bar object:
//!
//! - **macOS:** an `NSMenu` installed as `NSApp.mainMenu`, built through
//!   `objc2-app-kit`. [`MenuBar::install`] replaces the current bar.
//! - **Windows:** a Win32 `HMENU` attached to an `HWND` through the `windows`
//!   crate. [`MenuBar::attach`] subclasses the window so `WM_COMMAND`
//!   activations reach the bar's own event stream; dropping the returned
//!   [`Attachment`] detaches the bar.
//! - **Other platforms:** no application menu-bar object exists, so this crate
//!   does not provide one: the model types still compile, but
//!   [`MenuBar::new`] returns [`MenuError::Unsupported`].
//!
//! # Shortcuts
//!
//! A [`Shortcut`] is a W3C `KeyboardEvent.key` ([`keyboard_types::Key`]) plus
//! [`Modifiers`]. `Modifiers::COMMAND` is the platform's menu accelerator
//! modifier: ⌘ on macOS, Ctrl on Windows. Every key is mapped to the
//! platform's real key equivalent — on macOS, `Delete` is
//! `NSDeleteFunctionKey` (U+F728) and `Backspace` is `NSDeleteCharacter`
//! (U+007F) — and a key with no mapping fails [`MenuBar::new`] with
//! [`MenuError::UnmappableKey`] instead of being silently dropped.
//!
//! On Windows the accelerator is rendered as text next to the item title, but
//! chords do not fire by themselves: message pumps such as winit's never call
//! `TranslateAcceleratorW`, so the host dispatches the chord itself.
//!
//! # Activation
//!
//! Choosing an item, or pressing its accelerator, yields the item's
//! [`CommandId`] on [`MenuBar::events`]. Activation never goes through a
//! process-global channel: each bar owns its stream.
//!
//! # Threading
//!
//! All native objects stay on the main thread. `MenuBar` is `!Send`; the
//! `events()` stream is `Send` so activations can be consumed anywhere.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod error;
mod model;
mod sys;

use std::marker::PhantomData;
use std::rc::Rc;

pub use error::MenuError;
pub use keyboard_types::{Key, NamedKey};
pub use model::{Command, CommandId, Entry, Modifiers, Shortcut, StandardItem, Submenu};
#[cfg(target_os = "windows")]
pub use sys::Attachment;

/// A menu bar description, built once and installed on the platform.
///
/// On macOS the bar owns the `NSMenu` tree; on Windows it owns the `HMENU`.
/// The bar is `!Send` and must live on the main thread for as long as it is
/// installed or attached.
#[derive(Debug)]
pub struct MenuBar {
    inner: sys::MenuBarInner,
    events: async_channel::Receiver<CommandId>,
    // The bar owns native, main-thread-only objects on every platform; it
    // must not move to (or be shared with) another thread.
    _not_send: PhantomData<Rc<()>>,
}

impl MenuBar {
    /// Builds the native menu tree.
    ///
    /// # Errors
    ///
    /// Fails with [`MenuError::UnmappableKey`] on a shortcut the platform
    /// cannot use as a menu key equivalent, with
    /// [`MenuError::StandardItemUnsupported`] when a [`StandardItem`] appears
    /// outside macOS, and with [`MenuError::Unsupported`] on platforms that
    /// have no application menu-bar object.
    ///
    /// # Panics
    ///
    /// On macOS, panics when called off the main thread: `AppKit` objects must
    /// be created on the main thread.
    pub fn new(menus: impl IntoIterator<Item = Submenu>) -> Result<Self, MenuError> {
        let (sender, events) = async_channel::unbounded();
        let inner = sys::MenuBarInner::new(menus, &sender)?;
        Ok(Self {
            inner,
            events,
            _not_send: PhantomData,
        })
    }

    /// Every activation, by click or by accelerator, of a [`Command`] in this
    /// bar.
    ///
    /// The stream is `Send` and may be polled on any thread. It ends when the
    /// bar is dropped.
    pub fn events(&self) -> impl futures::Stream<Item = CommandId> + Send + 'static {
        self.events.clone()
    }

    /// Enables or disables the command `id`.
    ///
    /// # Panics
    ///
    /// Panics when `id` is not in this bar; that is a caller bug.
    pub fn set_enabled(&self, id: CommandId, enabled: bool) {
        self.inner.set_enabled(id, enabled);
    }

    /// Checks or unchecks the command `id`.
    ///
    /// # Panics
    ///
    /// Panics when `id` is not in this bar; that is a caller bug.
    pub fn set_checked(&self, id: CommandId, checked: bool) {
        self.inner.set_checked(id, checked);
    }
}

#[cfg(target_os = "macos")]
impl MenuBar {
    /// Installs the bar as `NSApp.mainMenu`, replacing the previous one.
    ///
    /// The bar must stay alive for as long as it is installed.
    pub fn install(&self, mtm: objc2::MainThreadMarker) {
        self.inner.install(mtm);
    }
}

#[cfg(target_os = "windows")]
impl MenuBar {
    /// Attaches the bar to a window; dropping the returned guard detaches it.
    ///
    /// The guard must drop before `hwnd` is destroyed.
    ///
    /// # Errors
    ///
    /// Fails when `hwnd` is not a valid window or belongs to another thread.
    pub fn attach(&self, hwnd: windows::Win32::Foundation::HWND) -> Result<Attachment, MenuError> {
        self.inner.attach(hwnd)
    }
}
