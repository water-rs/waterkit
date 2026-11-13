//! The elements of the windows on a display that an agent can act on, as
//! Accessibility reports them, and the references that address them.

use core::fmt;
use core::ptr::{self, NonNull};
use core::str::FromStr;
use core::time::Duration;

use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};

use crate::accessibility::decode;
use crate::error::platform;
use crate::{Error, Point};

/// The attribute an element's value is in.
pub const VALUE: &str = "AXValue";
/// The attribute that says whether an element has keyboard focus.
pub const FOCUSED: &str = "AXFocused";
/// The attribute of an application that names the element with keyboard
/// focus.
const FOCUSED_ELEMENT: &str = "AXFocusedUIElement";
/// How many ancestors the walk up from the focused element visits: far
/// deeper than any tree an application builds (a web page nests a few
/// hundred levels at most), so it ends at the root, and only a cyclic
/// `AXParent` chain runs it out.
const MAX_ANCESTORS: usize = 1024;
/// The action that presses a control.
pub const PRESS: &str = "AXPress";

/// One element of a listing [`Input::elements`](crate::Input::elements)
/// made: which listing, and where in it.
///
/// It stays valid until the same [`Input`](crate::Input) lists the elements
/// again; acting on it after that, or on an [`Input`](crate::Input) that did
/// not list it, fails with [`Error::StaleElement`] instead of acting on
/// whatever now holds its place.
///
/// Its text form, which [`FromStr`] reads back, is the listing as eight hex
/// digits, a colon and the index, such as `0a3f91c2:12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementRef {
    listing: u32,
    index: u32,
}

impl ElementRef {
    pub(crate) const fn new(listing: u32, index: u32) -> Self {
        Self { listing, index }
    }

    /// The listing that holds the element.
    #[must_use]
    pub const fn listing(&self) -> u32 {
        self.listing
    }

    /// The element's place in its listing, from 0.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Display for ElementRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08x}:{}", self.listing, self.index)
    }
}

/// Text that is not an [`ElementRef`]'s text form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{0:?} is not an element reference: expected eight hex digits, a colon and an index, such as 0a3f91c2:12"
)]
pub struct ParseElementRefError(String);

impl FromStr for ElementRef {
    type Err = ParseElementRefError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ParseElementRefError(text.to_owned());
        let (listing, index) = text.split_once(':').ok_or_else(invalid)?;
        // `from_str_radix` and `parse` take a sign; the text form has none.
        if listing.len() != 8 || !listing.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        Ok(Self {
            listing: u32::from_str_radix(listing, 16).map_err(|_| invalid())?,
            index: index.parse().map_err(|_| invalid())?,
        })
    }
}

/// A rectangle on a display, in points from its top-left corner: the space
/// a [`Point`] is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    origin: Point,
    width: f64,
    height: f64,
}

impl Frame {
    /// The global rectangle `rect` on the display whose top-left corner is
    /// at `display` in global coordinates.
    pub(crate) const fn on_display(rect: CGRect, display: CGPoint) -> Self {
        Self {
            origin: Point::new(rect.origin.x - display.x, rect.origin.y - display.y),
            width: rect.size.width,
            height: rect.size.height,
        }
    }

    /// The top-left corner.
    #[must_use]
    pub const fn origin(&self) -> Point {
        self.origin
    }

    /// Width in points.
    #[must_use]
    pub const fn width(&self) -> f64 {
        self.width
    }

    /// Height in points.
    #[must_use]
    pub const fn height(&self) -> f64 {
        self.height
    }

    /// The centre, where a press at the element lands.
    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(
            self.origin.x() + self.width / 2.0,
            self.origin.y() + self.height / 2.0,
        )
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{} at {}", self.width, self.height, self.origin)
    }
}

/// One element of a window on the display that can be pressed, focused or
/// given a value, as its application reported it when it was listed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Element {
    reference: ElementRef,
    pid: libc::pid_t,
    window: u32,
    role: String,
    subrole: Option<String>,
    title: Option<String>,
    label: Option<String>,
    value: Option<String>,
    placeholder: Option<String>,
    enabled: bool,
    frame: Option<Frame>,
}

/// What an application reports about one element, as [`Element`] carries
/// it.
#[derive(Debug)]
pub struct Reported {
    pub role: String,
    pub subrole: Option<String>,
    pub title: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub enabled: bool,
    /// In global display coordinates.
    pub frame: Option<CGRect>,
}

impl Element {
    pub(crate) fn new(
        reference: ElementRef,
        handle: &Handle,
        reported: Reported,
        display: CGPoint,
    ) -> Self {
        Self {
            reference,
            pid: handle.pid,
            window: handle.window,
            role: reported.role,
            subrole: reported.subrole,
            title: reported.title,
            label: reported.label,
            value: reported.value,
            placeholder: reported.placeholder,
            enabled: reported.enabled,
            frame: reported
                .frame
                .map(|frame| Frame::on_display(frame, display)),
        }
    }

    /// The reference that acts on the element.
    #[must_use]
    pub const fn reference(&self) -> ElementRef {
        self.reference
    }

    /// The process of the application that owns the element.
    #[must_use]
    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }

    /// The `WindowServer` number of the window the element was listed in.
    #[must_use]
    pub const fn window(&self) -> u32 {
        self.window
    }

    /// The Accessibility role (`AXRole`), such as `AXButton` or
    /// `AXTextField`.
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// The Accessibility subrole (`AXSubrole`), such as
    /// `AXSecureTextField`.
    #[must_use]
    pub fn subrole(&self) -> Option<&str> {
        self.subrole.as_deref()
    }

    /// The title it shows (`AXTitle`), such as a button's text.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The label it carries for assistive applications (`AXDescription`),
    /// such as an icon button's name or a web field's label.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Its value (`AXValue`) as text: a field's text, a slider's position.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// The hint an empty field shows (`AXPlaceholderValue`).
    #[must_use]
    pub fn placeholder(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    /// Whether it takes input: false only when it reports itself disabled
    /// (`AXEnabled`).
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Where it lies on the display (`AXPosition`, `AXSize`), when it
    /// reports both. Part of it, or all of it, can lie outside the display,
    /// such as a field scrolled out of a page.
    #[must_use]
    pub const fn frame(&self) -> Option<Frame> {
        self.frame
    }
}

/// The elements an agent can act on in the application windows on a display,
/// front window first and each window's in its own order.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Elements {
    listing: u32,
    listed: Vec<Element>,
    skipped: Vec<SkippedRead>,
    truncated: bool,
}

impl Elements {
    pub(crate) const fn new(
        listing: u32,
        listed: Vec<Element>,
        skipped: Vec<SkippedRead>,
        truncated: bool,
    ) -> Self {
        Self {
            listing,
            listed,
            skipped,
            truncated,
        }
    }

    /// The reads an application failed while it was listed, each of which
    /// left a node out or counted it as having nothing to read there, such
    /// as no actions.
    #[must_use]
    pub fn skipped(&self) -> &[SkippedRead] {
        &self.skipped
    }

    /// The listing every reference in it names.
    #[must_use]
    pub const fn listing(&self) -> u32 {
        self.listing
    }

    /// The elements, each with its reference.
    #[must_use]
    pub fn elements(&self) -> &[Element] {
        &self.listed
    }

    /// Whether the listing left out the rest of an application or of the
    /// display: it reached its [budget](crate::Input::element_budget), or an
    /// application could not be read at all, such as one that stopped
    /// answering. A failed read of one node does not truncate it; it is in
    /// [`skipped`](Self::skipped).
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

/// One Accessibility read an application failed during a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkippedRead {
    pid: libc::pid_t,
    window: Option<u32>,
    role: Option<String>,
    read: &'static str,
    status: i32,
}

impl SkippedRead {
    pub(crate) fn new(
        pid: libc::pid_t,
        window: Option<u32>,
        role: Option<&str>,
        read: &'static str,
        status: AXError,
    ) -> Self {
        Self {
            pid,
            window,
            role: role.map(str::to_owned),
            read,
            status: status.0,
        }
    }

    /// The application's process.
    #[must_use]
    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }

    /// The window being walked, when the read was of one of its nodes.
    #[must_use]
    pub const fn window(&self) -> Option<u32> {
        self.window
    }

    /// The node's role, when it was read before the failure.
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// What was read: an attribute such as `AXChildren`, or `actions`,
    /// `attributes`, `messaging timeout`, `windows` or `window number`.
    #[must_use]
    pub const fn read(&self) -> &'static str {
        self.read
    }

    /// The `AXError` the read failed with, such as -25200
    /// (`kAXErrorFailure`).
    #[must_use]
    pub const fn status(&self) -> i32 {
        self.status
    }
}

impl fmt::Display for SkippedRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pid {}", self.pid)?;
        if let Some(window) = self.window {
            write!(f, ", window {window}")?;
        }
        if let Some(role) = &self.role {
            write!(f, ", {role}")?;
        }
        write!(
            f,
            ": reading {} failed with AXError {}",
            self.read, self.status
        )
    }
}

/// The elements of an input's latest listing, as Accessibility holds them.
#[derive(Debug)]
pub struct Listing {
    pub id: u32,
    pub handles: Vec<Handle>,
}

impl Listing {
    /// The element `reference` names, when `listing` is the latest one.
    pub fn resolve(listing: Option<&Self>, reference: ElementRef) -> Result<Handle, Error> {
        match listing {
            Some(listing) if listing.id == reference.listing => {
                let index = usize::try_from(reference.index).expect("u32 fits in usize");
                listing
                    .handles
                    .get(index)
                    .cloned()
                    .ok_or_else(|| Error::UnknownElement {
                        element: reference,
                        count: u32::try_from(listing.handles.len())
                            .expect("a listing holds at most its u32 budget"),
                    })
            }
            current => Err(Error::StaleElement {
                element: reference,
                current: current.map(|listing| listing.id),
            }),
        }
    }
}

/// One listed element as Accessibility holds it, with the window it was
/// listed in.
#[derive(Debug, Clone)]
pub struct Handle {
    element: CFRetained<AXUIElement>,
    reference: ElementRef,
    pid: libc::pid_t,
    window: u32,
    /// The messaging timeout set on the element.
    timeout: Duration,
}

// SAFETY: an `AXUIElementRef` is an immutable Core Foundation object naming
// an element of another process, which Core Foundation lets any thread use,
// and every Accessibility call on it is a message to that process. The one
// setting it carries, its messaging timeout, is set before the handle is
// made.
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the element is the field the safety comment covers"
)]
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

impl Handle {
    /// `element` of `pid`, listed in `window` as `reference`, whose
    /// messaging timeout is `timeout`.
    pub const fn new(
        element: CFRetained<AXUIElement>,
        reference: ElementRef,
        pid: libc::pid_t,
        window: u32,
        timeout: Duration,
    ) -> Self {
        Self {
            element,
            reference,
            pid,
            window,
            timeout,
        }
    }

    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }

    pub const fn window(&self) -> u32 {
        self.window
    }

    pub const fn reference(&self) -> ElementRef {
        self.reference
    }

    /// `status` of `call` on the element as a result.
    fn check(&self, call: &'static str, status: AXError) -> Result<(), Error> {
        match status {
            AXError::Success => Ok(()),
            AXError::InvalidUIElement => Err(Error::ElementGone(self.reference)),
            AXError::CannotComplete => Err(Error::ElementNotAnswering {
                element: self.reference,
                timeout: self.timeout,
            }),
            status => Err(platform(call, status)),
        }
    }

    /// The value of `name`, or `None` when the element has none.
    pub fn read(&self, name: &'static str) -> Result<Option<CFRetained<CFType>>, Error> {
        self.read_of(&self.element, name)
    }

    /// The value of `name` on `element`, which is the handle's element or
    /// another of its application, or `None` when it has none; failures
    /// are the handle's.
    fn read_of(
        &self,
        element: &AXUIElement,
        name: &'static str,
    ) -> Result<Option<CFRetained<CFType>>, Error> {
        let attribute = CFString::from_static_str(name);
        let mut value: *const CFType = ptr::null();
        // SAFETY: `value` is a valid out pointer; on success it holds a +1
        // reference the caller owns.
        let status = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
        match status {
            AXError::NoValue | AXError::AttributeUnsupported => Ok(None),
            status => {
                self.check("AXUIElementCopyAttributeValue", status)?;
                // SAFETY: the copied value is +1 and owned here.
                Ok(NonNull::new(value.cast_mut())
                    .map(|value| unsafe { CFRetained::from_raw(value) }))
            }
        }
    }

    /// Whether `name` can be set.
    pub fn settable(&self, name: &'static str) -> Result<bool, Error> {
        let attribute = CFString::from_static_str(name);
        let mut settable = 0;
        // SAFETY: `settable` is a valid out pointer.
        let status = unsafe {
            self.element
                .is_attribute_settable(&attribute, NonNull::from(&mut settable))
        };
        match status {
            AXError::NoValue | AXError::AttributeUnsupported => Ok(false),
            status => {
                self.check("AXUIElementIsAttributeSettable", status)?;
                Ok(settable != 0)
            }
        }
    }

    /// Sets `name` to `value`.
    pub fn set(&self, name: &'static str, value: &CFType) -> Result<(), Error> {
        let attribute = CFString::from_static_str(name);
        // SAFETY: the element is live; Accessibility checks the value's type.
        let status = unsafe { self.element.set_attribute_value(&attribute, value) };
        self.check("AXUIElementSetAttributeValue", status)
    }

    /// Whether the element advertises `action`.
    pub fn advertises(&self, action: &str) -> Result<bool, Error> {
        let mut names: *const CFArray = ptr::null();
        // SAFETY: `names` is a valid out pointer; on success it holds a +1
        // array the caller owns.
        let status = unsafe { self.element.copy_action_names(NonNull::from(&mut names)) };
        match status {
            AXError::NoValue | AXError::ActionUnsupported => Ok(false),
            status => {
                self.check("AXUIElementCopyActionNames", status)?;
                let Some(names) = NonNull::new(names.cast_mut()) else {
                    return Ok(false);
                };
                // SAFETY: the copied array is +1 and owned here, and holds
                // the action names as strings.
                let names: CFRetained<CFArray<CFString>> =
                    unsafe { CFRetained::from_raw(names.cast()) };
                Ok(names.iter().any(|name| name.to_string() == action))
            }
        }
    }

    /// Performs `action` on the element.
    pub fn perform(&self, action: &'static str) -> Result<(), Error> {
        let name = CFString::from_static_str(action);
        // SAFETY: the element is live.
        let status = unsafe { self.element.perform_action(&name) };
        self.check("AXUIElementPerformAction", status)
    }

    /// The element's frame in global display coordinates, when it reports
    /// one.
    pub fn frame(&self) -> Result<Option<CGRect>, Error> {
        let origin = self.read("AXPosition")?.and_then(|value| {
            let value = value.downcast::<AXValue>().ok()?;
            decode(&value, AXValueType::CGPoint, CGPoint::ZERO)
        });
        let size = self.read("AXSize")?.and_then(|value| {
            let value = value.downcast::<AXValue>().ok()?;
            decode(&value, AXValueType::CGSize, CGSize::ZERO)
        });
        Ok(origin
            .zip(size)
            .map(|(origin, size)| CGRect::new(origin, size)))
    }

    /// Whether the element takes input: false only when it reports itself
    /// disabled.
    pub fn enabled(&self) -> Result<bool, Error> {
        Ok(self
            .read("AXEnabled")?
            .and_then(|value| value.downcast::<CFBoolean>().ok())
            .is_none_or(|enabled| enabled.as_bool()))
    }

    /// Whether its application's focused element (`AXFocusedUIElement`) is
    /// the element or one of its descendants, found by walking up
    /// `AXParent` from the focused element. A compound control passes
    /// focus on to a child, which still counts as the control focused.
    pub fn holds_focus(&self) -> Result<bool, Error> {
        // SAFETY: any pid names an application element; a pid without one
        // fails the read below instead.
        let application = unsafe { AXUIElement::new_application(self.pid) };
        // SAFETY: `application` is a live application element.
        let status = unsafe { application.set_messaging_timeout(self.timeout.as_secs_f32()) };
        self.check("AXUIElementSetMessagingTimeout", status)?;
        let Some(mut current) = self
            .read_of(&application, FOCUSED_ELEMENT)?
            .and_then(|value| value.downcast::<AXUIElement>().ok())
        else {
            return Ok(false);
        };
        for _ in 0..MAX_ANCESTORS {
            if *current == *self.element {
                return Ok(true);
            }
            // SAFETY: `current` is a live element.
            let status = unsafe { current.set_messaging_timeout(self.timeout.as_secs_f32()) };
            self.check("AXUIElementSetMessagingTimeout", status)?;
            let Some(parent) = self
                .read_of(&current, "AXParent")?
                .and_then(|value| value.downcast::<AXUIElement>().ok())
            else {
                return Ok(false);
            };
            current = parent;
        }
        Ok(false)
    }

    /// Whether the element's value is text, or it has none.
    pub fn holds_text(&self) -> Result<bool, Error> {
        Ok(self
            .read(VALUE)?
            .is_none_or(|value| value.downcast_ref::<CFString>().is_some()))
    }

    /// The element's value when it is text.
    pub fn text(&self) -> Result<Option<String>, Error> {
        Ok(self
            .read(VALUE)?
            .and_then(|value| value.downcast::<CFString>().ok())
            .map(|text| text.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_reads_back_its_text_form() {
        let reference = ElementRef::new(0x0a3f_91c2, 12);
        assert_eq!(reference.to_string(), "0a3f91c2:12");
        assert_eq!("0a3f91c2:12".parse::<ElementRef>(), Ok(reference));
        assert_eq!(
            "FFFFFFFF:0".parse::<ElementRef>(),
            Ok(ElementRef::new(u32::MAX, 0))
        );
    }

    #[test]
    fn text_that_is_not_a_reference_is_refused() {
        for text in [
            "",
            "12",
            "0a3f91c2",
            "0a3f91c2:",
            "a3f91c2:12",
            "0a3f91c2a:12",
            "+a3f91c2:12",
            "0a3f91c2:+12",
            "0a3f91c2:-1",
            "0a3f91c2:4294967296",
            "0a3f91g2:12",
            "0a3f91c2:1:2",
        ] {
            assert!(
                text.parse::<ElementRef>().is_err(),
                "{text:?} parsed as a reference"
            );
        }
    }

    #[test]
    fn a_frame_is_relative_to_its_display() {
        let display = CGPoint::new(-1280.0, 100.0);
        let frame = Frame::on_display(
            CGRect::new(CGPoint::new(-1180.0, 250.0), CGSize::new(80.0, 20.0)),
            display,
        );
        assert_eq!(frame.origin(), Point::new(100.0, 150.0));
        assert_eq!(frame.center(), Point::new(140.0, 160.0));
        assert_eq!(frame.to_string(), "80x20 at (100, 150)");
    }

    fn handle(reference: ElementRef) -> Handle {
        // SAFETY: any pid names an application element; nothing is asked of
        // it here.
        let element = unsafe { AXUIElement::new_application(1) };
        Handle::new(element, reference, 1, 7, Duration::from_secs(1))
    }

    #[test]
    fn only_the_latest_listing_resolves() {
        let listing = Listing {
            id: 5,
            handles: vec![handle(ElementRef::new(5, 0)), handle(ElementRef::new(5, 1))],
        };
        let current = ElementRef::new(5, 1);
        let resolved = Listing::resolve(Some(&listing), current).expect("it is in the listing");
        assert_eq!(resolved.reference(), current);

        let past_the_end = ElementRef::new(5, 2);
        assert!(matches!(
            Listing::resolve(Some(&listing), past_the_end),
            Err(Error::UnknownElement { element, count: 2 }) if element == past_the_end
        ));

        let earlier = ElementRef::new(4, 0);
        assert!(matches!(
            Listing::resolve(Some(&listing), earlier),
            Err(Error::StaleElement { element, current: Some(5) }) if element == earlier
        ));
        assert!(matches!(
            Listing::resolve(None, current),
            Err(Error::StaleElement { element, current: None }) if element == current
        ));
    }
}
