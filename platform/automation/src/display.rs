//! Virtual displays: private-API `CGVirtualDisplay`s.

use core::ffi::{CStr, c_char, c_void};
use core::num::NonZeroU32;
use core::ptr;
use core::time::Duration;
use std::sync::mpsc;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Method};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_core_foundation::{CFRunLoop, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration,
    CGConfigureDisplayMirrorOfDisplay, CGConfigureDisplayOrigin, CGConfigureOption,
    CGDirectDisplayID, CGDisplayBounds, CGDisplayChangeSummaryFlags, CGDisplayConfigRef,
    CGDisplayRegisterReconfigurationCallback, CGDisplayRemoveReconfigurationCallback, CGError,
    kCGNullDirectDisplay,
};
use objc2_foundation::{NSArray, NSNumber, NSString};
use waterkit_core::RefreshRate;

use crate::error::platform;
use crate::framework::Framework;
use crate::run_loop;
use crate::skylight::DisplayLists;
use crate::topology::{self, Presence};
use crate::{DisplayState, Error, Rect};

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
    /// The system keeps a display's settings by its serial number: reusing
    /// `serial` brings a display back with them, a new one introduces a
    /// display the system has not seen. Wherever the system would arrange it,
    /// [`VirtualDisplay::new`] places it itself.
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
    /// appearing; extending it into the topology when that is needed; a
    /// display reconfiguration already under way finishing; and the display
    /// coming to rest where it is placed. Three seconds by default.
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
    /// Creates the display and waits until it is online, active, outside any
    /// mirror set and at rest where it is placed.
    ///
    /// Once the display is online and no display reconfiguration is under
    /// way, it is placed for the login session at the right edge of the other
    /// active displays, top-aligned with the rightmost of them, and taken out
    /// of its mirror set when it is in one. That origin overlaps no display
    /// and leaves no gap, the two things a configuration adjusts, so the
    /// configuration keeps it there and every other display in place. `new`
    /// returns once that configuration has finished, with no other under
    /// way, and `CoreGraphics` reports the display at that origin with its
    /// size in points.
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
    /// - [`Error::Reconfiguring`] when a display reconfiguration is still
    ///   under way after the timeout;
    /// - [`Error::Unsettled`] when the display is not at the frame it was
    ///   placed at after the timeout;
    /// - [`Error::Platform`] when a display configuration call fails.
    pub fn new(config: &VirtualDisplayConfig, _mtm: MainThreadMarker) -> Result<Self, Error> {
        let skylight = Framework::skylight()?;
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
        let presence = Presence::of(id)?;
        if !presence.online {
            return Err(not_ready(id, &mut changes, &lists, presence, extended)?);
        }

        // Between a reconfiguration's begin and end callbacks this process
        // still reports the arrangement before it, which the placement is
        // computed from.
        if !changes.wait(timeout, |changes| Ok(!changes.configuring()))? {
            return Err(Error::Reconfiguring {
                display: id,
                timeout,
            });
        }
        let others = topology::active()?
            .into_iter()
            .filter(|&other| other != id)
            .map(|other| CGDisplayBounds(other));
        let frame = CGRect::new(
            beside(others),
            CGSize::new(
                f64::from(config.width.get()),
                f64::from(config.height.get()),
            ),
        );
        arrange(id, Presence::of(id)?.mirrored, frame.origin)?;
        let settled = changes.wait(timeout, |changes| {
            Ok(!changes.configuring() && Presence::of(id)?.ready() && CGDisplayBounds(id) == frame)
        })?;

        if settled {
            tracing::debug!(id, extended, origin = ?frame.origin, "virtual display online");
            return Ok(Self {
                _display: display,
                id,
                width: config.width,
                height: config.height,
                scale: config.scale,
            });
        }
        if changes.configuring() {
            return Err(Error::Reconfiguring {
                display: id,
                timeout,
            });
        }
        let presence = Presence::of(id)?;
        if !presence.ready() {
            return Err(not_ready(id, &mut changes, &lists, presence, extended)?);
        }
        Err(Error::Unsettled {
            display: id,
            placed: Rect::from_cg(frame),
            bounds: Rect::from_cg(CGDisplayBounds(id)),
        })
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

/// The [`Error::NotReady`] for display `id`, with the state observed of it.
fn not_ready(
    id: CGDirectDisplayID,
    changes: &mut Reconfigurations,
    lists: &DisplayLists,
    presence: Presence,
    extended: bool,
) -> Result<Error, Error> {
    Ok(Error::NotReady(DisplayState::new(
        id,
        changes.added(id),
        lists.registered()?.contains(&id),
        lists.online()?.contains(&id),
        presence.online,
        presence.active,
        presence.mirrored,
        extended,
    )))
}

/// The origin, in whole points, that puts a display at the right edge of the
/// displays with bounds `others`, top-aligned with the rightmost of them, or
/// the topmost of those when several end at that edge: it overlaps none of
/// them and shares an edge with that one, so it leaves no gap. The global
/// origin when there are none, where the only display is.
fn beside(others: impl IntoIterator<Item = CGRect>) -> CGPoint {
    let right = |rect: &CGRect| rect.origin.x + rect.size.width;
    others
        .into_iter()
        .max_by(|a, b| {
            right(a)
                .total_cmp(&right(b))
                .then(b.origin.y.total_cmp(&a.origin.y))
        })
        .map_or(CGPoint::ZERO, |rightmost| {
            CGPoint::new(right(&rightmost).round(), rightmost.origin.y.round())
        })
}

/// A display configuration call's status as a result.
fn configured(call: &str, status: CGError) -> Result<(), Error> {
    if status == CGError::Success {
        Ok(())
    } else {
        Err(platform(call, status))
    }
}

/// Places the display at `origin`, in whole points, for the login session,
/// taking it out of its mirror set when it is `mirrored`; virtual displays
/// refuse a permanent configuration. `CoreGraphics` invokes the driving
/// program's reconfiguration callbacks from within the call that completes
/// the configuration.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the origin is whole points of the 32-bit global display space"
)]
fn arrange(id: CGDirectDisplayID, mirrored: bool, origin: CGPoint) -> Result<(), Error> {
    let mut configuration: CGDisplayConfigRef = ptr::null_mut();
    // SAFETY: `configuration` is a valid out pointer.
    let status = unsafe { CGBeginDisplayConfiguration(&raw mut configuration) };
    configured("CGBeginDisplayConfiguration", status)?;
    let changes = (|| {
        if mirrored {
            // SAFETY: `configuration` is begun and not completed.
            let status = unsafe {
                CGConfigureDisplayMirrorOfDisplay(configuration, id, kCGNullDirectDisplay)
            };
            configured("CGConfigureDisplayMirrorOfDisplay", status)?;
        }
        // SAFETY: as above.
        let status = unsafe {
            CGConfigureDisplayOrigin(configuration, id, origin.x as i32, origin.y as i32)
        };
        configured("CGConfigureDisplayOrigin", status)
    })();
    if let Err(error) = changes {
        // SAFETY: the configuration is begun and not completed.
        unsafe { CGCancelDisplayConfiguration(configuration) };
        return Err(error);
    }
    // SAFETY: `configuration` is begun; completing consumes it.
    let status =
        unsafe { CGCompleteDisplayConfiguration(configuration, CGConfigureOption::ForSession) };
    configured("CGCompleteDisplayConfiguration", status)
}

/// One reconfiguration callback.
struct Change {
    display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
}

/// What the reconfiguration callbacks `CoreGraphics` invoked while
/// registered report, delivered on the main run loop: the displays added, and
/// whether a reconfiguration is under way.
///
/// Before a reconfiguration a callback with only
/// `kCGDisplayBeginConfigurationFlag` runs for each online display; after
/// it, one runs for each added, removed and online display, by when every
/// display state `CoreGraphics` reports is current (`CGDisplayConfiguration.h`).
/// A reconfiguration is under way from a begin callback to the next callback
/// without that flag.
struct Reconfigurations {
    sender: Box<mpsc::Sender<Change>>,
    receiver: mpsc::Receiver<Change>,
    added: Vec<CGDirectDisplayID>,
    configuring: bool,
}

unsafe extern "C-unwind" fn on_reconfiguration(
    display: CGDirectDisplayID,
    flags: CGDisplayChangeSummaryFlags,
    sender: *mut c_void,
) {
    // SAFETY: `sender` is the boxed sender `Reconfigurations` registered,
    // which outlives its registration.
    let sender = unsafe { &*sender.cast::<mpsc::Sender<Change>>() };
    sender
        .send(Change { display, flags })
        .expect("the receiver outlives the callback's registration");
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
            configuring: false,
        })
    }

    /// Takes in the callbacks delivered since the last call, in order.
    fn receive(&mut self) {
        for change in self.receiver.try_iter() {
            self.configuring = change
                .flags
                .contains(CGDisplayChangeSummaryFlags::BeginConfigurationFlag);
            if change.flags.contains(CGDisplayChangeSummaryFlags::AddFlag) {
                self.added.push(change.display);
            }
        }
    }

    /// Whether a callback reported `id` added.
    fn added(&mut self, id: CGDirectDisplayID) -> bool {
        self.receive();
        self.added.contains(&id)
    }

    /// Whether a reconfiguration has begun and not yet ended.
    fn configuring(&mut self) -> bool {
        self.receive();
        self.configuring
    }

    /// Spins the main run loop until `done` holds or `timeout` passes;
    /// whether `done` held. Each reconfiguration callback, and each other
    /// source the loop handles, ends one spin so `done` is re-checked.
    fn wait(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&mut Self) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        run_loop::spin(timeout, timeout, || done(self))
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
        let _skylight = Framework::skylight().expect("SkyLight opens");
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

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
    }

    #[test]
    fn a_display_goes_at_the_right_edge_top_aligned_with_the_rightmost() {
        let main = rect(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(beside([main]), CGPoint::new(1512.0, 0.0));
        let right = rect(1512.0, -200.0, 1920.0, 1080.0);
        assert_eq!(beside([main, right]), CGPoint::new(3432.0, -200.0));
        let left = rect(-1280.0, 0.0, 1280.0, 800.0);
        assert_eq!(beside([left, right, main]), CGPoint::new(3432.0, -200.0));
    }

    #[test]
    fn of_displays_ending_at_one_edge_the_topmost_aligns() {
        let lower = rect(0.0, 982.0, 1512.0, 900.0);
        let main = rect(0.0, 0.0, 1512.0, 982.0);
        assert_eq!(beside([lower, main]), CGPoint::new(1512.0, 0.0));
        assert_eq!(beside([main, lower]), CGPoint::new(1512.0, 0.0));
    }

    #[test]
    fn a_display_alone_goes_at_the_global_origin() {
        assert_eq!(beside([]), CGPoint::ZERO);
    }

    #[test]
    fn the_origin_is_in_whole_points() {
        assert_eq!(
            beside([rect(0.0, 158.5, 1727.6, 800.0)]),
            CGPoint::new(1728.0, 159.0)
        );
    }

    /// Feeds callbacks through the channel the registered callback sends on;
    /// registering creates and shows nothing.
    #[test]
    fn a_reconfiguration_is_under_way_from_its_begin_to_its_end() {
        let mut changes = Reconfigurations::watch().expect("the callback registers");
        let send = |changes: &Reconfigurations, display, flags| {
            changes
                .sender
                .send(Change { display, flags })
                .expect("the receiver is alive");
        };
        let begin = CGDisplayChangeSummaryFlags::BeginConfigurationFlag;
        assert!(!changes.configuring());
        send(&changes, 1, begin);
        send(&changes, 2, begin);
        assert!(changes.configuring());
        assert!(!changes.added(7));
        send(&changes, 7, CGDisplayChangeSummaryFlags::AddFlag);
        send(&changes, 1, CGDisplayChangeSummaryFlags::MovedFlag);
        assert!(!changes.configuring());
        assert!(changes.added(7));
        send(&changes, 1, begin);
        assert!(changes.configuring());
    }

    #[test]
    fn a_size_beyond_32_bit_pixels_is_refused() {
        assert!(matches!(
            config(u32::MAX, 800).pixels(),
            Err(Error::SizeOverflow { scale: 2, .. })
        ));
    }
}
