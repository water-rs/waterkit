//! Platform-neutral menu description types.
//!
//! These types describe *what* the menu bar contains. They exist on every
//! platform; only the platforms that own a native menu-bar object (macOS and
//! Windows) can turn them into a [`MenuBar`](crate::MenuBar).

use std::fmt;

use keyboard_types::Key;

/// Identifier of a [`Command`], supplied by the caller at construction.
///
/// Activation events delivered by [`MenuBar::events`](crate::MenuBar::events)
/// carry this id. Ids must be unique within one [`MenuBar`](crate::MenuBar);
/// [`MenuBar::new`](crate::MenuBar::new) fails with
/// [`MenuError::DuplicateCommandId`](crate::MenuError::DuplicateCommandId)
/// on a repeated id. Different bars may reuse ids freely — each bar owns its
/// own stream.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(u64);

impl CommandId {
    /// Creates a command id from a caller-chosen number.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The caller-chosen number this id was created with.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommandId({})", self.0)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A titled pull-down menu in a [`MenuBar`](crate::MenuBar).
///
/// The macOS application menu (the bold first menu holding About, Services,
/// Hide and Quit) is just a `Submenu` whose title is the application name and
/// whose entries are [`StandardItem`]s.
#[derive(Debug)]
pub struct Submenu {
    pub(crate) title: String,
    pub(crate) entries: Vec<Entry>,
    pub(crate) windows_menu: bool,
}

impl Submenu {
    /// Creates a submenu with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            entries: Vec::new(),
            windows_menu: false,
        }
    }

    /// Appends an entry to the submenu.
    #[must_use]
    pub fn entry(mut self, entry: impl Into<Entry>) -> Self {
        self.entries.push(entry.into());
        self
    }

    /// Marks this submenu as the application's windows menu:
    /// [`MenuBar::install`](crate::MenuBar::install) registers it as
    /// `NSApp.windowsMenu`, which is what makes `AppKit` keep the live
    /// window list on it.
    ///
    /// The mark exists only on macOS:
    /// [`MenuBar::new`](crate::MenuBar::new) fails with
    /// [`MenuError::StandardItemUnsupported`](crate::MenuError::StandardItemUnsupported)
    /// on any other platform, and with
    /// [`MenuError::DuplicateWindowsMenu`](crate::MenuError::DuplicateWindowsMenu)
    /// when more than one submenu in the bar is marked.
    #[must_use]
    pub const fn windows_menu(mut self) -> Self {
        self.windows_menu = true;
        self
    }
}

/// One row inside a [`Submenu`].
#[derive(Debug)]
pub enum Entry {
    /// An actionable item that produces a [`CommandId`] on activation.
    Command(Command),
    /// A nested pull-down menu.
    Submenu(Submenu),
    /// A visual separator line.
    Separator,
    /// One of the platform's standard items.
    Standard(StandardItem),
}

impl From<Command> for Entry {
    fn from(command: Command) -> Self {
        Self::Command(command)
    }
}

impl From<Submenu> for Entry {
    fn from(submenu: Submenu) -> Self {
        Self::Submenu(submenu)
    }
}

impl From<StandardItem> for Entry {
    fn from(item: StandardItem) -> Self {
        Self::Standard(item)
    }
}

/// An actionable menu item.
///
/// The caller supplies the item's [`CommandId`]; after the bar is built,
/// [`MenuBar::events`](crate::MenuBar::events) yields it each time the item
/// is chosen or its accelerator is pressed.
#[derive(Debug)]
pub struct Command {
    pub(crate) id: CommandId,
    pub(crate) title: String,
    pub(crate) shortcut: Option<Shortcut>,
    pub(crate) enabled: bool,
    pub(crate) checked: Option<bool>,
}

impl Command {
    /// Creates a command with the given id and title, enabled and unchecked.
    pub fn new(id: CommandId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }

    /// Gives the command a keyboard shortcut.
    #[must_use]
    pub fn shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Makes the command a checkable item showing a checkmark.
    ///
    /// `None` (the default) keeps the item plain.
    #[must_use]
    pub const fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Enables or disables the command.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The id this command was created with.
    #[must_use]
    pub const fn id(&self) -> CommandId {
        self.id
    }
}

/// A keyboard shortcut: a W3C `KeyboardEvent.key` plus modifiers.
///
/// On Windows the item's title is rendered with the accelerator text, and
/// [`MenuBar::accelerator_table`](crate::MenuBar::accelerator_table) provides
/// the matching `HACCEL` for hosts that call `TranslateAcceleratorW` in their
/// message pump. Hosts that never translate accelerators dispatch the chord
/// themselves and activate the matching [`Command`].
#[derive(Debug, Clone)]
pub struct Shortcut {
    /// The key, using W3C `KeyboardEvent.key` names (`Key::Character("s")`,
    /// `Key::Named(NamedKey::Delete)`, ...).
    pub key: Key,
    /// The modifiers to hold with the key. `COMMAND` is the platform's menu
    /// accelerator modifier: ⌘ on macOS, Ctrl on Windows.
    pub modifiers: Modifiers,
}

impl Shortcut {
    /// Creates a shortcut for the given key and modifiers.
    pub fn new(key: impl Into<Key>, modifiers: Modifiers) -> Self {
        Self {
            key: key.into(),
            modifiers,
        }
    }
}

/// Modifier set of a [`Shortcut`].
///
/// `COMMAND` is the platform's menu accelerator modifier: ⌘ on macOS, Ctrl on
/// Windows. `CONTROL` is the literal Control key (⌃ on macOS); on Windows it is
/// the same physical modifier as `COMMAND`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    /// The platform's menu accelerator modifier: ⌘ on macOS, Ctrl on Windows.
    pub const COMMAND: Self = Self(0b0001);
    /// The Control modifier: ⌃ on macOS, Ctrl on Windows.
    pub const CONTROL: Self = Self(0b0010);
    /// The Alt modifier: ⌥ on macOS, Alt on Windows.
    pub const ALT: Self = Self(0b0100);
    /// The Shift modifier: ⇧ on macOS, Shift on Windows.
    pub const SHIFT: Self = Self(0b1000);

    /// No modifiers.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns whether every bit in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns whether no modifiers are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for Modifiers {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (flag, name) in [
            (Self::COMMAND, "COMMAND"),
            (Self::CONTROL, "CONTROL"),
            (Self::ALT, "ALT"),
            (Self::SHIFT, "SHIFT"),
        ] {
            if self.contains(flag) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        if first {
            f.write_str("(empty)")?;
        }
        Ok(())
    }
}

/// One of the macOS menu bar's system items.
///
/// These get the platform's standard action — and key equivalent where the
/// platform assigns one — through the responder chain rather than reporting
/// a [`CommandId`]: the application menu holds About, Services, Hide, Hide
/// Others, Show All and Quit, and the windows menu holds Minimize, Zoom and
/// Bring All to Front. They exist only on macOS: constructing one on
/// another platform fails [`MenuBar::new`](crate::MenuBar::new) with
/// [`MenuError::StandardItemUnsupported`](crate::MenuError::StandardItemUnsupported).
#[derive(Debug)]
pub enum StandardItem {
    /// "About `name`" — opens the standard About panel.
    About {
        /// The application name shown in the item title.
        name: String,
    },
    /// "Services" — the standard services submenu, wired to
    /// `NSApp.servicesMenu` on install.
    Services,
    /// "Hide `name`" — hides the application (⌘H).
    Hide {
        /// The application name shown in the item title.
        name: String,
    },
    /// "Hide Others" — hides every other application (⌥⌘H).
    HideOthers,
    /// "Show All" — unhides all applications.
    ShowAll,
    /// "Quit `name`" — terminates the application (⌘Q).
    Quit {
        /// The application name shown in the item title.
        name: String,
    },
    /// "Minimize" — minimizes the key window (`performMiniaturize:`).
    Minimize,
    /// "Zoom" — zooms the key window (`performZoom:`).
    Zoom,
    /// "Bring All to Front" — orders the application's windows to the
    /// front (`arrangeInFront:`).
    BringAllToFront,
}
