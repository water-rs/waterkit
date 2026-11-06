//! Input posted through the crate reaches a window this test opens on a
//! virtual display it creates, and the user's cursor stays where it was.
//!
//! The target owns the process main thread (`harness = false`, one test
//! thread): the display is created there, and `AppKit` events are pumped
//! there. Input is posted from another thread meanwhile, as it is into any
//! application whose event loop keeps running.

use libtest_mimic::{Arguments, Trial};

fn main() {
    let mut arguments = Arguments::from_args();
    arguments.test_threads = Some(1);
    #[cfg(target_os = "macos")]
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "waterkit_automation=trace",
        ))
        .with_writer(std::io::stderr)
        .init();
    #[cfg(target_os = "macos")]
    let trials = vec![
        Trial::test(
            "a_click_and_keys_reach_a_window_on_a_virtual_display",
            || {
                mac::a_click_and_keys_reach_a_window_on_a_virtual_display();
                Ok(())
            },
        ),
        Trial::test("input_needs_an_online_display", || {
            mac::input_needs_an_online_display();
            Ok(())
        }),
    ];
    #[cfg(not(target_os = "macos"))]
    let trials: Vec<Trial> = Vec::new();
    libtest_mimic::run(&arguments, trials).exit();
}

#[cfg(target_os = "macos")]
mod mac {
    use core::num::NonZeroU32;
    use core::ptr::NonNull;
    use core::time::Duration;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::thread;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent, NSEventMask,
        NSEventType, NSScreen, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGDisplayBounds, CGEvent, CGMainDisplayID};
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSNumber, NSPoint, NSProcessInfo, ns_string,
    };
    use waterkit_automation::{
        Code, Error, Input, KeyDown, KeyUp, MouseButton, MouseDown, MouseUp, Point, Rect, Text,
        VirtualDisplay, VirtualDisplayConfig,
    };

    /// How long each posted event gets to arrive in the window.
    const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

    /// The test window's frame on the display, in points from its top-left.
    const WINDOW_ORIGIN: CGPoint = CGPoint::new(100.0, 100.0);
    const WINDOW_SIZE: CGSize = CGSize::new(400.0, 300.0);

    /// Where the click lands, in points from the window's top-left.
    const CLICK_IN_WINDOW: CGPoint = CGPoint::new(40.0, 30.0);

    /// What the window's view received.
    #[derive(Debug, Default)]
    struct Observed {
        /// Location in the window (bottom-left origin) and click count of each
        /// press.
        presses: Vec<(NSPoint, isize)>,
        releases: Vec<NSPoint>,
        /// Key code and characters of each key press.
        keys: Vec<(u16, String)>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitAutomationTestRecorder"]
        #[ivars = RefCell<Observed>]
        struct Recorder;

        impl Recorder {
            #[unsafe(method(acceptsFirstResponder))]
            fn accepts_first_responder(&self) -> bool {
                true
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                self.ivars()
                    .borrow_mut()
                    .presses
                    .push((event.locationInWindow(), event.clickCount()));
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, event: &NSEvent) {
                self.ivars()
                    .borrow_mut()
                    .releases
                    .push(event.locationInWindow());
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, event: &NSEvent) {
                let characters = event
                    .characters()
                    .map(|characters| characters.to_string())
                    .unwrap_or_default();
                self.ivars()
                    .borrow_mut()
                    .keys
                    .push((event.keyCode(), characters));
            }
        }
    );

    impl Recorder {
        fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(RefCell::default());
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }
    }

    define_class!(
        /// A borderless window, which `AppKit` keeps where it is put, that can
        /// still become key.
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitAutomationTestWindow"]
        struct KeyableWindow;

        impl KeyableWindow {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key_window(&self) -> bool {
                true
            }
        }
    );

    impl KeyableWindow {
        /// A window whose content rect is `frame`, relative to the
        /// bottom-left corner of `screen`.
        fn new(mtm: MainThreadMarker, frame: CGRect, screen: &NSScreen) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            // SAFETY: an initializer of NSWindow, with a valid style, backing
            // and screen.
            let window: Retained<Self> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: NSWindowStyleMask::Borderless,
                    backing: NSBackingStoreType::Buffered,
                    defer: false,
                    screen: screen
                ]
            };
            // SAFETY: the test owns the window through `Retained`; closing it
            // must not release it a second time.
            unsafe { window.setReleasedWhenClosed(false) };
            window
        }

        fn number(&self) -> u32 {
            u32::try_from(self.windowNumber()).expect("window numbers are 32-bit")
        }
    }

    /// Every event `NSApp` dispatches, as a local event monitor sees it.
    struct EventLog {
        monitor: Retained<AnyObject>,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl EventLog {
        fn install() -> Self {
            let events = Rc::new(RefCell::new(Vec::new()));
            let seen = Rc::clone(&events);
            let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: `AppKit` hands the monitor a live event.
                let described = describe_event(unsafe { event.as_ref() });
                seen.borrow_mut().push(described);
                event.as_ptr()
            });
            // SAFETY: the handler returns the event it is given, so dispatch
            // goes on unchanged.
            let monitor = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Any, &handler)
            }
            .expect("AppKit installs a local event monitor");
            Self { monitor, events }
        }

        fn describe(&self) -> String {
            let events = self.events.borrow();
            if events.is_empty() {
                "NSApp dispatched no event at all".to_owned()
            } else {
                format!("NSApp dispatched, in order: {}", events.join("; "))
            }
        }
    }

    impl Drop for EventLog {
        fn drop(&mut self) {
            // SAFETY: the monitor came from
            // `addLocalMonitorForEventsMatchingMask:handler:` and is removed
            // once.
            unsafe { NSEvent::removeMonitor(&self.monitor) };
        }
    }

    /// What `AppKit` routes `event` by: its type, window and location or key.
    fn describe_event(event: &NSEvent) -> String {
        let kind = event.r#type();
        let window = event.windowNumber();
        let pointer = |name: &str| {
            format!(
                "{name} in window {window} at {:?}",
                event.locationInWindow()
            )
        };
        let press = |name: &str| format!("{}, click count {}", pointer(name), event.clickCount());
        let key = |name: &str| {
            format!(
                "{name} in window {window}, key code {}, characters {:?}",
                event.keyCode(),
                event
                    .characters()
                    .map(|characters| characters.to_string())
                    .unwrap_or_default()
            )
        };
        let other = |name: &str| format!("{name} subtype {} in window {window}", event.subtype().0);
        match kind {
            NSEventType::LeftMouseDown => press("left mouse down"),
            NSEventType::LeftMouseUp => press("left mouse up"),
            NSEventType::RightMouseDown => press("right mouse down"),
            NSEventType::RightMouseUp => press("right mouse up"),
            NSEventType::MouseMoved => pointer("mouse moved"),
            NSEventType::LeftMouseDragged => pointer("left mouse dragged"),
            NSEventType::KeyDown => key("key down"),
            NSEventType::KeyUp => key("key up"),
            NSEventType::AppKitDefined => other("AppKit-defined"),
            NSEventType::SystemDefined => other("system-defined"),
            NSEventType::ApplicationDefined => other("application-defined"),
            _ => format!("event type {} in window {window}", kind.0),
        }
    }

    /// The user's cursor, read from a fresh event.
    fn cursor() -> CGPoint {
        let event = CGEvent::new(None).expect("CoreGraphics creates an event");
        CGEvent::location(Some(&event))
    }

    /// Dispatches `AppKit` events until `done` holds; false when `timeout`
    /// passes first.
    fn pump_until(
        mtm: MainThreadMarker,
        timeout: Duration,
        mut done: impl FnMut() -> bool,
    ) -> bool {
        let app = NSApplication::sharedApplication(mtm);
        let deadline = Instant::now() + timeout;
        while !done() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let until = NSDate::dateWithTimeIntervalSinceNow(remaining.as_secs_f64());
            // SAFETY: the default run loop mode is an immutable constant.
            let event = unsafe {
                app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&until),
                    NSDefaultRunLoopMode,
                    true,
                )
            };
            if let Some(event) = event {
                app.sendEvent(&event);
            }
        }
        true
    }

    /// Runs `post` with `input` on another thread while this one dispatches
    /// `AppKit` events, until the post has returned and `delivered` holds.
    /// Gives `input` back with the post's result and whether `delivered`
    /// held within [`DELIVERY_DEADLINE`].
    fn post_while_pumping(
        mtm: MainThreadMarker,
        mut input: Input,
        post: impl FnOnce(&mut Input) -> Result<(), Error> + Send + 'static,
        mut delivered: impl FnMut() -> bool,
    ) -> (Input, Result<(), Error>, bool) {
        let poster = thread::spawn(move || {
            let result = post(&mut input);
            (input, result)
        });
        pump_until(mtm, DELIVERY_DEADLINE, || {
            delivered() || poster.is_finished()
        });
        assert!(
            poster.is_finished() || delivered(),
            "Input::post did not return within {DELIVERY_DEADLINE:?}"
        );
        let (input, result) = poster.join().expect("the posting thread does not panic");
        if result.is_err() {
            return (input, result, false);
        }
        let arrived = pump_until(mtm, DELIVERY_DEADLINE, delivered);
        (input, result, arrived)
    }

    /// The display `AppKit` shows `screen` on.
    fn screen_display(screen: &NSScreen) -> u32 {
        screen
            .deviceDescription()
            .objectForKey(ns_string!("NSScreenNumber"))
            .expect("every screen carries its display number")
            .downcast::<NSNumber>()
            .expect("the display number is a number")
            .unsignedIntValue()
    }

    /// `AppKit`'s screen for `display`, once it lists one.
    fn appkit_screen(mtm: MainThreadMarker, display: u32) -> Option<Retained<NSScreen>> {
        NSScreen::screens(mtm)
            .iter()
            .find(|screen| screen_display(screen) == display)
    }

    /// Every screen `AppKit` lists, and the main display `CoreGraphics`
    /// reports.
    fn describe_screens(mtm: MainThreadMarker) -> String {
        let screens: Vec<String> = NSScreen::screens(mtm)
            .iter()
            .map(|screen| format!("display {} {:?}", screen_display(&screen), screen.frame()))
            .collect();
        let main = CGMainDisplayID();
        format!(
            "AppKit screens (bottom-left origin): [{}]; CoreGraphics main display {main} {:?}",
            screens.join(", "),
            CGDisplayBounds(main)
        )
    }

    /// Where `AppKit` and `WindowServer` have the test window.
    fn describe_window(window: &KeyableWindow, inspector: &Input) -> String {
        let number = window.number();
        let listed = inspector
            .windows()
            .expect("the window list reads")
            .into_iter()
            .find(|listed| listed.id() == number)
            .map_or_else(|| "not at all".to_owned(), |listed| listed.to_string());
        format!(
            "test window {number}: AppKit frame {:?} on display {:?}, visible {}, occlusion {:?}; WindowServer lists it as {listed}",
            window.frame(),
            window.screen().map(|screen| screen_display(&screen)),
            window.isVisible(),
            window.occlusionState(),
        )
    }

    /// Whether `AppKit` sees the test process as active and the test window
    /// as key, which decides where it routes a posted event.
    fn describe_app(mtm: MainThreadMarker, window: &KeyableWindow) -> String {
        let app = NSApplication::sharedApplication(mtm);
        let policy = match app.activationPolicy() {
            NSApplicationActivationPolicy::Regular => "regular".to_owned(),
            NSApplicationActivationPolicy::Accessory => "accessory".to_owned(),
            NSApplicationActivationPolicy::Prohibited => "prohibited".to_owned(),
            other => format!("{other:?}"),
        };
        format!(
            "test process pid {}: activation policy {policy}, active {}, key window {:?}; test window key {}, main {}",
            std::process::id(),
            app.isActive(),
            app.keyWindow().map(|key| key.windowNumber()),
            window.isKeyWindow(),
            window.isMainWindow(),
        )
    }

    /// Everything that tells where a posted event went; the crate's log
    /// above lists every record and event `Input` posted.
    fn describe_delivery(
        mtm: MainThreadMarker,
        window: &KeyableWindow,
        log: &EventLog,
        inspector: &Input,
    ) -> String {
        format!(
            "{}\n{}\n{}\n{}\nThe crate's log above lists what Input posted.",
            describe_app(mtm, window),
            log.describe(),
            describe_window(window, inspector),
            describe_screens(mtm),
        )
    }

    fn outcome(result: &Result<(), Error>) -> String {
        match result {
            Ok(()) => "posted".to_owned(),
            Err(error) => error.to_string(),
        }
    }

    /// Whether `rect` holds `point`, edges half-open.
    fn holds(rect: Rect, point: CGPoint) -> bool {
        (rect.x()..rect.x() + rect.width()).contains(&point.x)
            && (rect.y()..rect.y() + rect.height()).contains(&point.y)
    }

    /// A 1280x800-point display at 2x with a serial the system has not seen.
    fn create_display(mtm: MainThreadMarker) -> VirtualDisplay {
        let serial = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .subsec_nanos()
            | 1;
        let config = VirtualDisplayConfig::new(
            "waterkit automation test",
            NonZeroU32::new(1280).expect("non-zero"),
            NonZeroU32::new(800).expect("non-zero"),
            serial,
        );
        let display = VirtualDisplay::new(&config, mtm).expect("the virtual display comes up");
        let bounds = CGDisplayBounds(display.id());
        assert_eq!(
            (bounds.size.width, bounds.size.height),
            (1280.0, 800.0),
            "the display is 1280x800 points"
        );
        display
    }

    /// A window on `display` whose content view records what it receives,
    /// placed on `AppKit`'s screen for the display.
    fn open_window(
        mtm: MainThreadMarker,
        display: &VirtualDisplay,
    ) -> (Retained<KeyableWindow>, Retained<Recorder>) {
        let mut screen = None;
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || {
                screen = appkit_screen(mtm, display.id());
                screen.is_some()
            }),
            "AppKit never listed display {}: {}",
            display.id(),
            describe_screens(mtm)
        );
        let screen = screen.expect("the wait ends once the screen is listed");
        // The screen's frame has a bottom-left origin; the window's top edge
        // sits `WINDOW_ORIGIN.y` below the screen's.
        let frame = CGRect::new(
            CGPoint::new(
                WINDOW_ORIGIN.x,
                screen.frame().size.height - WINDOW_ORIGIN.y - WINDOW_SIZE.height,
            ),
            WINDOW_SIZE,
        );
        let window = KeyableWindow::new(mtm, frame, &screen);
        let recorder = Recorder::new(mtm, CGRect::new(CGPoint::ZERO, WINDOW_SIZE));
        window.setContentView(Some(&recorder));
        window.makeFirstResponder(Some(&recorder));
        window.orderFrontRegardless();
        (window, recorder)
    }

    /// One single click at [`CLICK_IN_WINDOW`], then "hi" and the A key.
    fn assert_delivered(observed: &Observed) {
        let expected = NSPoint::new(CLICK_IN_WINDOW.x, WINDOW_SIZE.height - CLICK_IN_WINDOW.y);
        let (location, clicks) = observed.presses[0];
        assert!(
            (location.x - expected.x).abs() < 0.5 && (location.y - expected.y).abs() < 0.5,
            "the press landed at {location:?} in the window, expected {expected:?}: {observed:?}"
        );
        assert_eq!(clicks, 1, "the press is a single click");
        assert_eq!(observed.presses.len(), 1, "one press arrived: {observed:?}");
        assert_eq!(
            observed.releases.len(),
            1,
            "one release arrived: {observed:?}"
        );
        assert_eq!(
            observed.keys[0].1, "hi",
            "the text arrived as typed: {observed:?}"
        );
        assert_eq!(observed.keys[1].0, 0, "the A key arrived: {observed:?}");
        assert_eq!(
            observed.keys.len(),
            2,
            "two key presses arrived: {observed:?}"
        );
    }

    pub fn a_click_and_keys_reach_a_window_on_a_virtual_display() {
        let mtm = MainThreadMarker::new().expect("libtest-mimic runs one thread on main");
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        eprintln!(
            "macOS {}.{}.{}",
            version.majorVersion, version.minorVersion, version.patchVersion
        );
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        let display = create_display(mtm);
        let (window, recorder) = open_window(mtm, &display);
        let log = EventLog::install();
        let cursor_before = cursor();
        let inspector = Input::new(display.id()).expect("input opens on the virtual display");
        let input = Input::new(display.id()).expect("input opens on the virtual display");
        let at = Point::new(
            WINDOW_ORIGIN.x + CLICK_IN_WINDOW.x,
            WINDOW_ORIGIN.y + CLICK_IN_WINDOW.y,
        );

        // `WindowServer` lists the window once it has ordered it in.
        let display_origin = CGDisplayBounds(display.id()).origin;
        let point = CGPoint::new(display_origin.x + at.x(), display_origin.y + at.y());
        let number = window.number();
        let listed = pump_until(mtm, DELIVERY_DEADLINE, || {
            inspector
                .windows()
                .expect("the window list reads")
                .iter()
                .any(|listed| {
                    listed.id() == number
                        && listed.on_screen()
                        && listed.application()
                        && holds(listed.bounds(), point)
                })
        });
        assert!(
            listed,
            "WindowServer never listed the test window on screen under {at}\n{}\n{}",
            describe_window(&window, &inspector),
            describe_screens(mtm)
        );

        let observed = || recorder.ivars().borrow();
        let (input, pressed, arrived) = post_while_pumping(
            mtm,
            input,
            move |input| input.post(MouseDown::new(at, MouseButton::Left)),
            || !observed().presses.is_empty(),
        );
        assert!(
            pressed.is_ok() && arrived,
            "the press ({}) never reached the window: {:?}\n{}",
            outcome(&pressed),
            observed(),
            describe_delivery(mtm, &window, &log, &inspector)
        );

        let (input, released, arrived) = post_while_pumping(
            mtm,
            input,
            move |input| input.post(MouseUp::new(at, MouseButton::Left)),
            || !observed().releases.is_empty(),
        );
        assert!(
            released.is_ok() && arrived,
            "the release ({}) never reached the window: {:?}\n{}",
            outcome(&released),
            observed(),
            describe_delivery(mtm, &window, &log, &inspector)
        );

        let (_input, typed, arrived) = post_while_pumping(
            mtm,
            input,
            |input| {
                input.post(Text::new("hi"))?;
                input.post(KeyDown::new(Code::KeyA))?;
                input.post(KeyUp::new(Code::KeyA))
            },
            || observed().keys.len() >= 2,
        );
        assert!(
            typed.is_ok() && arrived,
            "the key presses ({}) never reached the window: {:?}\n{}",
            outcome(&typed),
            observed(),
            describe_delivery(mtm, &window, &log, &inspector)
        );

        assert_delivered(&observed());

        let cursor_after = cursor();
        assert!(
            cursor_after.x.to_bits() == cursor_before.x.to_bits()
                && cursor_after.y.to_bits() == cursor_before.y.to_bits(),
            "the cursor moved from {cursor_before:?} to {cursor_after:?}"
        );
        drop(log);
        window.close();
    }

    pub fn input_needs_an_online_display() {
        let result = Input::new(u32::MAX);
        assert!(
            matches!(result, Err(Error::DisplayNotFound(u32::MAX))),
            "an unknown display is refused, got {result:?}"
        );
    }
}
