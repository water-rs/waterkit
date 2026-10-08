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
//! - **Other platforms:** the crate compiles to nothing — `MenuBar`, the
//!   model types and `MenuError` are all macOS/Windows-only, so code that
//!   tries to use them fails to compile.
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
//! On Windows the accelerator is rendered as text next to the item title, and
//! [`MenuBar::accelerator_table`] exposes the matching `HACCEL` for hosts
//! that call `TranslateAcceleratorW` in their message pump. Hosts that never
//! translate accelerators dispatch the chord themselves.
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod error;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod model;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod sys;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::marker::PhantomData;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::rc::Rc;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use error::MenuError;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use keyboard_types::{Key, NamedKey};
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use model::{Command, CommandId, Entry, Modifiers, Shortcut, StandardItem, Submenu};
#[cfg(target_os = "windows")]
pub use sys::Attachment;

/// A menu bar description, built once and installed on the platform.
///
/// On macOS the bar owns the `NSMenu` tree; on Windows it owns the `HMENU`.
/// The bar is `!Send` and must live on the main thread for as long as it is
/// installed or attached.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug)]
pub struct MenuBar {
    inner: sys::MenuBarInner,
    events: async_channel::Receiver<CommandId>,
    // The bar owns native, main-thread-only objects on every platform; it
    // must not move to (or be shared with) another thread.
    _not_send: PhantomData<Rc<()>>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl MenuBar {
    /// Builds the native menu tree.
    ///
    /// # Errors
    ///
    /// Fails with [`MenuError::UnmappableKey`] on a shortcut the platform
    /// cannot use as a menu key equivalent, with
    /// [`MenuError::DuplicateCommandId`] when two commands carry the same id,
    /// with [`MenuError::DuplicateWindowsMenu`] when more than one submenu is
    /// marked [`Submenu::windows_menu`], with
    /// [`MenuError::ItemLimitExceeded`] when the bar holds more items
    /// than the platform can address, and with
    /// [`MenuError::StandardItemUnsupported`] when a [`StandardItem`] or a
    /// [`Submenu::windows_menu`] mark appears outside macOS.
    ///
    /// # Panics
    ///
    /// On macOS, panics when called off the main thread: `AppKit` objects must
    /// be created on the main thread.
    pub fn new(menus: impl IntoIterator<Item = Submenu>) -> Result<Self, MenuError> {
        let menus: Vec<Submenu> = menus.into_iter().collect();
        if marked_windows_menus(&menus) > 1 {
            return Err(MenuError::DuplicateWindowsMenu);
        }
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
    /// The guard borrows the bar, so the bar outlives it. It must drop before
    /// `hwnd` is destroyed.
    ///
    /// # Errors
    ///
    /// Fails when `hwnd` is not a valid window or belongs to another thread.
    pub fn attach(
        &self,
        hwnd: windows::Win32::Foundation::HWND,
    ) -> Result<Attachment<'_>, MenuError> {
        self.inner.attach(hwnd)
    }

    /// The `HACCEL` translating this bar's shortcuts, or `None` when the bar
    /// has none.
    ///
    /// A host whose message pump calls
    /// `TranslateAcceleratorW(hwnd, table, msg)` receives chord activations as
    /// `WM_COMMAND` notifications, which the attachment reports as the
    /// command's [`CommandId`].
    #[must_use]
    pub const fn accelerator_table(
        &self,
    ) -> Option<windows::Win32::UI::WindowsAndMessaging::HACCEL> {
        self.inner.accelerator_table()
    }
}

/// The number of submenus marked [`Submenu::windows_menu`] in `menus`, at
/// any depth — the bar may mark at most one.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn marked_windows_menus(menus: &[Submenu]) -> usize {
    fn marked(submenu: &Submenu) -> usize {
        usize::from(submenu.windows_menu)
            + submenu
                .entries
                .iter()
                .map(|entry| match entry {
                    Entry::Submenu(nested) => marked(nested),
                    _ => 0,
                })
                .sum::<usize>()
    }
    menus.iter().map(marked).sum()
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use crate::{MenuBar, MenuError, Submenu};

    /// `MenuBar::new` rejects a second `windows_menu` mark before any
    /// platform object is built, so the check needs no main thread.
    #[test]
    fn duplicate_windows_menu_mark_is_rejected() {
        assert!(matches!(
            MenuBar::new([
                Submenu::new("Window").windows_menu(),
                Submenu::new("Other").windows_menu(),
            ]),
            Err(MenuError::DuplicateWindowsMenu)
        ));
    }
}
