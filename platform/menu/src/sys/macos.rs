//! macOS backend: the bar is an `NSMenu` tree installed as `NSApp.mainMenu`.
//!
//! Activation: every `Command` item targets a `MenuCommandTarget` (a small
//! Objective-C class defined here) whose action sends the item's
//! [`CommandId`] into the bar's own channel — there is no process-global
//! channel and no `static` state.

use std::collections::HashMap;

use async_channel::Sender;
use keyboard_types::{Key, NamedKey};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use objc2_foundation::NSString;

use crate::{Command, CommandId, Entry, MenuError, Modifiers, Shortcut, StandardItem, Submenu};

/// The `ObjC` ivars of [`MenuCommandTarget`]: the id to report and the bar's
/// own event channel.
#[derive(Debug)]
struct MenuCommandTargetIvars {
    command_id: CommandId,
    sender: Sender<CommandId>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WaterkitMenuCommandTarget"]
    #[ivars = MenuCommandTargetIvars]
    #[derive(Debug)]
    struct MenuCommandTarget;

    impl MenuCommandTarget {
        #[unsafe(method(waterkitMenuCommandActivate:))]
        fn activate(&self, _sender: &NSMenuItem) {
            let ivars = self.ivars();
            // The channel is unbounded and the bar holds a receiver, so
            // `try_send` cannot fail while the bar is alive.
            ivars
                .sender
                .try_send(ivars.command_id)
                .expect("waterkit-menu: the event channel is closed while the bar is alive");
        }
    }
);

impl MenuCommandTarget {
    fn new(
        mtm: MainThreadMarker,
        command_id: CommandId,
        sender: Sender<CommandId>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuCommandTargetIvars { command_id, sender });
        // SAFETY: NSObject's `init` needs no further guarantees; `this` is a
        // freshly allocated object of the right class.
        unsafe { msg_send![super(this), init] }
    }
}

/// The macOS menu bar: owns the `NSMenu` tree plus the per-item action
/// targets (`NSMenuItem.target` is weak, so the bar keeps them alive).
#[derive(Debug)]
pub struct MenuBarInner {
    bar: Retained<NSMenu>,
    commands: HashMap<CommandId, Retained<NSMenuItem>>,
    targets: Vec<Retained<MenuCommandTarget>>,
    services_menu: Option<Retained<NSMenu>>,
    windows_menu: Option<Retained<NSMenu>>,
}

impl MenuBarInner {
    pub(crate) fn new(
        menus: impl IntoIterator<Item = Submenu>,
        sender: &Sender<CommandId>,
    ) -> Result<Self, MenuError> {
        let mtm = MainThreadMarker::new().expect(
            "waterkit-menu: MenuBar must be built on the main thread (AppKit is main-thread only)",
        );
        let mut inner = Self {
            bar: NSMenu::new(mtm),
            commands: HashMap::new(),
            targets: Vec::new(),
            services_menu: None,
            windows_menu: None,
        };
        // Manual state management: with auto-validation on, AppKit would
        // re-enable items at display time because our targets implement no
        // `validateMenuItem:`.
        inner.bar.setAutoenablesItems(false);
        for submenu in menus {
            let item = inner.submenu_item(&submenu, mtm, sender)?;
            inner.bar.addItem(&item);
        }
        Ok(inner)
    }

    /// Installs the bar as `NSApp.mainMenu`, replacing the previous one, and
    /// wires the services submenu if the bar has one.
    pub(crate) fn install(&self, mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        if let Some(services) = &self.services_menu {
            app.setServicesMenu(Some(services));
        }
        app.setMainMenu(Some(&self.bar));
        // The windows menu lives inside the installed bar, so registration
        // follows `setMainMenu`: AppKit keeps the live window list on the
        // marked submenu. A bar marking none clears a previous bar's
        // registration.
        app.setWindowsMenu(self.windows_menu.as_deref());
    }

    pub(crate) fn set_enabled(&self, id: CommandId, enabled: bool) {
        self.item(id, "set_enabled").setEnabled(enabled);
    }

    pub(crate) fn set_checked(&self, id: CommandId, checked: bool) {
        self.item(id, "set_checked").setState(if checked {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    fn item(&self, id: CommandId, caller: &str) -> &NSMenuItem {
        self.commands
            .get(&id)
            .unwrap_or_else(|| panic!("waterkit-menu: {caller}: CommandId {id} is not in this bar"))
    }

    /// Builds the item placed in the parent menu: its title shows `submenu`'s
    /// title and it carries the freshly built `NSMenu`.
    fn submenu_item(
        &mut self,
        submenu: &Submenu,
        mtm: MainThreadMarker,
        sender: &Sender<CommandId>,
    ) -> Result<Retained<NSMenuItem>, MenuError> {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setTitle(&NSString::from_str(&submenu.title));
        for entry in &submenu.entries {
            let item = self.entry_item(entry, mtm, sender)?;
            menu.addItem(&item);
        }
        if submenu.windows_menu {
            // `MenuBar::new` rejects a second mark, so this is assigned at
            // most once.
            self.windows_menu = Some(menu.clone());
        }
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(&submenu.title));
        item.setSubmenu(Some(&menu));
        Ok(item)
    }

    fn entry_item(
        &mut self,
        entry: &Entry,
        mtm: MainThreadMarker,
        sender: &Sender<CommandId>,
    ) -> Result<Retained<NSMenuItem>, MenuError> {
        match entry {
            Entry::Command(command) => self.command_item(command, mtm, sender),
            Entry::Submenu(submenu) => self.submenu_item(submenu, mtm, sender),
            Entry::Separator => Ok(NSMenuItem::separatorItem(mtm)),
            Entry::Standard(item) => Ok(self.standard_item(item, mtm)),
        }
    }

    fn command_item(
        &mut self,
        command: &Command,
        mtm: MainThreadMarker,
        sender: &Sender<CommandId>,
    ) -> Result<Retained<NSMenuItem>, MenuError> {
        if self.commands.contains_key(&command.id()) {
            return Err(MenuError::DuplicateCommandId(command.id()));
        }
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(&command.title));
        if let Some(shortcut) = &command.shortcut {
            let equivalent = key_equivalent(shortcut)?;
            item.setKeyEquivalent(&equivalent);
            item.setKeyEquivalentModifierMask(modifier_mask(shortcut.modifiers));
        }
        if let Some(checked) = command.checked {
            item.setState(if checked {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        item.setEnabled(command.enabled);
        let target = MenuCommandTarget::new(mtm, command.id(), sender.clone());
        // SAFETY: the target lives as long as the bar (it is kept in
        // `self.targets`), so the weak item target never dangles.
        unsafe {
            item.setTarget(Some(&*target as &AnyObject));
            item.setAction(Some(sel!(waterkitMenuCommandActivate:)));
        }
        self.targets.push(target);
        self.commands.insert(command.id(), item.clone());
        Ok(item)
    }

    /// One of the macOS application menu's standard items. Their targets stay
    /// `nil`, so the responder chain (`NSApp`) receives the standard action.
    fn standard_item(
        &mut self,
        item: &StandardItem,
        mtm: MainThreadMarker,
    ) -> Retained<NSMenuItem> {
        let menu_item = NSMenuItem::new(mtm);
        match item {
            StandardItem::About { name } => {
                menu_item.setTitle(&NSString::from_str(&format!("About {name}")));
                // SAFETY: `orderFrontStandardAboutPanel:` is a standard AppKit
                // action handled by the responder chain.
                unsafe { menu_item.setAction(Some(sel!(orderFrontStandardAboutPanel:))) };
            }
            StandardItem::Services => {
                menu_item.setTitle(&NSString::from_str("Services"));
                let services = NSMenu::new(mtm);
                services.setAutoenablesItems(false);
                menu_item.setSubmenu(Some(&services));
                self.services_menu = Some(services);
            }
            StandardItem::Hide { name } => {
                menu_item.setTitle(&NSString::from_str(&format!("Hide {name}")));
                // SAFETY: `hide:` is the standard AppKit hide action.
                unsafe { menu_item.setAction(Some(sel!(hide:))) };
                menu_item.setKeyEquivalent(&NSString::from_str("h"));
                menu_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
            }
            StandardItem::HideOthers => {
                menu_item.setTitle(&NSString::from_str("Hide Others"));
                // SAFETY: `hideOtherApplications:` is a standard AppKit action.
                unsafe { menu_item.setAction(Some(sel!(hideOtherApplications:))) };
                menu_item.setKeyEquivalent(&NSString::from_str("h"));
                menu_item.setKeyEquivalentModifierMask(
                    NSEventModifierFlags::Command | NSEventModifierFlags::Option,
                );
            }
            StandardItem::ShowAll => {
                menu_item.setTitle(&NSString::from_str("Show All"));
                // SAFETY: `unhideAllApplications:` is a standard AppKit action.
                unsafe { menu_item.setAction(Some(sel!(unhideAllApplications:))) };
            }
            StandardItem::Quit { name } => {
                menu_item.setTitle(&NSString::from_str(&format!("Quit {name}")));
                // SAFETY: `terminate:` is the standard AppKit quit action.
                unsafe { menu_item.setAction(Some(sel!(terminate:))) };
                menu_item.setKeyEquivalent(&NSString::from_str("q"));
                menu_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
            }
            StandardItem::Minimize => {
                menu_item.setTitle(&NSString::from_str("Minimize"));
                // SAFETY: `performMiniaturize:` is the standard AppKit
                // minimize action; the nil target routes it to the key window.
                unsafe { menu_item.setAction(Some(sel!(performMiniaturize:))) };
            }
            StandardItem::Zoom => {
                menu_item.setTitle(&NSString::from_str("Zoom"));
                // SAFETY: `performZoom:` is the standard AppKit zoom action.
                unsafe { menu_item.setAction(Some(sel!(performZoom:))) };
            }
            StandardItem::BringAllToFront => {
                menu_item.setTitle(&NSString::from_str("Bring All to Front"));
                // SAFETY: `arrangeInFront:` is the standard AppKit
                // order-to-front action.
                unsafe { menu_item.setAction(Some(sel!(arrangeInFront:))) };
            }
        }
        menu_item
    }
}

/// Maps a shortcut to the string passed to `-[NSMenuItem setKeyEquivalent:]`.
fn key_equivalent(shortcut: &Shortcut) -> Result<Retained<NSString>, MenuError> {
    let scalar = match &shortcut.key {
        Key::Character(character) => {
            let mut chars = character.chars();
            let Some(c) = chars.next() else {
                return Err(MenuError::UnmappableKey(shortcut.key.clone()));
            };
            if chars.next().is_some() {
                // A key equivalent is a single unichar; a multi-scalar key
                // value has no macOS equivalent.
                return Err(MenuError::UnmappableKey(shortcut.key.clone()));
            }
            c
        }
        Key::Named(named) => named_key_equivalent(*named)
            .ok_or_else(|| MenuError::UnmappableKey(shortcut.key.clone()))?,
    };
    Ok(NSString::from_str(&scalar.to_string()))
}

/// Maps a shortcut's modifiers to `NSEventModifierFlags`.
fn modifier_mask(modifiers: Modifiers) -> NSEventModifierFlags {
    let mut flags = NSEventModifierFlags::empty();
    if modifiers.contains(Modifiers::COMMAND) {
        flags.insert(NSEventModifierFlags::Command);
    }
    if modifiers.contains(Modifiers::CONTROL) {
        flags.insert(NSEventModifierFlags::Control);
    }
    if modifiers.contains(Modifiers::ALT) {
        flags.insert(NSEventModifierFlags::Option);
    }
    if modifiers.contains(Modifiers::SHIFT) {
        flags.insert(NSEventModifierFlags::Shift);
    }
    flags
}

/// The unichar a named key maps to in `-[NSMenuItem setKeyEquivalent:]` — the
/// `NS*FunctionKey` constants from `NSEvent.h` (private-use area U+F700+), or
/// an ASCII control character for keys like Enter and Tab.
///
/// Keys without a real `AppKit` key equivalent (modifier keys themselves,
/// dead keys, media keys, ...) return `None` and become
/// [`MenuError::UnmappableKey`] at `MenuBar::new`.
const fn named_key_equivalent(key: NamedKey) -> Option<char> {
    let code = match key {
        NamedKey::ArrowUp => 0xF700,
        NamedKey::ArrowDown => 0xF701,
        NamedKey::ArrowLeft => 0xF702,
        NamedKey::ArrowRight => 0xF703,
        NamedKey::F1 => 0xF704,
        NamedKey::F2 => 0xF705,
        NamedKey::F3 => 0xF706,
        NamedKey::F4 => 0xF707,
        NamedKey::F5 => 0xF708,
        NamedKey::F6 => 0xF709,
        NamedKey::F7 => 0xF70A,
        NamedKey::F8 => 0xF70B,
        NamedKey::F9 => 0xF70C,
        NamedKey::F10 => 0xF70D,
        NamedKey::F11 => 0xF70E,
        NamedKey::F12 => 0xF70F,
        NamedKey::F13 => 0xF710,
        NamedKey::F14 => 0xF711,
        NamedKey::F15 => 0xF712,
        NamedKey::F16 => 0xF713,
        NamedKey::F17 => 0xF714,
        NamedKey::F18 => 0xF715,
        NamedKey::F19 => 0xF716,
        NamedKey::F20 => 0xF717,
        NamedKey::F21 => 0xF718,
        NamedKey::F22 => 0xF719,
        NamedKey::F23 => 0xF71A,
        NamedKey::F24 => 0xF71B,
        NamedKey::F25 => 0xF71C,
        NamedKey::F26 => 0xF71D,
        NamedKey::F27 => 0xF71E,
        NamedKey::F28 => 0xF71F,
        NamedKey::F29 => 0xF720,
        NamedKey::F30 => 0xF721,
        NamedKey::F31 => 0xF722,
        NamedKey::F32 => 0xF723,
        NamedKey::F33 => 0xF724,
        NamedKey::F34 => 0xF725,
        NamedKey::F35 => 0xF726,
        NamedKey::Insert => 0xF727,
        // Forward delete: NSDeleteFunctionKey.
        NamedKey::Delete => 0xF728,
        NamedKey::Home => 0xF729,
        NamedKey::End => 0xF72B,
        NamedKey::PageUp => 0xF72C,
        NamedKey::PageDown => 0xF72D,
        NamedKey::PrintScreen => 0xF72E,
        NamedKey::ScrollLock => 0xF72F,
        NamedKey::Pause => 0xF730,
        // The application menu key: NSMenuFunctionKey.
        NamedKey::ContextMenu => 0xF735,
        // The numeric keypad's Clear key: NSClearLineFunctionKey.
        NamedKey::Clear => 0xF739,
        NamedKey::Select => 0xF741,
        NamedKey::Execute => 0xF742,
        NamedKey::Undo => 0xF743,
        NamedKey::Redo => 0xF744,
        NamedKey::Find => 0xF745,
        NamedKey::Help => 0xF746,
        // Backspace: NSDeleteCharacter.
        NamedKey::Backspace => 0x007F,
        NamedKey::Enter => 0x000D,
        NamedKey::Escape => 0x001B,
        NamedKey::Tab => 0x0009,
        _ => return None,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StandardItem;

    fn shortcut_of(key: Key) -> Shortcut {
        Shortcut {
            key,
            modifiers: Modifiers::COMMAND,
        }
    }

    fn equivalent(key: Key) -> String {
        key_equivalent(&shortcut_of(key))
            .expect("key should map")
            .to_string()
    }

    #[test]
    fn character_keys_map_to_their_character() {
        assert_eq!(equivalent(Key::Character("a".into())), "a");
        // An uppercase key value maps to its character; AppKit implies ⇧.
        assert_eq!(equivalent(Key::Character("S".into())), "S");
        assert_eq!(equivalent(Key::Character(" ".into())), " ");
    }

    #[test]
    fn delete_and_backspace_map_to_distinct_equivalents() {
        // NSDeleteFunctionKey and NSDeleteCharacter from NSEvent.h.
        assert_eq!(equivalent(Key::Named(NamedKey::Delete)), "\u{F728}");
        assert_eq!(equivalent(Key::Named(NamedKey::Backspace)), "\u{7F}");
    }

    #[test]
    fn control_and_function_keys_map_to_nsevent_constants() {
        assert_eq!(equivalent(Key::Named(NamedKey::Enter)), "\u{D}");
        assert_eq!(equivalent(Key::Named(NamedKey::Escape)), "\u{1B}");
        assert_eq!(equivalent(Key::Named(NamedKey::Tab)), "\u{9}");
        assert_eq!(equivalent(Key::Named(NamedKey::ArrowUp)), "\u{F700}");
        assert_eq!(equivalent(Key::Named(NamedKey::ArrowDown)), "\u{F701}");
        assert_eq!(equivalent(Key::Named(NamedKey::ArrowLeft)), "\u{F702}");
        assert_eq!(equivalent(Key::Named(NamedKey::ArrowRight)), "\u{F703}");
        assert_eq!(equivalent(Key::Named(NamedKey::F1)), "\u{F704}");
        assert_eq!(equivalent(Key::Named(NamedKey::F24)), "\u{F71B}");
        assert_eq!(equivalent(Key::Named(NamedKey::F35)), "\u{F726}");
        assert_eq!(equivalent(Key::Named(NamedKey::Home)), "\u{F729}");
        assert_eq!(equivalent(Key::Named(NamedKey::End)), "\u{F72B}");
        assert_eq!(equivalent(Key::Named(NamedKey::PageUp)), "\u{F72C}");
        assert_eq!(equivalent(Key::Named(NamedKey::PageDown)), "\u{F72D}");
    }

    #[test]
    fn unmappable_keys_error() {
        for key in [
            // Modifier keys cannot be menu accelerators on their own.
            Key::Named(NamedKey::Control),
            Key::Named(NamedKey::Shift),
            Key::Named(NamedKey::Meta),
            Key::Named(NamedKey::Alt),
            Key::Named(NamedKey::CapsLock),
            // Dead and media keys have no menu equivalent either.
            Key::Named(NamedKey::Dead),
            Key::Named(NamedKey::MediaPlayPause),
            // Multi-scalar values are not a single unichar.
            Key::Character("ab".into()),
            Key::Character(String::new()),
        ] {
            assert!(matches!(
                key_equivalent(&shortcut_of(key)),
                Err(MenuError::UnmappableKey(_))
            ));
        }
    }

    #[test]
    fn modifiers_map_to_nsevent_flags() {
        let mask = modifier_mask(Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT);
        assert!(mask.contains(NSEventModifierFlags::Command));
        assert!(mask.contains(NSEventModifierFlags::Shift));
        assert!(mask.contains(NSEventModifierFlags::Option));
        assert!(!mask.contains(NSEventModifierFlags::Control));

        let mask = modifier_mask(Modifiers::CONTROL);
        assert!(mask.contains(NSEventModifierFlags::Control));
        assert!(!mask.contains(NSEventModifierFlags::Command));
    }

    #[test]
    fn submenu_builder_collects_entries() {
        let submenu = Submenu::new("File")
            .entry(Command::new(CommandId::new(1), "Open"))
            .entry(Entry::Separator)
            .entry(StandardItem::ShowAll);
        assert_eq!(submenu.entries.len(), 3);
        assert_eq!(submenu.title, "File");
    }

    #[test]
    fn command_keeps_the_supplied_id() {
        let command = Command::new(CommandId::new(42), "a");
        assert_eq!(command.id(), CommandId::new(42));
    }
}
