//! Input posted through the crate reaches a window this test opens on a
//! virtual display it creates, and the user's cursor stays where it was.
//!
//! The target owns the process main thread (`harness = false`, one test
//! thread): the display is created there, and `AppKit` events are pumped
//! there.

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
    use core::time::Duration;
    use std::cell::RefCell;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use objc2::rc::Retained;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent, NSEventMask,
        NSScreen, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGDisplayBounds, CGEvent, CGMainDisplayID};
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSNumber, NSPoint, NSProcessInfo, ns_string,
    };
    use waterkit_automation::{
        Code, Error, Input, KeyDown, KeyUp, MouseButton, MouseDown, MouseUp, Point, Text,
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
    fn describe_window(window: &KeyableWindow, input: &Input) -> String {
        let number = u32::try_from(window.windowNumber()).expect("window numbers are 32-bit");
        let listed = input
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

    fn outcome(result: &Result<(), Error>) -> String {
        match result {
            Ok(()) => "posted".to_owned(),
            Err(error) => error.to_string(),
        }
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
        let cursor_before = cursor();
        let mut input = Input::new(display.id()).expect("input opens on the virtual display");
        let at = Point::new(
            WINDOW_ORIGIN.x + CLICK_IN_WINDOW.x,
            WINDOW_ORIGIN.y + CLICK_IN_WINDOW.y,
        );

        // `WindowServer` lists the window once it has ordered it in.
        let mut pressed = Err(Error::DisplayNotFound(display.id()));
        let listed = pump_until(mtm, DELIVERY_DEADLINE, || {
            pressed = input.post(MouseDown::new(at, MouseButton::Left));
            !matches!(pressed, Err(Error::NoWindow { .. }))
        });
        assert!(
            listed,
            "the test window was never found under {at}; the last press gave: {}\n{}\n{}",
            outcome(&pressed),
            describe_window(&window, &input),
            describe_screens(mtm)
        );
        pressed.expect("the press is posted");
        let observed = || recorder.ivars().borrow();
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || !observed().presses.is_empty()),
            "the press never reached the window: {:?}",
            observed()
        );
        input
            .post(MouseUp::new(at, MouseButton::Left))
            .expect("the release is posted");
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || !observed().releases.is_empty()),
            "the release never reached the window: {:?}",
            observed()
        );

        input.post(Text::new("hi")).expect("the text is posted");
        input
            .post(KeyDown::new(Code::KeyA))
            .expect("the key press is posted");
        input
            .post(KeyUp::new(Code::KeyA))
            .expect("the key release is posted");
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || observed().keys.len() >= 2),
            "the key presses never reached the window: {:?}",
            observed()
        );

        assert_delivered(&observed());

        let cursor_after = cursor();
        assert!(
            cursor_after.x.to_bits() == cursor_before.x.to_bits()
                && cursor_after.y.to_bits() == cursor_before.y.to_bits(),
            "the cursor moved from {cursor_before:?} to {cursor_after:?}"
        );
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
