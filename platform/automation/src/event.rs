//! Typed input events.

use core::fmt;
use core::num::NonZeroU32;

use keyboard_types::{Code, Modifiers};

use crate::{Error, Input};

/// A position on a display, in points from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

impl Point {
    /// The point `x` points right of and `y` points below the display's
    /// top-left corner.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Points from the display's left edge.
    #[must_use]
    pub const fn x(&self) -> f64 {
        self.x
    }

    /// Points from the display's top edge.
    #[must_use]
    pub const fn y(&self) -> f64 {
        self.y
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The secondary button.
    Right,
    /// The middle button or wheel press.
    Middle,
}

/// An event [`Input::post`] delivers. Implemented by the event types of this
/// crate only.
pub trait InputEvent: sealed::Sealed {}

pub mod sealed {
    use crate::{Error, Input};

    pub trait Sealed {
        fn deliver(self, input: &mut Input) -> Result<(), Error>;
    }
}

const ONE: NonZeroU32 = NonZeroU32::MIN;

macro_rules! modifiers_setter {
    () => {
        /// The modifier keys held during the event.
        #[must_use]
        pub const fn modifiers(mut self, modifiers: Modifiers) -> Self {
            self.modifiers = modifiers;
            self
        }
    };
}

macro_rules! input_event {
    ($ty:ident, $deliver:ident) => {
        impl InputEvent for $ty {}

        impl sealed::Sealed for $ty {
            fn deliver(self, input: &mut Input) -> Result<(), Error> {
                input.$deliver(self)
            }
        }
    };
}

/// A mouse button going down: a press.
///
/// The window under the point receives it and becomes the key window of its
/// application; later key events go there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseDown {
    pub(crate) at: Point,
    pub(crate) button: MouseButton,
    pub(crate) clicks: NonZeroU32,
    pub(crate) modifiers: Modifiers,
}

impl MouseDown {
    /// `button` going down at `at`, as the first click of a sequence.
    #[must_use]
    pub const fn new(at: Point, button: MouseButton) -> Self {
        Self {
            at,
            button,
            clicks: ONE,
            modifiers: Modifiers::empty(),
        }
    }

    /// The press's place in a click sequence: 2 for the second press of a
    /// double click.
    #[must_use]
    pub const fn clicks(mut self, clicks: NonZeroU32) -> Self {
        self.clicks = clicks;
        self
    }

    modifiers_setter!();
}

/// A mouse button coming up.
///
/// While a button is held, the release goes to the window the press landed
/// on, wherever the point is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseUp {
    pub(crate) at: Point,
    pub(crate) button: MouseButton,
    pub(crate) clicks: NonZeroU32,
    pub(crate) modifiers: Modifiers,
}

impl MouseUp {
    /// `button` coming up at `at`, ending the first click of a sequence.
    #[must_use]
    pub const fn new(at: Point, button: MouseButton) -> Self {
        Self {
            at,
            button,
            clicks: ONE,
            modifiers: Modifiers::empty(),
        }
    }

    /// The release's place in a click sequence; it matches its press.
    #[must_use]
    pub const fn clicks(mut self, clicks: NonZeroU32) -> Self {
        self.clicks = clicks;
        self
    }

    modifiers_setter!();
}

/// The pointer moving to a point. While a button is held it is a drag, and
/// goes to the window the press landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseMove {
    pub(crate) at: Point,
    pub(crate) modifiers: Modifiers,
}

impl MouseMove {
    /// The pointer arriving at `at`.
    #[must_use]
    pub const fn new(at: Point) -> Self {
        Self {
            at,
            modifiers: Modifiers::empty(),
        }
    }

    modifiers_setter!();
}

/// A scroll-wheel turn over a point, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scroll {
    pub(crate) at: Point,
    pub(crate) dx: i32,
    pub(crate) dy: i32,
    pub(crate) modifiers: Modifiers,
}

impl Scroll {
    /// A scroll over `at` moving the view `dx` pixels right and `dy` pixels
    /// down through the content, as a web `WheelEvent`'s deltas do.
    #[must_use]
    pub const fn new(at: Point, dx: i32, dy: i32) -> Self {
        Self {
            at,
            dx,
            dy,
            modifiers: Modifiers::empty(),
        }
    }

    modifiers_setter!();
}

/// A key going down, named by its physical position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDown {
    pub(crate) code: Code,
    pub(crate) modifiers: Modifiers,
}

impl KeyDown {
    /// The key at `code` going down.
    #[must_use]
    pub const fn new(code: Code) -> Self {
        Self {
            code,
            modifiers: Modifiers::empty(),
        }
    }

    modifiers_setter!();
}

/// A key coming up, named by its physical position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyUp {
    pub(crate) code: Code,
    pub(crate) modifiers: Modifiers,
}

impl KeyUp {
    /// The key at `code` coming up.
    #[must_use]
    pub const fn new(code: Code) -> Self {
        Self {
            code,
            modifiers: Modifiers::empty(),
        }
    }

    modifiers_setter!();
}

/// Text typed as key presses carrying the characters themselves, whatever
/// the keyboard layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    pub(crate) text: String,
}

impl Text {
    /// Typing `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

input_event!(MouseDown, mouse_down);
input_event!(MouseUp, mouse_up);
input_event!(MouseMove, mouse_move);
input_event!(Scroll, scroll);
input_event!(KeyDown, key_down);
input_event!(KeyUp, key_up);
impl InputEvent for Text {}

impl sealed::Sealed for Text {
    fn deliver(self, input: &mut Input) -> Result<(), Error> {
        input.text(&self)
    }
}
