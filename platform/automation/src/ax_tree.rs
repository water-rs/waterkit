// Copyright (c) 2026 Cua AI, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Derived from cua-driver (https://github.com/trycua/cua): the rule that makes
// an element addressable (it advertises an action, or a value control's
// AXValue is settable), the value-control roles, the actions every web
// element advertises, the default node budget, the one-round-trip read of a
// node's attributes, the Chromium accessibility opt-in (AXManualAccessibility,
// then AXEnhancedUserInterface) with its wait for a populated AXWebArea and
// its probe limits, and recording the opt-in per process lifetime (pid and
// start time); from libs/cua-driver/rust/crates/platform-macos/src/ax/
// tree.rs, batch.rs, enablement.rs and bindings.rs.

//! Walking a window's Accessibility tree for the elements an agent can act
//! on, and asking a Chromium-based application to build its tree.

use core::ptr::{self, NonNull};
use core::time::Duration;

use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize, Type,
};

use crate::SkippedRead;
use crate::accessibility::decode;
use crate::element::{Reported, VALUE};

/// How many nodes a listing visits by default, across every window:
/// cua-driver's `DEFAULT_MAX_ELEMENTS`, which covers realistic application
/// chrome and web pages while keeping a Chromium tree of thousands of nodes
/// bounded.
pub const DEFAULT_BUDGET: u32 = 2_000;

/// The attributes one round trip reads of every node.
const ATTRIBUTES: [&str; 10] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    VALUE,
    "AXPlaceholderValue",
    "AXEnabled",
    "AXPosition",
    "AXSize",
    "AXChildren",
];
const ROLE: usize = 0;
const SUBROLE: usize = 1;
const TITLE: usize = 2;
const DESCRIPTION: usize = 3;
const VALUE_SLOT: usize = 4;
const PLACEHOLDER: usize = 5;
const ENABLED: usize = 6;
const POSITION: usize = 7;
const SIZE: usize = 8;
const CHILDREN: usize = 9;

/// Actions every element of web content advertises, which say nothing about
/// whether it does anything (cua-driver's `UNLISTED_ACTIONS`).
const UNIVERSAL_ACTIONS: [&str; 2] = ["AXShowMenu", "AXScrollToVisible"];

/// Roles whose element is addressable by a settable value alone, such as
/// `Finder`'s inline rename field, which advertises no action (cua-driver's
/// `role_supports_value_addressing`).
const VALUE_ROLES: [&str; 7] = [
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXSlider",
    "AXStepper",
    "AXCheckBox",
    "AXRadioButton",
];

/// The role of a web page's root in Chromium and `WebKit`.
const WEB_AREA: &str = "AXWebArea";

/// How deep below a window, and through how many nodes, the probe for a
/// populated web area looks (cua-driver's `WEB_AREA_MAX_DEPTH`,
/// `WEB_AREA_PROBE_NODES`).
const PROBE_DEPTH: u32 = 10;
const PROBE_NODES: u32 = 400;

/// One addressable element a walk found.
#[derive(Debug)]
pub struct Found {
    pub element: CFRetained<AXUIElement>,
    pub reported: Reported,
}

/// What the status of an Accessibility call on one element says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It answered.
    Answered,
    /// The element has nothing there: no such attribute, value or action.
    Absent,
    /// The call failed for this element only, such as one gone since
    /// (`kAXErrorInvalidUIElement`) or one the application failed to read
    /// (`kAXErrorFailure`, the -25200 the Mac mini met on an action list).
    /// The walk skips what it could not read and goes on.
    NodeFailed,
    /// No call to the application can work: it does not answer within the
    /// messaging timeout (`kAXErrorCannotComplete`), or this process has no
    /// Accessibility access (`kAXErrorAPIDisabled`). The application's part
    /// of the walk ends there.
    Unreachable,
}

impl Outcome {
    /// The outcome a call's `status` means, as cua-driver's walk treats
    /// them: only an unanswered call stops it, and any other failure reads
    /// as nothing there.
    pub const fn of(status: AXError) -> Self {
        match status {
            AXError::Success => Self::Answered,
            AXError::NoValue
            | AXError::AttributeUnsupported
            | AXError::ActionUnsupported
            | AXError::NotImplemented => Self::Absent,
            AXError::CannotComplete | AXError::APIDisabled => Self::Unreachable,
            _ => Self::NodeFailed,
        }
    }
}

/// A call the application could not be reached by, which ends its part of
/// the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreachable {
    /// What was read.
    pub read: &'static str,
    pub status: AXError,
}

/// How a walk of one window ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walked {
    /// It visited every node.
    Complete,
    /// It ran out of budget.
    OutOfBudget,
    /// The application could not be reached any more.
    Unreachable(Unreachable),
}

/// Where a walk records the reads it skips.
pub struct Skips<'a> {
    pid: libc::pid_t,
    window: u32,
    skipped: &'a mut Vec<SkippedRead>,
}

impl<'a> Skips<'a> {
    pub const fn new(pid: libc::pid_t, window: u32, skipped: &'a mut Vec<SkippedRead>) -> Self {
        Self {
            pid,
            window,
            skipped,
        }
    }

    /// Records that reading `read` of a node with role `role` failed with
    /// `status`.
    fn record(&mut self, read: &'static str, status: AXError, role: Option<&str>) {
        tracing::debug!(
            pid = self.pid,
            window = self.window,
            role,
            read,
            ?status,
            "an Accessibility read failed; the walk skips it"
        );
        self.skipped.push(SkippedRead::new(
            self.pid,
            Some(self.window),
            role,
            read,
            status,
        ));
    }

    /// What `status`, of reading `read`, means for the walk: `Ok(true)` when
    /// it answered, `Ok(false)` when there is nothing to use, recorded when
    /// it failed, and `Err` when the application cannot be reached.
    fn answered(
        &mut self,
        read: &'static str,
        status: AXError,
        role: Option<&str>,
    ) -> Result<bool, Unreachable> {
        match Outcome::of(status) {
            Outcome::Answered => Ok(true),
            Outcome::Absent => Ok(false),
            Outcome::NodeFailed => {
                self.record(read, status, role);
                Ok(false)
            }
            Outcome::Unreachable => Err(Unreachable { read, status }),
        }
    }
}

/// The walk of windows' Accessibility trees for one listing.
pub struct Tree {
    names: CFRetained<CFArray<CFString>>,
    timeout: Duration,
}

impl Tree {
    /// A walk whose every call on an element waits at most `timeout` for its
    /// application.
    pub fn new(timeout: Duration) -> Self {
        let names = ATTRIBUTES.map(CFString::from_static_str);
        Self {
            names: CFArray::from_retained_objects(&names),
            timeout,
        }
    }

    /// Walks the tree below `window`, depth first in the order the
    /// application reports its children, adding each addressable element
    /// to `found` and spending one unit of `budget` on each node it visits,
    /// the window's own included. The window itself is not added: what it
    /// advertises (`AXRaise`) is no action an element takes, and each
    /// element names its window.
    ///
    /// A read a node fails is recorded in `skips` and the walk goes on: a
    /// node whose attributes cannot be read is left out with its subtree,
    /// one whose actions cannot be read counts as having none. Only an
    /// application that cannot be reached ends the walk.
    pub fn walk(
        &self,
        window: &AXUIElement,
        budget: &mut u32,
        found: &mut Vec<Found>,
        skips: &mut Skips<'_>,
    ) -> Walked {
        let mut stack = vec![window.retain()];
        let mut root = true;
        while let Some(node) = stack.pop() {
            let below_root = !core::mem::replace(&mut root, false);
            let Some(remaining) = budget.checked_sub(1) else {
                return Walked::OutOfBudget;
            };
            *budget = remaining;
            if let Err(unreachable) = self.visit(&node, below_root, found, &mut stack, skips) {
                return Walked::Unreachable(unreachable);
            }
        }
        Walked::Complete
    }

    /// Reads `node`, adds it to `found` when it is addressable and
    /// `listable`, and pushes its children onto `stack`.
    fn visit(
        &self,
        node: &AXUIElement,
        listable: bool,
        found: &mut Vec<Found>,
        stack: &mut Vec<CFRetained<AXUIElement>>,
        skips: &mut Skips<'_>,
    ) -> Result<(), Unreachable> {
        // Messaging timeouts are per element, not inherited from the
        // application's.
        // SAFETY: `node` is a live element.
        let status = unsafe { node.set_messaging_timeout(self.timeout.as_secs_f32()) };
        if status != AXError::Success {
            skips.record("messaging timeout", status, None);
            return Ok(());
        }
        let Some(values) = self.attributes(node, skips)? else {
            return Ok(());
        };
        let entry = |slot: usize| values[slot].clone();
        if let Some(role) = string(entry(ROLE)).filter(|_| listable)
            && addressable(node, &role, skips)?
        {
            found.push(Found {
                element: node.retain(),
                reported: Reported {
                    role,
                    subrole: string(entry(SUBROLE)),
                    title: string(entry(TITLE)),
                    label: string(entry(DESCRIPTION)),
                    value: value_text(entry(VALUE_SLOT)),
                    placeholder: string(entry(PLACEHOLDER)),
                    enabled: entry(ENABLED)
                        .and_then(|value| value.downcast::<CFBoolean>().ok())
                        .is_none_or(|enabled| enabled.as_bool()),
                    frame: frame(entry(POSITION), entry(SIZE)),
                },
            });
        }
        if let Some(children) = entry(CHILDREN).and_then(|value| value.downcast::<CFArray>().ok()) {
            // SAFETY: `AXChildren` is an array of elements.
            let children: &CFArray<AXUIElement> = unsafe { children.cast_unchecked() };
            // Reversed onto the stack, so the first child is walked first.
            let mut children: Vec<_> = children.iter().collect();
            children.reverse();
            stack.extend(children);
        }
        Ok(())
    }

    /// Every attribute of [`ATTRIBUTES`] of `node`, in their order, `None`
    /// for one it does not answer: in one round trip, or one attribute at a
    /// time when the application refuses the batched read, as cua-driver's
    /// walk falls back. `Ok(None)` when the node is gone.
    fn attributes(
        &self,
        node: &AXUIElement,
        skips: &mut Skips<'_>,
    ) -> Result<Option<[Option<CFRetained<CFType>>; ATTRIBUTES.len()]>, Unreachable> {
        let mut values: *const CFArray = ptr::null();
        // SAFETY: the names are strings and `values` is a valid out pointer;
        // on success it holds a +1 array the caller owns.
        let status = unsafe {
            node.copy_multiple_attribute_values(
                self.names.as_opaque(),
                AXCopyMultipleAttributeOptions::empty(),
                NonNull::from(&mut values),
            )
        };
        if status == AXError::Success
            && let Some(values) = NonNull::new(values.cast_mut())
        {
            // SAFETY: the copied array is +1 and owned here.
            let values: CFRetained<CFArray<CFType>> =
                unsafe { CFRetained::from_raw(values.cast()) };
            if values.len() == ATTRIBUTES.len() {
                return Ok(Some(core::array::from_fn(|slot| values.get(slot))));
            }
        }
        match Outcome::of(status) {
            Outcome::Unreachable => {
                return Err(Unreachable {
                    read: "attributes",
                    status,
                });
            }
            Outcome::NodeFailed if status == AXError::InvalidUIElement => {
                skips.record("attributes", status, None);
                return Ok(None);
            }
            _ => tracing::trace!(
                ?status,
                "the batched attribute read was refused; reading one at a time"
            ),
        }
        let mut role = None;
        let mut slots: [Option<CFRetained<CFType>>; ATTRIBUTES.len()] = Default::default();
        for (slot, name) in slots.iter_mut().zip(ATTRIBUTES) {
            let attribute = CFString::from_static_str(name);
            let mut value: *const CFType = ptr::null();
            // SAFETY: `value` is a valid out pointer; on success it holds a
            // +1 reference the caller owns.
            let status =
                unsafe { node.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
            if skips.answered(name, status, role.as_deref())? {
                // SAFETY: the copied value is +1 and owned here.
                *slot = NonNull::new(value.cast_mut())
                    .map(|value| unsafe { CFRetained::from_raw(value) });
            }
            if name == ATTRIBUTES[ROLE] {
                role = string(slot.clone());
            }
        }
        Ok(Some(slots))
    }
}

/// Whether `node`, of role `role`, is one an agent can act on: it advertises
/// an action beyond the ones every web element does, or it is a value
/// control whose value can be set. A read it fails counts as nothing there.
fn addressable(node: &AXUIElement, role: &str, skips: &mut Skips<'_>) -> Result<bool, Unreachable> {
    let mut names: *const CFArray = ptr::null();
    // SAFETY: `names` is a valid out pointer; on success it holds a +1 array
    // the caller owns.
    let status = unsafe { node.copy_action_names(NonNull::from(&mut names)) };
    let acts = skips.answered("actions", status, Some(role))?
        && NonNull::new(names.cast_mut()).is_some_and(|names| {
            // SAFETY: the copied array is +1 and owned here, and holds the
            // action names as strings.
            let names: CFRetained<CFArray<CFString>> =
                unsafe { CFRetained::from_raw(names.cast()) };
            names
                .iter()
                .any(|name| !UNIVERSAL_ACTIONS.contains(&name.to_string().as_str()))
        });
    if acts || !VALUE_ROLES.contains(&role) {
        return Ok(acts);
    }
    let attribute = CFString::from_static_str(VALUE);
    let mut settable = 0;
    // SAFETY: `settable` is a valid out pointer.
    let status = unsafe { node.is_attribute_settable(&attribute, NonNull::from(&mut settable)) };
    Ok(skips.answered("AXValue settable", status, Some(role))? && settable != 0)
}

/// A string attribute's text, trimmed; `None` when it is not a string or
/// holds only white space.
fn string(value: Option<CFRetained<CFType>>) -> Option<String> {
    let text = value?.downcast::<CFString>().ok()?.to_string();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// A value as text: a string as it is, a number in decimal.
fn value_text(value: Option<CFRetained<CFType>>) -> Option<String> {
    let value = value?;
    match value.downcast::<CFNumber>() {
        Ok(number) => number.as_f64().map(|number| number.to_string()),
        Err(value) => value
            .downcast::<CFString>()
            .ok()
            .map(|text| text.to_string())
            .filter(|text| !text.is_empty()),
    }
}

/// The frame `AXPosition` and `AXSize` give, in global display
/// coordinates.
fn frame(position: Option<CFRetained<CFType>>, size: Option<CFRetained<CFType>>) -> Option<CGRect> {
    let position = position?.downcast::<AXValue>().ok()?;
    let size = size?.downcast::<AXValue>().ok()?;
    Some(CGRect::new(
        decode(&position, AXValueType::CGPoint, CGPoint::ZERO)?,
        decode(&size, AXValueType::CGSize, CGSize::ZERO)?,
    ))
}

/// What an application made of the request to build its Accessibility
/// tree for assistive applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptIn {
    /// It took the request: a Chromium-based application, which builds its
    /// web content's tree only on that request, and does so asynchronously.
    Accepted,
    /// It has no such request to take, as a native application has not.
    Refused,
    /// It did not answer in time; asking again later may work.
    Unanswered,
}

/// Asks the application `app` to build its tree: `AXManualAccessibility`,
/// which Chromium and Electron take without the side effects of
/// `AXEnhancedUserInterface`, and that only when the first is unsupported.
pub fn opt_in(app: &AXUIElement) -> OptIn {
    match set_true(app, "AXManualAccessibility") {
        AXError::Success => OptIn::Accepted,
        AXError::CannotComplete => OptIn::Unanswered,
        AXError::AttributeUnsupported => match set_true(app, "AXEnhancedUserInterface") {
            AXError::Success => OptIn::Accepted,
            AXError::CannotComplete => OptIn::Unanswered,
            _ => OptIn::Refused,
        },
        _ => OptIn::Refused,
    }
}

fn set_true(element: &AXUIElement, name: &'static str) -> AXError {
    let attribute = CFString::from_static_str(name);
    // SAFETY: the element is live and the attribute takes a boolean.
    unsafe { element.set_attribute_value(&attribute, CFBoolean::new(true)) }
}

/// Whether a web area with content (an `AXWebArea` with children) lies
/// within [`PROBE_DEPTH`] levels of `window`, looking at no more than
/// [`PROBE_NODES`] nodes. An `AXWebArea` without children is the stub
/// Chromium shows before it has built the page's tree. A node that does not
/// answer counts as no content yet.
pub fn holds_web_content(window: &AXUIElement) -> bool {
    let mut visits = PROBE_NODES;
    let mut stack = vec![(window.retain(), 0)];
    while let Some((node, depth)) = stack.pop() {
        let Some(remaining) = visits.checked_sub(1) else {
            return false;
        };
        visits = remaining;
        let children = copy(&node, "AXChildren")
            .and_then(|value| value.downcast::<CFArray>().ok())
            .map_or_else(Vec::new, |children| {
                // SAFETY: `AXChildren` is an array of elements.
                let children: &CFArray<AXUIElement> = unsafe { children.cast_unchecked() };
                children.iter().collect::<Vec<_>>()
            });
        if string(copy(&node, "AXRole")).as_deref() == Some(WEB_AREA) {
            if !children.is_empty() {
                return true;
            }
        } else if depth < PROBE_DEPTH {
            stack.extend(children.into_iter().map(|child| (child, depth + 1)));
        }
    }
    false
}

/// The value of `name` on `element`, or `None` when it has none or does not
/// answer.
fn copy(element: &AXUIElement, name: &'static str) -> Option<CFRetained<CFType>> {
    let attribute = CFString::from_static_str(name);
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out pointer; on success it holds a +1
    // reference the caller owns.
    let status = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
    // SAFETY: the copied value is +1 and owned here.
    (status == AXError::Success)
        .then(|| NonNull::new(value.cast_mut()).map(|value| unsafe { CFRetained::from_raw(value) }))
        .flatten()
}

/// One lifetime of a process: its pid and its start time, since pids are
/// reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    pid: libc::pid_t,
    started: (u64, u64),
}

impl Process {
    /// The process `pid` now; `None` when it has exited.
    pub fn of(pid: libc::pid_t) -> Option<Self> {
        // SAFETY: an all-zero `proc_bsdinfo` is a valid value of the C
        // structure.
        let mut info: libc::proc_bsdinfo = unsafe { core::mem::zeroed() };
        let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).expect("the structure is small");
        // SAFETY: `info` is writable for `size` bytes.
        let filled = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        (filled == size).then_some(Self {
            pid,
            started: (info.pbi_start_tvsec, info.pbi_start_tvusec),
        })
    }

    pub const fn pid(&self) -> libc::pid_t {
        self.pid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_one_lifetime() {
        let pid = libc::pid_t::try_from(std::process::id()).expect("pids fit in pid_t");
        let now = Process::of(pid).expect("this process is running");
        assert_eq!(Process::of(pid), Some(now));
        assert_ne!(now.started, (0, 0));
    }

    #[test]
    fn values_read_as_text() {
        let text = |value: CFRetained<CFType>| value_text(Some(value));
        assert_eq!(
            text(CFString::from_str("Ada 艾达 エイダ 에이다").into()).as_deref(),
            Some("Ada 艾达 エイダ 에이다")
        );
        assert_eq!(text(CFNumber::new_i32(1).into()).as_deref(), Some("1"));
        assert_eq!(text(CFNumber::new_f64(0.5).into()).as_deref(), Some("0.5"));
        assert_eq!(text(CFString::from_str("").into()), None);
        assert_eq!(
            string(Some(CFString::from_str("  Submit ").into())).as_deref(),
            Some("Submit")
        );
        assert_eq!(string(Some(CFString::from_str(" \n").into())), None);
    }

    /// Built over this process's own application element, which answers no
    /// attribute without an `AppKit` application; nothing is shown.
    #[test]
    fn a_walk_spends_its_budget() {
        let pid = libc::pid_t::try_from(std::process::id()).expect("pids fit in pid_t");
        // SAFETY: any pid names an application element.
        let app = unsafe { AXUIElement::new_application(pid) };
        let tree = Tree::new(Duration::from_secs(1));
        let mut found = Vec::new();
        let mut skipped = Vec::new();
        let mut budget = 0;
        let walked = tree.walk(
            &app,
            &mut budget,
            &mut found,
            &mut Skips::new(pid, 0, &mut skipped),
        );
        assert_eq!(walked, Walked::OutOfBudget);
        assert!(found.is_empty());
        assert_eq!(skipped, []);
    }

    /// The statuses the Mac mini's listings met: -25200 on an action list
    /// and -25201 on a window lookup are failures of one node; only an
    /// unanswered call or missing Accessibility access ends an application's
    /// walk.
    #[test]
    fn only_an_unreachable_application_ends_its_walk() {
        assert_eq!(Outcome::of(AXError::Success), Outcome::Answered);
        for absent in [
            AXError::NoValue,
            AXError::AttributeUnsupported,
            AXError::ActionUnsupported,
            AXError::NotImplemented,
        ] {
            assert_eq!(Outcome::of(absent), Outcome::Absent, "{absent:?}");
        }
        for failed in [
            AXError::Failure,
            AXError(-25200),
            AXError::IllegalArgument,
            AXError::InvalidUIElement,
            AXError::NotEnoughPrecision,
        ] {
            assert_eq!(Outcome::of(failed), Outcome::NodeFailed, "{failed:?}");
        }
        for cut_off in [AXError::CannotComplete, AXError::APIDisabled] {
            assert_eq!(Outcome::of(cut_off), Outcome::Unreachable, "{cut_off:?}");
        }
    }

    #[test]
    fn a_failed_read_is_recorded_and_an_unreachable_one_is_not() {
        let mut skipped = Vec::new();
        let mut skips = Skips::new(42, 7, &mut skipped);
        assert_eq!(skips.answered("actions", AXError::Success, None), Ok(true));
        assert_eq!(
            skips.answered("actions", AXError::ActionUnsupported, None),
            Ok(false)
        );
        assert_eq!(
            skips.answered("actions", AXError::Failure, Some("AXButton")),
            Ok(false)
        );
        assert_eq!(
            skips.answered("AXChildren", AXError::CannotComplete, None),
            Err(Unreachable {
                read: "AXChildren",
                status: AXError::CannotComplete
            })
        );
        assert_eq!(
            skipped,
            [SkippedRead::new(
                42,
                Some(7),
                Some("AXButton"),
                "actions",
                AXError::Failure
            )]
        );
        assert_eq!(
            skipped[0].to_string(),
            "pid 42, window 7, AXButton: reading actions failed with AXError -25200"
        );
    }
}
