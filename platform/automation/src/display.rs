//! Virtual displays: private-API `CGVirtualDisplay`s.

use core::ffi::{CStr, c_char, c_void};
use core::num::NonZeroU32;
use core::ptr;
use core::time::Duration;
use std::rc::{Rc, Weak};
use std::sync::mpsc;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, Method};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_core_foundation::{CFRunLoop, CGRect, CGSize};
use objc2_core_graphics::{
    CGBeginDisplayConfiguration, CGCancelDisplayConfiguration, CGCompleteDisplayConfiguration,
    CGConfigureDisplayMirrorOfDisplay, CGConfigureDisplayOrigin, CGConfigureOption,
    CGDirectDisplayID, CGDisplayBounds, CGDisplayChangeSummaryFlags, CGDisplayConfigRef,
    CGDisplayCopyDisplayMode, CGDisplayMode, CGDisplayModelNumber,
    CGDisplayRegisterReconfigurationCallback, CGDisplayRemoveReconfigurationCallback,
    CGDisplayVendorNumber, CGError, CGMainDisplayID, kCGNullDirectDisplay,
};
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_io_kit::kDisplayVendorIDUnknown;
use waterkit_core::RefreshRate;

use crate::error::platform;
use crate::framework::Framework;
use crate::skylight::DisplayLists;
use crate::topology::{self, Presence};
use crate::{DisplayState, Error, run_loop};

/// Vendor id the displays carry: outside the ids assigned to real vendors.
const VENDOR_ID: u32 = 0xF0F0;
/// Product id of the displays callers ask for.
const PRODUCT_ID: u32 = 0xF0F1;
/// Product id of the stand-in a [`DisplayHost`] puts in place of the
/// system's headless fallback display.
const STAND_IN_PRODUCT_ID: u32 = 0xF0F2;
/// The stand-in's serial. Only one stand-in is up at a time, and the same
/// serial each time brings back the arrangement the system keeps for it.
const STAND_IN_SERIAL: u32 = 1;
/// The stand-in's name in the display lists.
const STAND_IN_NAME: &str = "Headless Display";
/// The model number of the display macOS runs when no monitor is connected:
/// `'virt'`. No header publishes it; it is what that display reports on
/// macOS 27, and what Vuo (`library/VuoScreenCommon.mm`) recognises the
/// display by, beside the published vendor `kDisplayVendorIDUnknown`.
const FALLBACK_MODEL: u32 = u32::from_be_bytes(*b"virt");
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
    /// and taking it out of a mirror set; and how long dropping it waits for
    /// it to go offline. Three seconds by default.
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

/// Where a process creates its [`VirtualDisplay`]s: on the main thread,
/// beside a main display that stays the user's.
///
/// With no monitor connected, macOS runs a fallback display of its own
/// (vendor `kDisplayVendorIDUnknown`, model `'virt'`) and removes it as soon
/// as another display comes online, which would leave the new display the
/// only one and move the user's desktop onto it. So while the only online
/// displays are that fallback, the host first brings up a stand-in display
/// in the fallback's mode, which takes over the user's desktop as the
/// fallback goes and stays the main display. Every display created through
/// the host holds the stand-in; it is removed with the last of them, and the
/// system brings its fallback back. Displays created through different hosts
/// do not share a stand-in, so a process keeps one host for all of them.
#[derive(Debug)]
pub struct DisplayHost {
    stand_in: Weak<Created>,
    _mtm: MainThreadMarker,
}

impl DisplayHost {
    /// A host with no display up yet.
    #[must_use]
    pub const fn new(mtm: MainThreadMarker) -> Self {
        Self {
            stand_in: Weak::new(),
            _mtm: mtm,
        }
    }

    /// The stand-in the displays of this host share: the live one, or a new
    /// one when the only online displays are the system's headless fallback;
    /// `None` when the user has displays of their own.
    fn stand_in(
        &mut self,
        classes: &Classes,
        lists: &DisplayLists<'_>,
        changes: &mut Reconfigurations,
        config: &VirtualDisplayConfig,
    ) -> Result<Option<Rc<Created>>, Error> {
        if let Some(stand_in) = self.stand_in.upgrade() {
            return Ok(Some(stand_in));
        }
        let online = topology::online()?;
        if online.is_empty() || !online.iter().all(|&id| is_headless_fallback(id)) {
            return Ok(None);
        }
        let fallback = CGMainDisplayID();
        let stand_in = bring_up(
            classes,
            lists,
            changes,
            &stand_in_config(fallback, config)?,
            STAND_IN_PRODUCT_ID,
            None,
        )?;
        let still_online = || {
            Ok::<_, Error>(
                topology::online()?
                    .into_iter()
                    .find(|id| online.contains(id)),
            )
        };
        changes.wait(config.online_timeout, |_| Ok(still_online()?.is_none()))?;
        if let Some(fallback) = still_online()? {
            return Err(Error::FallbackStayed {
                fallback,
                stand_in: stand_in.id,
            });
        }
        keep_main(stand_in.id, changes, config.online_timeout)?;
        tracing::debug!(
            stand_in = stand_in.id,
            ?online,
            "a stand-in display replaced the headless fallback as the main display"
        );
        let stand_in = Rc::new(stand_in);
        self.stand_in = Rc::downgrade(&stand_in);
        Ok(Some(stand_in))
    }
}

/// Whether `id` is the display macOS runs when no monitor is connected.
fn is_headless_fallback(id: CGDirectDisplayID) -> bool {
    CGDisplayVendorNumber(id) == kDisplayVendorIDUnknown
        && CGDisplayModelNumber(id) == FALLBACK_MODEL
}

/// The stand-in for the display `fallback`: its mode, and `config`'s
/// timeout. A mode that reports no refresh rate takes `config`'s.
fn stand_in_config(
    fallback: CGDirectDisplayID,
    config: &VirtualDisplayConfig,
) -> Result<VirtualDisplayConfig, Error> {
    let mode = CGDisplayCopyDisplayMode(fallback).ok_or_else(|| {
        Error::Platform(format!(
            "CGDisplayCopyDisplayMode returned NULL for display {fallback}"
        ))
    })?;
    let mode = Some(&*mode);
    let (width, height) = (CGDisplayMode::width(mode), CGDisplayMode::height(mode));
    let (pixels_wide, pixels_high) = (
        CGDisplayMode::pixel_width(mode),
        CGDisplayMode::pixel_height(mode),
    );
    let hertz = CGDisplayMode::refresh_rate(mode);
    let unmatchable = || Error::UnmatchableMode {
        display: fallback,
        width,
        height,
        pixels_wide,
        pixels_high,
        refresh_rate: hertz,
    };
    let points = |extent: usize| {
        u32::try_from(extent)
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or_else(unmatchable)
    };
    let scale = [BackingScale::X1, BackingScale::X2]
        .into_iter()
        .find(|scale| {
            let factor = scale.factor() as usize;
            width.checked_mul(factor) == Some(pixels_wide)
                && height.checked_mul(factor) == Some(pixels_high)
        })
        .ok_or_else(unmatchable)?;
    // `CGDisplayModeGetRefreshRate` reports 0 for a display that states no
    // rate.
    let refresh_rate = if hertz == 0.0 {
        config.refresh_rate
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "refresh rates are far inside f32's range and precision"
        )]
        let hertz = hertz as f32;
        RefreshRate::new(hertz).map_err(|_| unmatchable())?
    };
    Ok(VirtualDisplayConfig::new(
        STAND_IN_NAME,
        points(width)?,
        points(height)?,
        STAND_IN_SERIAL,
    )
    .scale(scale)
    .refresh_rate(refresh_rate)
    .online_timeout(config.online_timeout))
}

/// A display on none of the user's monitors, removed when dropped.
///
/// Created through a [`DisplayHost`] and dropped on the main thread; see the
/// crate documentation for how creation services the main run loop. Dropping
/// services it the same way until the display is offline and the displays it
/// was created beside are back in the frames they had (the system's headless
/// fallback under a new id, when the host's stand-in goes with it), at most
/// [`online_timeout`](VirtualDisplayConfig::online_timeout) for each.
#[derive(Debug)]
pub struct VirtualDisplay {
    // Declared before the stand-in: the display goes first, so the stand-in
    // is never removed while this display is the one left.
    display: Created,
    _stand_in: Option<Rc<Created>>,
    width: NonZeroU32,
    height: NonZeroU32,
    scale: BackingScale,
}

impl VirtualDisplay {
    /// Creates the display and waits until it is online, active and outside
    /// any mirror set, with the user's main display still main; on a Mac
    /// whose only display is the system's headless fallback, `host` first
    /// puts its stand-in in the fallback's place.
    ///
    /// # Errors
    ///
    /// - [`Error::Unavailable`] or [`Error::SignatureMismatch`] when the
    ///   private API this needs is missing or changed;
    /// - [`Error::SizeOverflow`] when the size in pixels does not fit in 32
    ///   bits;
    /// - [`Error::Refused`] when `CoreGraphics` refuses the display or the
    ///   stand-in;
    /// - [`Error::ExtendRefused`] when the display or the stand-in stays out
    ///   of the topology and `SLWindowMirroringManager` refuses to bring it
    ///   in;
    /// - [`Error::NotReady`] when the display or the stand-in is still not
    ///   online, active and unmirrored after the timeouts;
    /// - [`Error::UnmatchableMode`] when no stand-in can reproduce the
    ///   fallback's mode, and [`Error::FallbackStayed`] when the fallback
    ///   stays online beside the stand-in;
    /// - [`Error::NotMain`] when the user's main display, or the stand-in,
    ///   does not stay or become main;
    /// - [`Error::Platform`] when a display configuration call fails.
    pub fn new(config: &VirtualDisplayConfig, host: &mut DisplayHost) -> Result<Self, Error> {
        let skylight = Framework::skylight()?;
        let lists = DisplayLists::resolve(&skylight)?;
        let classes = Classes::get()?;
        // Checked before a stand-in is brought up for a display that cannot
        // be made.
        config.pixels()?;

        // Registered before any display exists: each is added asynchronously
        // after `applySettings:`.
        let mut changes = Reconfigurations::watch()?;
        let stand_in = host.stand_in(&classes, &lists, &mut changes, config)?;
        let main = CGMainDisplayID();
        let display = bring_up(
            &classes,
            &lists,
            &mut changes,
            config,
            PRODUCT_ID,
            stand_in.as_ref().map(|stand_in| stand_in.id),
        )?;
        keep_main(main, &mut changes, config.online_timeout)?;
        let id = display.id;
        tracing::debug!(
            id,
            main,
            stand_in = stand_in.as_ref().map(|stand_in| stand_in.id),
            "virtual display up beside the main display"
        );
        Ok(Self {
            display,
            _stand_in: stand_in,
            width: config.width,
            height: config.height,
            scale: config.scale,
        })
    }

    /// The display's `CGDirectDisplayID`.
    #[must_use]
    pub const fn id(&self) -> CGDirectDisplayID {
        self.display.id
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

/// The displays a new display must leave as it found them, read just before
/// it is created.
#[derive(Debug, Clone, PartialEq)]
struct Arrangement {
    /// The frames of the user's displays: every online display this crate
    /// did not create. They are matched by frame, not id, because the system
    /// brings its headless fallback back under a new id.
    users: Vec<CGRect>,
    /// The host's stand-in and its frame, when the new display has one: the
    /// user's desktop on a Mac with no monitor, though this crate made it.
    stand_in: Option<(CGDirectDisplayID, CGRect)>,
}

impl Arrangement {
    fn read(stand_in: Option<CGDirectDisplayID>) -> Result<Self, Error> {
        Ok(Self {
            users: user_frames(&topology::online()?),
            stand_in: stand_in.map(|id| (id, CGDisplayBounds(id))),
        })
    }

    /// Whether `gone` is offline and every display is back in its frame.
    fn restored(&self, gone: CGDirectDisplayID) -> Result<bool, Error> {
        let online = topology::online()?;
        Ok(!online.contains(&gone)
            && same_frames(&self.users, &user_frames(&online))
            && self
                .stand_in
                .is_none_or(|(id, frame)| online.contains(&id) && CGDisplayBounds(id) == frame))
    }
}

/// The frames of the displays in `online` this crate did not create.
fn user_frames(online: &[CGDirectDisplayID]) -> Vec<CGRect> {
    online
        .iter()
        .filter(|&&id| CGDisplayVendorNumber(id) != VENDOR_ID)
        .map(|&id| CGDisplayBounds(id))
        .collect()
}

/// Whether `a` and `b` hold the same frames, in any order.
fn same_frames(a: &[CGRect], b: &[CGRect]) -> bool {
    let mut unmatched = b.to_vec();
    a.len() == b.len()
        && a.iter().all(|frame| {
            unmatched
                .iter()
                .position(|other| other == frame)
                .map(|at| unmatched.swap_remove(at))
                .is_some()
        })
}

/// A virtual display the system has assigned an id, removed when dropped.
///
/// Removal is asynchronous: releasing the display returns before
/// `WindowServer` takes it offline, before the user's displays are back in
/// the modes they had (a display can pass through another mode as an extra
/// one leaves), and, when it was the last display, before the system brings
/// its headless fallback back. So dropping it waits, servicing the main run
/// loop as creation does, until the display is offline and the
/// [`Arrangement`] it found is back; a later creation then never reads a
/// display on its way out, or a mode in passing, for the user's.
#[derive(Debug)]
struct Created {
    object: Option<Retained<AnyObject>>,
    id: CGDirectDisplayID,
    found: Arrangement,
    timeout: Duration,
}

impl Drop for Created {
    fn drop(&mut self) {
        let id = self.id;
        // Registered before the release, so the removal ends the wait.
        let changes = match Reconfigurations::watch() {
            Ok(changes) => changes,
            Err(error) => {
                drop(self.object.take());
                tracing::warn!(id, %error, "the virtual display's removal cannot be awaited");
                return;
            }
        };
        drop(self.object.take());
        let settled = run_loop::spin(self.timeout, self.timeout, || self.found.restored(id));
        drop(changes);
        match settled {
            Ok(true) => tracing::debug!(id, "virtual display offline, the displays it found back"),
            Ok(false) => tracing::warn!(
                id,
                found = ?self.found,
                now = ?topology::online().map(|online| user_frames(&online)),
                timeout = ?self.timeout,
                "the virtual display is still online, or the displays it found are not back, after its removal"
            ),
            Err(error) => {
                tracing::warn!(id, %error, "the virtual display's removal could not be awaited");
            }
        }
    }
}

/// Creates a display with product id `product`, beside the host's stand-in
/// `stand_in` when there is one, and waits until it is online, active and
/// outside any mirror set.
fn bring_up(
    classes: &Classes,
    lists: &DisplayLists<'_>,
    changes: &mut Reconfigurations,
    config: &VirtualDisplayConfig,
    product: u32,
    stand_in: Option<CGDirectDisplayID>,
) -> Result<Created, Error> {
    let (pixels_wide, pixels_high) = config.pixels()?;
    let found = Arrangement::read(stand_in)?;
    let object = autoreleasepool(|_| classes.create(config, product, pixels_wide, pixels_high))?;
    // SAFETY: `displayID` takes nothing and returns the display's id.
    let id: CGDirectDisplayID = unsafe { msg_send![&object, displayID] };
    if id == kCGNullDirectDisplay {
        return Err(Error::Refused("displayID is null"));
    }
    let timeout = config.online_timeout;
    // From here every way out removes the display and waits for it to go.
    let created = Created {
        object: Some(object),
        id,
        found,
        timeout,
    };

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
        tracing::debug!(id, product, extended, "virtual display online");
        return Ok(created);
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

/// Makes `main` the main display, unless it is, by moving it to the origin
/// of the global display space, where the main display sits.
fn keep_main(
    main: CGDirectDisplayID,
    changes: &mut Reconfigurations,
    timeout: Duration,
) -> Result<(), Error> {
    if CGMainDisplayID() == main {
        return Ok(());
    }
    let taken_by = CGMainDisplayID();
    // SAFETY: `configure` passes a begun configuration.
    configure(|configuration| unsafe {
        (
            "CGConfigureDisplayOrigin",
            CGConfigureDisplayOrigin(configuration, main, 0, 0),
        )
    })?;
    changes.wait(timeout, |_| Ok(CGMainDisplayID() == main))?;
    let now = CGMainDisplayID();
    if now == main {
        tracing::debug!(main, taken_by, "the main display was made main again");
        Ok(())
    } else {
        Err(Error::NotMain {
            display: main,
            main: now,
        })
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
        product: u32,
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
            let _: () = msg_send![&descriptor, setProductID: product];
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
    // SAFETY: `configure` passes a begun configuration.
    configure(|configuration| unsafe {
        (
            "CGConfigureDisplayMirrorOfDisplay",
            CGConfigureDisplayMirrorOfDisplay(configuration, id, kCGNullDirectDisplay),
        )
    })
}

/// Applies the one change `step` makes to a begun configuration, for the
/// login session; `step` returns the call it made and its status.
fn configure(
    step: impl FnOnce(CGDisplayConfigRef) -> (&'static str, CGError),
) -> Result<(), Error> {
    let mut configuration: CGDisplayConfigRef = ptr::null_mut();
    // SAFETY: `configuration` is a valid out pointer.
    let status = unsafe { CGBeginDisplayConfiguration(&raw mut configuration) };
    if status != CGError::Success {
        return Err(platform("CGBeginDisplayConfiguration", status));
    }
    let (call, status) = step(configuration);
    if status != CGError::Success {
        // SAFETY: the configuration is begun and not completed.
        unsafe { CGCancelDisplayConfiguration(configuration) };
        return Err(platform(call, status));
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
        run_loop::spin(timeout, timeout, || done(self)).map(drop)
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

    /// A stand-in made from the main display's mode has that display's size
    /// in points; reading a mode creates and shows nothing.
    #[test]
    fn a_stand_in_reproduces_a_display_mode() {
        let main = CGMainDisplayID();
        let stand_in = stand_in_config(main, &config(1280, 800)).expect("the main mode matches");
        let bounds = objc2_core_graphics::CGDisplayBounds(main);
        assert_eq!(
            (
                f64::from(stand_in.width.get()),
                f64::from(stand_in.height.get())
            ),
            (bounds.size.width, bounds.size.height)
        );
        assert_eq!(stand_in.serial, STAND_IN_SERIAL);
    }

    #[test]
    fn frames_match_in_any_order_and_count() {
        let frame = |x: f64, width: f64| {
            CGRect::new(
                objc2_core_foundation::CGPoint::new(x, 0.0),
                CGSize::new(width, 1080.0),
            )
        };
        let wide = frame(0.0, 1920.0);
        let side = frame(1920.0, 1280.0);
        assert!(same_frames(&[wide, side], &[side, wide]));
        assert!(same_frames(&[], &[]));
        assert!(!same_frames(&[wide], &[frame(0.0, 1024.0)]));
        assert!(!same_frames(&[wide, wide], &[wide, side]));
        assert!(!same_frames(&[wide], &[wide, side]));
    }

    #[test]
    fn a_size_beyond_32_bit_pixels_is_refused() {
        assert!(matches!(
            config(u32::MAX, 800).pixels(),
            Err(Error::SizeOverflow { scale: 2, .. })
        ));
    }
}
