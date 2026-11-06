//! Virtual displays: private-API `CGVirtualDisplay`s.

use core::ffi::{CStr, c_char, c_void};
use core::num::NonZeroU32;
use core::ptr;
use core::time::Duration;
use std::sync::mpsc;
use std::time::Instant;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Method};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRunLoop, CFRunLoopTimer, CGSize, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration,
    CGConfigureDisplayMirrorOfDisplay, CGConfigureOption, CGDirectDisplayID,
    CGDisplayChangeSummaryFlags, CGDisplayConfigRef, CGDisplayRegisterReconfigurationCallback,
    CGDisplayRemoveReconfigurationCallback, CGError, kCGNullDirectDisplay,
};
use objc2_foundation::{NSArray, NSNumber, NSString};
use waterkit_core::RefreshRate;

use crate::error::platform;
use crate::skylight::{DisplayLists, SkyLight};
use crate::topology::Presence;
use crate::{DisplayState, Error};

/// Vendor id the displays carry: outside the ids assigned to real vendors.
const VENDOR_ID: u32 = 0xF0F0;
/// Product id the displays carry.
const PRODUCT_ID: u32 = 0xF0F1;
/// Panel density the physical size is derived from, in points per inch: a
/// typical desktop panel's.
const POINTS_PER_INCH: f64 = 110.0;
const MILLIMETRES_PER_INCH: f64 = 25.4;

/// The encoding of `-[SLWindowMirroringManager extend:]` the call is written
/// against: returns `BOOL`, takes one object.
const EXTEND_ENCODING: &CStr = c"B24@0:8@16";

/// The default for [`VirtualDisplayConfig::online_timeout`].
const DEFAULT_ONLINE_TIMEOUT: Duration = Duration::from_secs(3);

/// How many pixels back one point of a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackingScale {
    /// One pixel per point.
    X1,
    /// Two pixels per point in each direction (`HiDPI`).
    X2,
}

impl BackingScale {
    /// Pixels per point in each direction.
    #[must_use]
    pub const fn factor(self) -> u32 {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
        }
    }
}

/// What a [`VirtualDisplay`] looks like.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualDisplayConfig {
    name: String,
    width: NonZeroU32,
    height: NonZeroU32,
    serial: u32,
    scale: BackingScale,
    refresh_rate: RefreshRate,
    online_timeout: Duration,
}

impl VirtualDisplayConfig {
    /// A display called `name`, `width`x`height` points at 2x backing scale
    /// and 60 Hz.
    ///
    /// The system keeps a display's arrangement and settings by its serial
    /// number: reusing `serial` brings a display back where it was, a new one
    /// introduces a display the system has not seen.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        width: NonZeroU32,
        height: NonZeroU32,
        serial: u32,
    ) -> Self {
        Self {
            name: name.into(),
            width,
            height,
            serial,
            scale: BackingScale::X2,
            refresh_rate: RefreshRate::new_unchecked(60.0),
            online_timeout: DEFAULT_ONLINE_TIMEOUT,
        }
    }

    /// The backing scale.
    #[must_use]
    pub const fn scale(mut self, scale: BackingScale) -> Self {
        self.scale = scale;
        self
    }

    /// The refresh rate.
    #[must_use]
    pub const fn refresh_rate(mut self, refresh_rate: RefreshRate) -> Self {
        self.refresh_rate = refresh_rate;
        self
    }

    /// How long each step of bringing the display up may take: the display
    /// appearing, and, when they are needed, extending it into the topology
    /// and taking it out of a mirror set. Three seconds by default.
    #[must_use]
    pub const fn online_timeout(mut self, timeout: Duration) -> Self {
        self.online_timeout = timeout;
        self
    }

    /// The size in pixels: the points times the backing scale.
    fn pixels(&self) -> Result<(NonZeroU32, NonZeroU32), Error> {
        let scale = NonZeroU32::new(self.scale.factor()).expect("backing scales are non-zero");
        let overflow = || Error::SizeOverflow {
            width: self.width.get(),
            height: self.height.get(),
            scale: scale.get(),
        };
        Ok((
            self.width.checked_mul(scale).ok_or_else(overflow)?,
            self.height.checked_mul(scale).ok_or_else(overflow)?,
        ))
    }
}

/// A display on none of the user's monitors, removed when dropped.
///
/// Created and dropped on the main thread; see the crate documentation for
/// how creation services the main run loop.
#[derive(Debug)]
pub struct VirtualDisplay {
    _display: Retained<AnyObject>,
    id: CGDirectDisplayID,
    width: NonZeroU32,
    height: NonZeroU32,
    scale: BackingScale,
}

impl VirtualDisplay {
    /// Creates the display and waits until it is online, active and outside
    /// any mirror set.
    ///
    /// # Errors
    ///
    /// - [`Error::Unavailable`] or [`Error::SignatureMismatch`] when the
    ///   private API this needs is missing or changed;
    /// - [`Error::SizeOverflow`] when the size in pixels does not fit in 32
    ///   bits;
    /// - [`Error::Refused`] when `CoreGraphics` refuses the display;
    /// - [`Error::ExtendRefused`] when the display stays out of the topology
    ///   and `SLWindowMirroringManager` refuses to bring it in;
    /// - [`Error::NotReady`] when the display is still not online, active
    ///   and unmirrored after the timeouts;
    /// - [`Error::Platform`] when a display configuration call fails.
    pub fn new(config: &VirtualDisplayConfig, _mtm: MainThreadMarker) -> Result<Self, Error> {
        let skylight = SkyLight::open()?;
        let lists = DisplayLists::resolve(&skylight)?;
        let classes = Classes::get()?;
        let (pixels_wide, pixels_high) = config.pixels()?;

        // Registered before the display exists: it is added asynchronously
        // after `applySettings:`.
        let mut changes = Reconfigurations::watch()?;
        let display = autoreleasepool(|_| classes.create(config, pixels_wide, pixels_high))?;
        // SAFETY: `displayID` takes nothing and returns the display's id.
        let id: CGDirectDisplayID = unsafe { msg_send![&display, displayID] };
        if id == kCGNullDirectDisplay {
            return Err(Error::Refused("displayID is null"));
        }

        let timeout = config.online_timeout;
        changes.wait(timeout, |changes| {
            Ok(changes.added(id) || Presence::of(id)?.online)
        })?;
        let mut extended = false;
        if !Presence::of(id)?.online
            && lists.registered()?.contains(&id)
            && !lists.online()?.contains(&id)
        {
            extend(id)?;
            extended = true;
            changes.wait(timeout, |_| Ok(Presence::of(id)?.online))?;
        }
        if Presence::of(id)?.mirrored {
            unmirror(id)?;
            changes.wait(timeout, |_| Ok(!Presence::of(id)?.mirrored))?;
        }

        let presence = Presence::of(id)?;
        if presence.ready() {
            tracing::debug!(id, extended, "virtual display online");
            return Ok(Self {
                _display: display,
                id,
                width: config.width,
                height: config.height,
                scale: config.scale,
            });
        }
        let state = DisplayState::new(
            id,
            changes.added(id),
            lists.registered()?.contains(&id),
            lists.online()?.contains(&id),
            presence.online,
            presence.active,
            presence.mirrored,
            extended,
        );
        Err(Error::NotReady(state))
    }

    /// The display's `CGDirectDisplayID`.
    #[must_use]
    pub const fn id(&self) -> CGDirectDisplayID {
        self.id
    }

    /// Width in points.
    #[must_use]
    pub const fn width(&self) -> NonZeroU32 {
        self.width
    }

    /// Height in points.
    #[must_use]
    pub const fn height(&self) -> NonZeroU32 {
        self.height
    }

    /// The backing scale.
    #[must_use]
    pub const fn scale(&self) -> BackingScale {
        self.scale
    }
}

/// The private `CoreGraphics` classes a display is built from.
struct Classes {
    descriptor: &'static AnyClass,
    display: &'static AnyClass,
    settings: &'static AnyClass,
    mode: &'static AnyClass,
}

fn class(name: &'static CStr) -> Result<&'static AnyClass, Error> {
    AnyClass::get(name)
        .ok_or_else(|| Error::Unavailable(name.to_str().expect("class names are ASCII")))
}

impl Classes {
    fn get() -> Result<Self, Error> {
        Ok(Self {
            descriptor: class(c"CGVirtualDisplayDescriptor")?,
            display: class(c"CGVirtualDisplay")?,
            settings: class(c"CGVirtualDisplaySettings")?,
            mode: class(c"CGVirtualDisplayMode")?,
        })
    }

    /// Builds the descriptor, creates the display and applies its one mode.
    fn create(
        &self,
        config: &VirtualDisplayConfig,
        pixels_wide: NonZeroU32,
        pixels_high: NonZeroU32,
    ) -> Result<Retained<AnyObject>, Error> {
        let millimetres = CGSize::new(
            f64::from(config.width.get()) / POINTS_PER_INCH * MILLIMETRES_PER_INCH,
            f64::from(config.height.get()) / POINTS_PER_INCH * MILLIMETRES_PER_INCH,
        );
        let name = NSString::from_str(&config.name);
        let logged_name = config.name.clone();
        let termination = RcBlock::new(move |_: *mut AnyObject, _: *mut AnyObject| {
            tracing::warn!(display = %logged_name, "the system terminated the virtual display");
        });
        let queue = DispatchQueue::main();
        // SAFETY: every message matches the private classes' selectors and
        // argument types; every object is owned by a `Retained`; the main
        // queue is an `OS_dispatch_queue` object that outlives the display.
        unsafe {
            let descriptor: Retained<AnyObject> = msg_send![self.descriptor, new];
            let queue: &AnyObject = &*(&raw const *queue).cast::<AnyObject>();
            let _: () = msg_send![&descriptor, setQueue: queue];
            let _: () = msg_send![&descriptor, setName: &*name];
            let _: () = msg_send![&descriptor, setMaxPixelsWide: pixels_wide.get()];
            let _: () = msg_send![&descriptor, setMaxPixelsHigh: pixels_high.get()];
            let _: () = msg_send![&descriptor, setSizeInMillimeters: millimetres];
            let _: () = msg_send![&descriptor, setVendorID: VENDOR_ID];
            let _: () = msg_send![&descriptor, setProductID: PRODUCT_ID];
            let _: () = msg_send![&descriptor, setSerialNum: config.serial];
            let _: () = msg_send![&descriptor, setTerminationHandler: &*termination];

            let allocated: Allocated<AnyObject> = msg_send![self.display, alloc];
            let display: Option<Retained<AnyObject>> =
                msg_send![allocated, initWithDescriptor: &*descriptor];
            let display = display.ok_or(Error::Refused("initWithDescriptor: returned nil"))?;

            // The mode is in points; its pixels are the points times the
            // backing scale, the descriptor's maximum.
            let settings: Retained<AnyObject> = msg_send![self.settings, new];
            let hidpi = u32::from(config.scale == BackingScale::X2);
            let _: () = msg_send![&settings, setHiDPI: hidpi];
            let allocated: Allocated<AnyObject> = msg_send![self.mode, alloc];
            let mode: Option<Retained<AnyObject>> = msg_send![
                allocated,
                initWithWidth: config.width.get(),
                height: config.height.get(),
                refreshRate: f64::from(config.refresh_rate.get())
            ];
            let mode = mode.ok_or(Error::Refused(
                "initWithWidth:height:refreshRate: returned nil",
            ))?;
            let modes = NSArray::from_retained_slice(&[mode]);
            let _: () = msg_send![&settings, setModes: &*modes];
            let applied: bool = msg_send![&display, applySettings: &*settings];
            if !applied {
                return Err(Error::Refused("applySettings: returned NO"));
            }
            Ok(display)
        }
    }
}

unsafe extern "C" {
    fn method_getTypeEncoding(method: *const Method) -> *const c_char;
}

/// `-[SLWindowMirroringManager extend:]`: the one call that brings a display
/// `WindowServer` registered but left offline into the topology.
fn extend(id: CGDirectDisplayID) -> Result<(), Error> {
    const METHOD: &str = "-[SLWindowMirroringManager extend:]";
    let manager_class = class(c"SLWindowMirroringManager")?;
    let method = manager_class
        .instance_method(sel!(extend:))
        .ok_or(Error::Unavailable(METHOD))?;
    // SAFETY: `method` is a live method of the class; the runtime owns the
    // NUL-terminated encoding.
    let encoding = unsafe { CStr::from_ptr(method_getTypeEncoding(ptr::from_ref(method))) };
    if encoding != EXTEND_ENCODING {
        return Err(Error::SignatureMismatch {
            method: METHOD,
            found: encoding.to_string_lossy().into_owned(),
            expected: EXTEND_ENCODING.to_str().expect("the encoding is ASCII"),
        });
    }
    if manager_class
        .metaclass()
        .instance_method(sel!(shared))
        .is_none()
    {
        return Err(Error::Unavailable("+[SLWindowMirroringManager shared]"));
    }
    let number = NSNumber::new_u32(id);
    let extended = autoreleasepool(|_| {
        // SAFETY: `+shared` takes nothing and returns the manager; `extend:`
        // matches the checked encoding: it returns `BOOL` and takes the id
        // boxed as an object.
        let manager: Option<Retained<AnyObject>> = unsafe { msg_send![manager_class, shared] };
        let manager = manager.ok_or(Error::Refused(
            "+[SLWindowMirroringManager shared] returned nil",
        ))?;
        let extended: bool = unsafe { msg_send![&manager, extend: &*number] };
        Ok::<_, Error>(extended)
    })?;
    if extended {
        Ok(())
    } else {
        Err(Error::ExtendRefused(id))
    }
}

/// Takes the display out of its mirror set for the login session; virtual
/// displays refuse a permanent configuration.
fn unmirror(id: CGDirectDisplayID) -> Result<(), Error> {
    let mut configuration: CGDisplayConfigRef = ptr::null_mut();
    // SAFETY: `configuration` is a valid out pointer.
    let status = unsafe { CGBeginDisplayConfiguration(&raw mut configuration) };
    if status != CGError::Success {
        return Err(platform("CGBeginDisplayConfiguration", status));
    }
    // SAFETY: `configuration` was just begun.
    let status =
        unsafe { CGConfigureDisplayMirrorOfDisplay(configuration, id, kCGNullDirectDisplay) };
    if status != CGError::Success {
        // SAFETY: the configuration is begun and not completed.
        unsafe { CGCancelDisplayConfiguration(configuration) };
        return Err(platform("CGConfigureDisplayMirrorOfDisplay", status));
    }
    // SAFETY: `configuration` is begun; completing consumes it.
    let status =
        unsafe { CGCompleteDisplayConfiguration(configuration, CGConfigureOption::ForSession) };
    if status == CGError::Success {
        Ok(())
    } else {
        Err(platform("CGCompleteDisplayConfiguration", status))
    }
}

/// The displays `CoreGraphics` reports added while registered, delivered on
/// the main run loop.
struct Reconfigurations {
    sender: Box<mpsc::Sender<CGDirectDisplayID>>,
    receiver: mpsc::Receiver<CGDirectDisplayID>,
    added: Vec<CGDirectDisplayID>,
}

unsafe extern "C-unwind" fn on_reconfiguration(
    display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    sender: *mut c_void,
) {
    if flags.contains(CGDisplayChangeSummaryFlags::AddFlag) {
        // SAFETY: `sender` is the boxed sender `Reconfigurations` registered,
        // which outlives its registration.
        let sender = unsafe { &*sender.cast::<mpsc::Sender<CGDirectDisplayID>>() };
        sender
            .send(display)
            .expect("the receiver outlives the callback's registration");
    }
    // Every callback ends the current wait so it re-checks the display.
    CFRunLoop::main()
        .expect("the process has a main run loop")
        .stop();
}

impl Reconfigurations {
    fn watch() -> Result<Self, Error> {
        let (sender, receiver) = mpsc::channel();
        let sender = Box::new(sender);
        let info = (&raw const *sender).cast_mut().cast::<c_void>();
        // SAFETY: the callback matches the registered signature and `info`
        // stays valid until `drop` removes the registration.
        let status =
            unsafe { CGDisplayRegisterReconfigurationCallback(Some(on_reconfiguration), info) };
        if status != CGError::Success {
            return Err(platform("CGDisplayRegisterReconfigurationCallback", status));
        }
        Ok(Self {
            sender,
            receiver,
            added: Vec::new(),
        })
    }

    /// Whether a callback reported `id` added.
    fn added(&mut self, id: CGDirectDisplayID) -> bool {
        self.added.extend(self.receiver.try_iter());
        self.added.contains(&id)
    }

    /// Spins the main run loop until `done` holds or `timeout` passes; the
    /// caller then reads the state it waited for. Each reconfiguration
    /// callback, and each other source the loop handles, ends one spin so
    /// `done` is re-checked.
    fn wait(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&mut Self) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        let main = CFRunLoop::main().expect("the process has a main run loop");
        // A mode with no source or timer makes the loop return at once; the
        // timer keeps it waiting until the deadline.
        // SAFETY: the timer has no callout and no context.
        let timer = unsafe {
            CFRunLoopTimer::new(
                None,
                CFAbsoluteTimeGetCurrent() + timeout.as_secs_f64(),
                0.0,
                0,
                0,
                None,
                ptr::null_mut(),
            )
        }
        .expect("CoreFoundation creates a timer");
        // SAFETY: the default mode is an immutable constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        main.add_timer(Some(&timer), mode);
        let outcome = loop {
            match done(self) {
                Ok(false) => {}
                finished => break finished.map(drop),
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Ok(());
            }
            CFRunLoop::run_in_mode(mode, remaining.as_secs_f64(), true);
        };
        timer.invalidate();
        outcome
    }
}

impl Drop for Reconfigurations {
    fn drop(&mut self) {
        let info = (&raw const *self.sender).cast_mut().cast::<c_void>();
        // SAFETY: removes the registration `watch` made, with the same
        // callback and pointer.
        let status =
            unsafe { CGDisplayRemoveReconfigurationCallback(Some(on_reconfiguration), info) };
        assert!(
            status == CGError::Success || std::thread::panicking(),
            "removing the display reconfiguration callback failed: {status:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The private classes and `extend:` exist with the shapes this module
    /// calls. Looking classes and methods up creates and shows nothing.
    #[test]
    fn the_private_display_api_has_the_expected_shape() {
        let _skylight = SkyLight::open().expect("SkyLight opens");
        Classes::get().expect("the CGVirtualDisplay classes exist");
        let manager = class(c"SLWindowMirroringManager").expect("the mirroring manager exists");
        let method = manager
            .instance_method(sel!(extend:))
            .expect("the manager has extend:");
        // SAFETY: as in `extend`.
        let encoding = unsafe { CStr::from_ptr(method_getTypeEncoding(ptr::from_ref(method))) };
        assert_eq!(encoding, EXTEND_ENCODING);
    }

    fn config(width: u32, height: u32) -> VirtualDisplayConfig {
        VirtualDisplayConfig::new(
            "test",
            NonZeroU32::new(width).expect("non-zero"),
            NonZeroU32::new(height).expect("non-zero"),
            1,
        )
    }

    fn pixels(config: &VirtualDisplayConfig) -> Option<(u32, u32)> {
        config
            .pixels()
            .ok()
            .map(|(wide, high)| (wide.get(), high.get()))
    }

    #[test]
    fn pixels_are_points_times_the_backing_scale() {
        assert_eq!(pixels(&config(1280, 800)), Some((2560, 1600)));
        assert_eq!(
            pixels(&config(1280, 800).scale(BackingScale::X1)),
            Some((1280, 800))
        );
    }

    #[test]
    fn a_size_beyond_32_bit_pixels_is_refused() {
        assert!(matches!(
            config(u32::MAX, 800).pixels(),
            Err(Error::SizeOverflow { scale: 2, .. })
        ));
    }
}
