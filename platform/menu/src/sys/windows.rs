//! Windows backend: the bar is an `HMENU` attached to an `HWND`.
//!
//! Activation: `WM_COMMAND` reaches the window procedure, so
//! [`MenuBar::attach`](crate::MenuBar::attach) subclasses the window with
//! `SetWindowSubclass` and observes `WM_COMMAND` for the bar's item ids; the
//! returned [`Attachment`] removes the subclass on drop. There is no
//! process-global channel and no `static` state.
//!
//! Win32 menus have no accelerators of their own: the chord text is rendered
//! as the accelerator portion of the item title, and since hosts like winit
//! never call `TranslateAcceleratorW`, the host dispatches the chord itself.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use async_channel::Sender;
use keyboard_types::{Key, NamedKey};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_ACCEPT, VK_APPS, VK_BACK, VK_BROWSER_BACK, VK_BROWSER_FAVORITES,
    VK_BROWSER_FORWARD, VK_BROWSER_HOME, VK_BROWSER_REFRESH, VK_BROWSER_SEARCH, VK_BROWSER_STOP,
    VK_CAPITAL, VK_CLEAR, VK_CONVERT, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_EXECUTE, VK_F1,
    VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12, VK_F13, VK_F14,
    VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F20, VK_F21, VK_F22, VK_F23, VK_F24, VK_HELP,
    VK_HOME, VK_INSERT, VK_LAUNCH_APP1, VK_LAUNCH_APP2, VK_LAUNCH_MAIL, VK_LAUNCH_MEDIA_SELECT,
    VK_LEFT, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP,
    VK_MODECHANGE, VK_NEXT, VK_NONCONVERT, VK_NUMLOCK, VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4,
    VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA, VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PAUSE,
    VK_PRINT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SCROLL, VK_SELECT, VK_SNAPSHOT, VK_SPACE, VK_TAB,
    VK_UP, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP, VK_ZOOM,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, DestroyMenu, GetMenuItemCount, HMENU, IsWindow,
    MENU_ITEM_STATE, MENUITEMINFOW, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, MFS_CHECKED, MFS_ENABLED, MFS_GRAYED, MFS_UNCHECKED, MIIM_STATE, SetMenu,
    SetMenuItemInfoW, WM_COMMAND,
};
use windows::core::PCWSTR;

use crate::{Command, CommandId, Entry, MenuError, Modifiers, Shortcut, Submenu};

/// `uidsubclass` value identifying this bar's subclass procedure.
const SUBCLASS_ID: usize = 0x574B_4D4E; // 'WKMN'

/// The data a subclass procedure needs to report an activation. Owned by the
/// [`Attachment`]; the window stores a borrowed pointer in `dwRefData`.
#[derive(Debug)]
struct AttachContext {
    sender: Sender<CommandId>,
    /// Win32 menu-item id (the low word of `WM_COMMAND`'s `wParam`) → id.
    commands: HashMap<u32, CommandId>,
}

/// A [`Command`]'s slot in the menu tree: where it lives plus the state the
/// bar last wrote, so `SetMenuItemInfoW` updates can be computed without
/// reading the menu back.
#[derive(Debug)]
struct ItemSlot {
    parent: HMENU,
    position: u32,
    enabled: Cell<bool>,
    checked: Cell<Option<bool>>,
}

impl ItemSlot {
    const fn state(&self) -> MENU_ITEM_STATE {
        let enabled = if self.enabled.get() {
            MFS_ENABLED.0
        } else {
            MFS_GRAYED.0
        };
        let checked = match self.checked.get() {
            Some(true) => MFS_CHECKED.0,
            Some(false) | None => MFS_UNCHECKED.0,
        };
        MENU_ITEM_STATE(enabled | checked)
    }
}

/// The Windows menu bar: owns the `HMENU` tree.
#[derive(Debug)]
pub struct MenuBarInner {
    menu: HMENU,
    items: HashMap<CommandId, ItemSlot>,
    /// Win32 item id → `CommandId`, cloned into the attach context.
    commands: HashMap<u32, CommandId>,
    sender: Sender<CommandId>,
    /// `Some(hwnd)` while an [`Attachment`] is alive.
    attachment: Rc<Cell<Option<HWND>>>,
}

impl MenuBarInner {
    pub(crate) fn new(
        menus: impl IntoIterator<Item = Submenu>,
        sender: &Sender<CommandId>,
    ) -> Result<Self, MenuError> {
        let mut inner = Self {
            menu: unsafe { CreateMenu() }
                .map_err(|error| MenuError::Platform(format!("CreateMenu: {error}")))?,
            items: HashMap::new(),
            commands: HashMap::new(),
            sender: sender.clone(),
            attachment: Rc::new(Cell::new(None)),
        };
        if let Err(error) = inner.build(menus) {
            // A bar that failed to build must not leak its menu tree.
            unsafe {
                let _ = DestroyMenu(inner.menu);
            }
            return Err(error);
        }
        Ok(inner)
    }

    fn build(&mut self, menus: impl IntoIterator<Item = Submenu>) -> Result<(), MenuError> {
        for submenu in menus {
            let popup = unsafe { CreatePopupMenu() }
                .map_err(|error| MenuError::Platform(format!("CreatePopupMenu: {error}")))?;
            self.fill(popup, &submenu.entries)?;
            append_submenu(self.menu, popup, &submenu.title)?;
        }
        Ok(())
    }

    /// Appends `entries` to `parent`.
    fn fill(&mut self, parent: HMENU, entries: &[Entry]) -> Result<(), MenuError> {
        for entry in entries {
            match entry {
                Entry::Command(command) => self.append_command(parent, command)?,
                Entry::Submenu(submenu) => {
                    let popup = unsafe { CreatePopupMenu() }.map_err(|error| {
                        MenuError::Platform(format!("CreatePopupMenu: {error}"))
                    })?;
                    self.fill(popup, &submenu.entries)?;
                    append_submenu(parent, popup, &submenu.title)?;
                }
                Entry::Separator => {
                    // SAFETY: `AppendMenuW` copies the (null) string argument.
                    unsafe { AppendMenuW(parent, MF_SEPARATOR, 0, PCWSTR::null()) }
                        .map_err(|error| MenuError::Platform(format!("AppendMenuW: {error}")))?;
                }
                Entry::Standard(_) => return Err(MenuError::StandardItemUnsupported),
            }
        }
        Ok(())
    }

    fn append_command(&mut self, parent: HMENU, command: &Command) -> Result<(), MenuError> {
        let mut flags = MF_STRING;
        if !command.enabled {
            flags |= MF_GRAYED;
        }
        if let Some(checked) = command.checked {
            flags |= if checked { MF_CHECKED } else { MF_UNCHECKED };
        }
        let title = match &command.shortcut {
            Some(shortcut) => format!("{}\t{}", command.title, accelerator_text(shortcut)?),
            None => command.title.clone(),
        };
        let position = unsafe { GetMenuItemCount(Some(parent)) };
        if position < 0 {
            return Err(MenuError::Platform("GetMenuItemCount failed".to_owned()));
        }
        let id = command.id().raw();
        // SAFETY: `wide` outlives the call; the menu copies the string.
        let mut wide = to_wide(&title);
        unsafe {
            AppendMenuW(
                parent,
                flags,
                usize::try_from(id).expect("CommandId fits in usize"),
                PCWSTR(wide.as_mut_ptr()),
            )
        }
        .map_err(|error| MenuError::Platform(format!("AppendMenuW: {error}")))?;
        self.items.insert(
            command.id(),
            ItemSlot {
                parent,
                position: u32::try_from(position).expect("position is non-negative"),
                enabled: Cell::new(command.enabled),
                checked: Cell::new(command.checked),
            },
        );
        self.commands.insert(
            u32::try_from(id).expect("CommandId fits in u32"),
            command.id(),
        );
        Ok(())
    }

    /// Attaches the bar to `hwnd` and subclasses the window so `WM_COMMAND`
    /// activations reach the bar's stream.
    pub(crate) fn attach(&self, hwnd: HWND) -> Result<Attachment, MenuError> {
        if self.attachment.get().is_some() {
            return Err(MenuError::Platform(
                "menu bar is already attached to a window".to_owned(),
            ));
        }
        // SAFETY: `IsWindow` accepts any HWND-shaped value.
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            return Err(MenuError::Platform(
                "attach: hwnd is not a window on this thread".to_owned(),
            ));
        }
        let context = Box::into_raw(Box::new(AttachContext {
            sender: self.sender.clone(),
            commands: self.commands.clone(),
        }));
        // SAFETY: `menu_subclass_proc` is a valid SUBCLASSPROC; `context` is
        // kept alive by the returned `Attachment`, which removes the subclass
        // before the box is freed.
        let ok = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(menu_subclass_proc),
                SUBCLASS_ID,
                context as usize,
            )
        };
        if !ok.as_bool() {
            // SAFETY: `context` came from `Box::into_raw` above and is not
            // shared yet, since the subclass was never installed.
            drop(unsafe { Box::from_raw(context) });
            return Err(MenuError::Platform(
                "SetWindowSubclass: hwnd belongs to another thread".to_owned(),
            ));
        }
        // SAFETY: both handles are valid on this thread.
        unsafe { SetMenu(hwnd, Some(self.menu)) }
            .map_err(|error| MenuError::Platform(format!("SetMenu: {error}")))?;
        self.attachment.set(Some(hwnd));
        Ok(Attachment {
            hwnd,
            context: context as usize,
            shared: self.attachment.clone(),
        })
    }

    pub(crate) fn set_enabled(&self, id: CommandId, enabled: bool) {
        self.update_slot(id, |slot| slot.enabled.set(enabled));
    }

    pub(crate) fn set_checked(&self, id: CommandId, checked: bool) {
        self.update_slot(id, |slot| slot.checked.set(Some(checked)));
    }

    /// Mutates `id`'s slot and writes the resulting state to the item.
    fn update_slot(&self, id: CommandId, update: impl Fn(&ItemSlot)) {
        let slot = self
            .items
            .get(&id)
            .unwrap_or_else(|| panic!("waterkit-menu: CommandId {id} is not in this bar"));
        update(slot);
        let info = MENUITEMINFOW {
            cbSize: u32::try_from(size_of::<MENUITEMINFOW>()).expect("size fits in u32"),
            fMask: MIIM_STATE,
            fState: slot.state(),
            ..Default::default()
        };
        // SAFETY: `slot.parent` is a menu this bar built; `info` is a valid
        // input buffer.
        unsafe {
            SetMenuItemInfoW(slot.parent, slot.position, true, &raw const info).unwrap_or_else(
                |error| panic!("waterkit-menu: updating CommandId {id} failed: {error}"),
            );
        }
    }
}

impl Drop for MenuBarInner {
    fn drop(&mut self) {
        // SAFETY: detaches the menu first when a live attachment still points
        // at it, then destroys the menu tree this bar built.
        unsafe {
            if let Some(hwnd) = self.attachment.get() {
                let _ = SetMenu(hwnd, None);
            }
            let _ = DestroyMenu(self.menu);
        }
    }
}

/// The active binding between a [`MenuBar`](crate::MenuBar) and a window.
///
/// Dropping the guard removes the window subclass and detaches the menu. The
/// guard must drop before `hwnd` is destroyed and while the bar is alive.
#[derive(Debug)]
pub struct Attachment {
    hwnd: HWND,
    /// The `Box<AttachContext>` shared with the subclass's `dwRefData`;
    /// freed on drop after the subclass is removed.
    context: usize,
    shared: Rc<Cell<Option<HWND>>>,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        // SAFETY: `menu_subclass_proc`/`SUBCLASS_ID` is this attachment's
        // subclass on `hwnd`; removing it ends all reads of `context`, so the
        // box can be freed. `context` came from `Box::into_raw` in `attach`.
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(menu_subclass_proc), SUBCLASS_ID);
            let _ = SetMenu(self.hwnd, None);
            drop(Box::from_raw(self.context as *mut AttachContext));
        }
        self.shared.set(None);
    }
}

/// The subclass procedure installed by [`MenuBarInner::attach`]. Reports menu
/// activations (`WM_COMMAND` with `HIWORD(wParam) == 0`) for the bar's ids and
/// defers everything else to the previous procedure.
///
/// `dwRefData` is a live `*const AttachContext` from `SetWindowSubclass` until
/// the attachment's `RemoveWindowSubclass`.
unsafe extern "system" fn menu_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    dwrefdata: usize,
) -> LRESULT {
    if msg == WM_COMMAND && hiword(wparam) == 0 {
        // SAFETY: upheld by the attach/remove contract described above.
        let context = unsafe { &*(dwrefdata as *const AttachContext) };
        if let Some(id) = context.commands.get(&loword(wparam)) {
            let _ = context.sender.try_send(*id);
            return LRESULT(0);
        }
    }
    // SAFETY: forwards to the previous subclass/window procedure.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn loword(wparam: WPARAM) -> u32 {
    u32::from(u16::try_from(wparam.0 & 0xFFFF).unwrap_or(0))
}

fn hiword(wparam: WPARAM) -> u16 {
    u16::try_from((wparam.0 >> 16) & 0xFFFF).unwrap_or(0)
}

fn append_submenu(parent: HMENU, popup: HMENU, title: &str) -> Result<(), MenuError> {
    let mut wide = to_wide(title);
    // SAFETY: `wide` outlives the call; the menu copies the string. `popup`
    // becomes owned by `parent`.
    unsafe {
        AppendMenuW(
            parent,
            MF_POPUP,
            popup.0 as usize,
            PCWSTR(wide.as_mut_ptr()),
        )
    }
    .map_err(|error| MenuError::Platform(format!("AppendMenuW: {error}")))
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The "Ctrl+Alt+Shift+X" accelerator text of a shortcut, rendered after a tab
/// in the item title. On Windows `COMMAND` is Ctrl, so `COMMAND` and
/// `CONTROL` print once.
fn accelerator_text(shortcut: &Shortcut) -> Result<String, MenuError> {
    // The virtual-key code is validation only: nothing dispatches it — the
    // text is what a Win32 menu can show.
    let (_virtual_key, key_text) = map_key(&shortcut.key)?;
    let mut text = String::new();
    let modifiers = shortcut.modifiers;
    if modifiers.contains(Modifiers::COMMAND) || modifiers.contains(Modifiers::CONTROL) {
        text.push_str("Ctrl+");
    }
    if modifiers.contains(Modifiers::ALT) {
        text.push_str("Alt+");
    }
    if modifiers.contains(Modifiers::SHIFT) {
        text.push_str("Shift+");
    }
    text.push_str(&key_text);
    Ok(text)
}

/// Maps a W3C `Key` to its virtual-key code and accelerator text.
fn map_key(key: &Key) -> Result<(VIRTUAL_KEY, String), MenuError> {
    let mapped = match key {
        Key::Named(named) => named_virtual_key(*named).map(|(vk, text)| (vk, text.to_owned())),
        Key::Character(character) => character_virtual_key(character),
    };
    mapped.ok_or_else(|| MenuError::UnmappableKey(key.clone()))
}

const fn named_virtual_key(key: NamedKey) -> Option<(VIRTUAL_KEY, &'static str)> {
    let mapped = match key {
        NamedKey::Backspace => (VK_BACK, "Backspace"),
        NamedKey::Tab => (VK_TAB, "Tab"),
        NamedKey::Clear => (VK_CLEAR, "Clear"),
        NamedKey::Enter => (VK_RETURN, "Enter"),
        NamedKey::Pause => (VK_PAUSE, "Pause"),
        NamedKey::CapsLock => (VK_CAPITAL, "CapsLock"),
        NamedKey::Escape => (VK_ESCAPE, "Esc"),
        NamedKey::Convert => (VK_CONVERT, "Convert"),
        NamedKey::NonConvert => (VK_NONCONVERT, "NonConvert"),
        NamedKey::Accept => (VK_ACCEPT, "Accept"),
        NamedKey::ModeChange => (VK_MODECHANGE, "ModeChange"),
        NamedKey::PageUp => (VK_PRIOR, "PgUp"),
        NamedKey::PageDown => (VK_NEXT, "PgDn"),
        NamedKey::End => (VK_END, "End"),
        NamedKey::Home => (VK_HOME, "Home"),
        NamedKey::ArrowLeft => (VK_LEFT, "Left"),
        NamedKey::ArrowUp => (VK_UP, "Up"),
        NamedKey::ArrowRight => (VK_RIGHT, "Right"),
        NamedKey::ArrowDown => (VK_DOWN, "Down"),
        NamedKey::Select => (VK_SELECT, "Select"),
        NamedKey::PrintScreen => (VK_SNAPSHOT, "PrtSc"),
        NamedKey::Execute => (VK_EXECUTE, "Execute"),
        NamedKey::Insert => (VK_INSERT, "Ins"),
        NamedKey::Delete => (VK_DELETE, "Del"),
        NamedKey::Help => (VK_HELP, "Help"),
        NamedKey::ContextMenu => (VK_APPS, "Menu"),
        NamedKey::F1 => (VK_F1, "F1"),
        NamedKey::F2 => (VK_F2, "F2"),
        NamedKey::F3 => (VK_F3, "F3"),
        NamedKey::F4 => (VK_F4, "F4"),
        NamedKey::F5 => (VK_F5, "F5"),
        NamedKey::F6 => (VK_F6, "F6"),
        NamedKey::F7 => (VK_F7, "F7"),
        NamedKey::F8 => (VK_F8, "F8"),
        NamedKey::F9 => (VK_F9, "F9"),
        NamedKey::F10 => (VK_F10, "F10"),
        NamedKey::F11 => (VK_F11, "F11"),
        NamedKey::F12 => (VK_F12, "F12"),
        NamedKey::F13 => (VK_F13, "F13"),
        NamedKey::F14 => (VK_F14, "F14"),
        NamedKey::F15 => (VK_F15, "F15"),
        NamedKey::F16 => (VK_F16, "F16"),
        NamedKey::F17 => (VK_F17, "F17"),
        NamedKey::F18 => (VK_F18, "F18"),
        NamedKey::F19 => (VK_F19, "F19"),
        NamedKey::F20 => (VK_F20, "F20"),
        NamedKey::F21 => (VK_F21, "F21"),
        NamedKey::F22 => (VK_F22, "F22"),
        NamedKey::F23 => (VK_F23, "F23"),
        NamedKey::F24 => (VK_F24, "F24"),
        // Win32 defines no VK_F25 through VK_F35: they are unmappable here.
        NamedKey::NumLock => (VK_NUMLOCK, "NumLock"),
        NamedKey::ScrollLock => (VK_SCROLL, "ScrollLock"),
        NamedKey::Print => (VK_PRINT, "Print"),
        NamedKey::ZoomToggle => (VK_ZOOM, "Zoom"),
        NamedKey::AudioVolumeMute => (VK_VOLUME_MUTE, "Mute"),
        NamedKey::AudioVolumeDown => (VK_VOLUME_DOWN, "VolumeDown"),
        NamedKey::AudioVolumeUp => (VK_VOLUME_UP, "VolumeUp"),
        NamedKey::MediaTrackNext => (VK_MEDIA_NEXT_TRACK, "NextTrack"),
        NamedKey::MediaTrackPrevious => (VK_MEDIA_PREV_TRACK, "PrevTrack"),
        NamedKey::MediaStop => (VK_MEDIA_STOP, "Stop"),
        NamedKey::MediaPlayPause => (VK_MEDIA_PLAY_PAUSE, "PlayPause"),
        NamedKey::LaunchMail => (VK_LAUNCH_MAIL, "Mail"),
        NamedKey::LaunchMediaPlayer => (VK_LAUNCH_MEDIA_SELECT, "MediaPlayer"),
        NamedKey::LaunchApplication1 => (VK_LAUNCH_APP1, "App1"),
        NamedKey::LaunchApplication2 => (VK_LAUNCH_APP2, "App2"),
        NamedKey::BrowserBack => (VK_BROWSER_BACK, "BrowserBack"),
        NamedKey::BrowserFavorites => (VK_BROWSER_FAVORITES, "BrowserFavorites"),
        NamedKey::BrowserForward => (VK_BROWSER_FORWARD, "BrowserForward"),
        NamedKey::BrowserHome => (VK_BROWSER_HOME, "BrowserHome"),
        NamedKey::BrowserRefresh => (VK_BROWSER_REFRESH, "BrowserRefresh"),
        NamedKey::BrowserSearch => (VK_BROWSER_SEARCH, "BrowserSearch"),
        NamedKey::BrowserStop => (VK_BROWSER_STOP, "BrowserStop"),
        _ => return None,
    };
    Some(mapped)
}

/// Maps a `Key::Character` to (VK code, accelerator text) for the ASCII
/// characters that have virtual-key codes, plus the OEM punctuation keys of
/// the US keyboard layout.
fn character_virtual_key(character: &str) -> Option<(VIRTUAL_KEY, String)> {
    let mut chars = character.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_ascii_alphanumeric() {
        // '0'-'9' and 'A'-'Z' are their own virtual-key codes.
        let upper = c.to_ascii_uppercase();
        return Some((VIRTUAL_KEY(upper as u16), upper.to_string()));
    }
    let (vk, text) = match c {
        ' ' => (VK_SPACE, "Space"),
        '\t' => (VK_TAB, "Tab"),
        '\r' | '\n' => (VK_RETURN, "Enter"),
        '`' => (VK_OEM_3, "`"),
        '-' => (VK_OEM_MINUS, "-"),
        '=' => (VK_OEM_PLUS, "="),
        '[' => (VK_OEM_4, "["),
        ']' => (VK_OEM_6, "]"),
        '\\' => (VK_OEM_5, "\\"),
        ';' => (VK_OEM_1, ";"),
        '\'' => (VK_OEM_7, "'"),
        ',' => (VK_OEM_COMMA, ","),
        '.' => (VK_OEM_PERIOD, "."),
        '/' => (VK_OEM_2, "/"),
        _ => return None,
    };
    Some((vk, text.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(key: &Key) -> (u16, String) {
        map_key(key)
            .map(|(vk, text)| (vk.0, text))
            .expect("key should map")
    }

    fn unmappable(key: &Key) {
        assert!(matches!(map_key(key), Err(MenuError::UnmappableKey(_))));
    }

    #[test]
    fn alphanumeric_characters_map_to_their_virtual_key_codes() {
        assert_eq!(
            map(&Key::Character("o".into())),
            (u16::from(b'O'), "O".to_owned())
        );
        assert_eq!(
            map(&Key::Character("Q".into())),
            (u16::from(b'Q'), "Q".to_owned())
        );
        assert_eq!(
            map(&Key::Character("5".into())),
            (u16::from(b'5'), "5".to_owned())
        );
    }

    #[test]
    fn oem_punctuation_maps_to_virtual_key_codes() {
        assert_eq!(
            map(&Key::Character(" ".into())),
            (VK_SPACE.0, "Space".to_owned())
        );
        assert_eq!(
            map(&Key::Character("-".into())),
            (VK_OEM_MINUS.0, "-".to_owned())
        );
        assert_eq!(
            map(&Key::Character("=".into())),
            (VK_OEM_PLUS.0, "=".to_owned())
        );
        assert_eq!(
            map(&Key::Character("[".into())),
            (VK_OEM_4.0, "[".to_owned())
        );
        assert_eq!(
            map(&Key::Character("]".into())),
            (VK_OEM_6.0, "]".to_owned())
        );
        assert_eq!(
            map(&Key::Character("\\".into())),
            (VK_OEM_5.0, "\\".to_owned())
        );
        assert_eq!(
            map(&Key::Character(";".into())),
            (VK_OEM_1.0, ";".to_owned())
        );
        assert_eq!(
            map(&Key::Character("'".into())),
            (VK_OEM_7.0, "'".to_owned())
        );
        assert_eq!(
            map(&Key::Character("`".into())),
            (VK_OEM_3.0, "`".to_owned())
        );
        assert_eq!(
            map(&Key::Character(",".into())),
            (VK_OEM_COMMA.0, ",".to_owned())
        );
        assert_eq!(
            map(&Key::Character(".".into())),
            (VK_OEM_PERIOD.0, ".".to_owned())
        );
        assert_eq!(
            map(&Key::Character("/".into())),
            (VK_OEM_2.0, "/".to_owned())
        );
    }

    #[test]
    fn named_keys_map_to_virtual_key_codes() {
        assert_eq!(
            map(&Key::Named(NamedKey::Backspace)),
            (VK_BACK.0, "Backspace".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::Delete)),
            (VK_DELETE.0, "Del".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::Enter)),
            (VK_RETURN.0, "Enter".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::Escape)),
            (VK_ESCAPE.0, "Esc".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::Tab)),
            (VK_TAB.0, "Tab".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::ArrowUp)),
            (VK_UP.0, "Up".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::ArrowDown)),
            (VK_DOWN.0, "Down".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::ArrowLeft)),
            (VK_LEFT.0, "Left".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::ArrowRight)),
            (VK_RIGHT.0, "Right".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::Home)),
            (VK_HOME.0, "Home".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::End)),
            (VK_END.0, "End".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::PageUp)),
            (VK_PRIOR.0, "PgUp".to_owned())
        );
        assert_eq!(
            map(&Key::Named(NamedKey::PageDown)),
            (VK_NEXT.0, "PgDn".to_owned())
        );
        assert_eq!(map(&Key::Named(NamedKey::F1)), (VK_F1.0, "F1".to_owned()));
        assert_eq!(
            map(&Key::Named(NamedKey::F24)),
            (VK_F24.0, "F24".to_owned())
        );
    }

    #[test]
    fn f25_and_beyond_have_no_virtual_key() {
        // Win32 defines only VK_F1 through VK_F24.
        unmappable(&Key::Named(NamedKey::F25));
        unmappable(&Key::Named(NamedKey::F35));
    }

    #[test]
    fn unmappable_keys_error() {
        unmappable(&Key::Named(NamedKey::Control));
        unmappable(&Key::Named(NamedKey::Shift));
        unmappable(&Key::Named(NamedKey::Alt));
        unmappable(&Key::Named(NamedKey::Meta));
        unmappable(&Key::Named(NamedKey::Dead));
        unmappable(&Key::Character("ab".into()));
        unmappable(&Key::Character("é".into()));
        unmappable(&Key::Character(String::new()));
    }

    #[test]
    fn command_modifier_is_ctrl_on_windows() {
        let text = |modifiers| {
            accelerator_text(&Shortcut {
                key: Key::Character("q".into()),
                modifiers,
            })
            .expect("key maps")
        };
        assert_eq!(text(Modifiers::COMMAND), "Ctrl+Q");
        assert_eq!(text(Modifiers::COMMAND | Modifiers::SHIFT), "Ctrl+Shift+Q");
        // COMMAND and CONTROL are the same physical key on Windows.
        assert_eq!(text(Modifiers::COMMAND | Modifiers::CONTROL), "Ctrl+Q");
        assert_eq!(text(Modifiers::ALT), "Alt+Q");
        assert_eq!(text(Modifiers::empty()), "Q");

        let tab = accelerator_text(&Shortcut {
            key: Key::Named(NamedKey::Tab),
            modifiers: Modifiers::CONTROL | Modifiers::SHIFT,
        })
        .expect("key maps");
        assert_eq!(tab, "Ctrl+Shift+Tab");
    }

    /// A `WM_COMMAND` sent to the attached window delivers the item's
    /// `CommandId` on `events()`; an id outside the bar is forwarded to the
    /// previous procedure without reporting.
    #[test]
    fn wm_command_delivers_command_id() {
        use futures::{FutureExt, StreamExt};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
            SendMessageW, WNDCLASSW, WS_OVERLAPPED,
        };

        use windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE;

        unsafe extern "system" fn wnd_proc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            // SAFETY: forwards every message to the default procedure.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        let class_name = to_wide("WaterkitMenuSubclassTest");
        // SAFETY: registers a private class and creates an overlapped window
        // on this thread, then destroys it at the end.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&raw const class);
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("CreateWindowExW failed")
        };

        let open = Command::new("Open");
        let open_id = open.id();
        let menu_id = u32::try_from(open_id.raw()).expect("CommandId fits in u32");
        let bar =
            crate::MenuBar::new([Submenu::new("File").entry(open)]).expect("MenuBar::new failed");
        let attachment = bar.attach(hwnd).expect("attach failed");
        let mut events = std::pin::pin!(bar.events());

        let send = |menu_id: u32| unsafe {
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM(menu_id as usize)),
                Some(LPARAM(0)),
            );
        };
        send(menu_id);
        assert_eq!(events.next().now_or_never().flatten(), Some(open_id));

        // An accelerator-style WM_COMMAND (HIWORD = 1) is not a menu pick.
        unsafe {
            SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(WPARAM((1usize << 16) | menu_id as usize)),
                Some(LPARAM(0)),
            );
        }
        assert!(events.next().now_or_never().is_none());

        // An unknown menu id is forwarded to DefWindowProc, not reported.
        send(0xBEEF);
        assert!(events.next().now_or_never().is_none());

        drop(attachment);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}
