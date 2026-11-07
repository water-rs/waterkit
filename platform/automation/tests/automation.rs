//! Input posted through the crate reaches windows on a virtual display the
//! test creates, one opened by the test process itself and one opened by
//! another process, also under a transparent window of a third process that
//! ignores mouse events, and the user's cursor stays where it was. A stock
//! application launched through the crate opens in the background with its
//! window on the virtual display.
//!
//! The test owns the process main thread (`harness = false`, one test
//! thread): the display is created there, and `AppKit` events are pumped
//! there. Input into the test's own window is posted from another thread
//! meanwhile, as it is into any application whose event loop keeps running.
//!
//! The other process is this binary run again with [`mac::TARGET_DISPLAY`]
//! set: it opens its window on that display and writes what the window
//! receives to its standard output, one JSON object per line. Its log, on the
//! standard error it shares with the test, traces every event `AppKit`
//! dispatches in it and the key state each one meets. Run with
//! [`mac::OVERLAY_DISPLAY`] set instead, it covers that display with a
//! transparent window above the normal level that ignores mouse events, and
//! writes every input event it dispatches the same way.

use libtest_mimic::{Arguments, Trial};

fn main() {
    #[cfg(target_os = "macos")]
    if let Some(display) = std::env::var_os(mac::TARGET_DISPLAY) {
        mac::log_to_stderr("automation=trace");
        mac::serve_target_window(&display);
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(display) = std::env::var_os(mac::OVERLAY_DISPLAY) {
        mac::log_to_stderr("automation=trace");
        mac::serve_overlay_window(&display);
        return;
    }
    let mut arguments = Arguments::from_args();
    arguments.test_threads = Some(1);
    #[cfg(target_os = "macos")]
    mac::log_to_stderr("waterkit_automation=trace,automation=trace");
    #[cfg(target_os = "macos")]
    let trials = vec![
        Trial::test(
            "a_click_and_keys_reach_a_window_on_a_virtual_display",
            || {
                mac::a_click_and_keys_reach_a_window_on_a_virtual_display();
                Ok(())
            },
        ),
        Trial::test("a_click_and_keys_reach_a_window_of_another_process", || {
            mac::a_click_and_keys_reach_a_window_of_another_process();
            Ok(())
        }),
        Trial::test(
            "a_click_and_keys_pass_an_overlay_that_ignores_the_mouse",
            || {
                mac::a_click_and_keys_pass_an_overlay_that_ignores_the_mouse();
                Ok(())
            },
        ),
        Trial::test("input_needs_an_online_display", || {
            mac::input_needs_an_online_display();
            Ok(())
        }),
        Trial::test(
            "an_app_launches_in_the_background_onto_a_virtual_display",
            || {
                mac::an_app_launches_in_the_background_onto_a_virtual_display();
                Ok(())
            },
        ),
    ];
    #[cfg(not(target_os = "macos"))]
    let trials: Vec<Trial> = Vec::new();
    libtest_mimic::run(&arguments, trials).exit();
}

#[cfg(target_os = "macos")]
mod mac {
    use core::fmt;
    use core::num::NonZeroU32;
    use core::ptr::NonNull;
    use core::time::Duration;
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use block2::RcBlock;
    use dispatch2::DispatchQueue;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent,
        NSEventMask, NSEventType, NSFloatingWindowLevel, NSRunningApplication, NSScreen, NSView,
        NSWindow, NSWindowStyleMask, NSWorkspace,
    };
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGDisplayBounds, CGEvent, CGMainDisplayID};
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSNumber, NSPoint, NSProcessInfo, ns_string,
    };
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Serialize};
    use waterkit_automation::{
        Code, Error, Input, KeyDown, KeyUp, LaunchConfig, LaunchedApplication, ListedWindow,
        MouseButton, MouseDown, MouseUp, Point, Rect, Text, VirtualDisplay, VirtualDisplayConfig,
    };

    /// Set to a display id, this binary opens the target window on that
    /// display instead of running the tests.
    pub const TARGET_DISPLAY: &str = "WATERKIT_AUTOMATION_TARGET_DISPLAY";

    /// Set to a display id, this binary covers that display with a window
    /// that ignores mouse events instead of running the tests.
    pub const OVERLAY_DISPLAY: &str = "WATERKIT_AUTOMATION_OVERLAY_DISPLAY";

    /// The line the test writes to the overlay's standard input to learn
    /// what it has received.
    const SYNC_REQUEST: &str = "sync";

    /// Writes the logs `filter` selects to standard error.
    pub fn log_to_stderr(filter: &str) {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stderr)
            .init();
    }

    /// How long each posted event gets to arrive in the window, and the
    /// target application to report the window focused.
    const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);

    /// How long one post may take: the focus wait and the delivery wait, each
    /// up to [`DELIVERY_DEADLINE`].
    const POST_DEADLINE: Duration = Duration::from_secs(15);

    /// How long the other process gets to open its window on the display.
    const TARGET_READY_DEADLINE: Duration = Duration::from_secs(30);

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

    impl Observed {
        fn record(&mut self, report: Report) {
            match report {
                Report::Ready { .. } => {}
                Report::Press { x, y, clicks } => self.presses.push((NSPoint::new(x, y), clicks)),
                Report::Release { x, y } => self.releases.push(NSPoint::new(x, y)),
                Report::Key { code, characters } => self.keys.push((code, characters)),
            }
        }
    }

    /// One line the other process writes.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Report {
        /// The window is on screen where `WindowServer` lists it.
        Ready {
            pid: u32,
            window: u32,
            /// Whether the process believes it is active.
            active: bool,
            /// Whether the window is its key window.
            key: bool,
        },
        Press {
            x: f64,
            y: f64,
            clicks: isize,
        },
        Release {
            x: f64,
            y: f64,
        },
        Key {
            code: u16,
            characters: String,
        },
    }

    /// One line the overlay process writes.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum OverlayReport {
        /// The overlay is on screen where `WindowServer` lists it.
        Ready { pid: u32, window: u32 },
        /// An input event the overlay's application dispatched.
        Received { event: String },
        /// Every event that reached the overlay before the last sync request
        /// has been dispatched.
        Synced,
    }

    fn write_report(report: &impl Serialize) {
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, report).expect("the report serializes");
        stdout
            .write_all(b"\n")
            .and_then(|()| stdout.flush())
            .expect("the test reads the target's reports");
    }

    /// How the recorder takes a click and where it puts what it received.
    struct RecorderState {
        observed: RefCell<Observed>,
        /// Whether a click into the window while it is not key reaches the
        /// view. A text view, like `TextEdit`'s, refuses it.
        accepts_first_mouse: bool,
        /// Whether every event is also written to standard output.
        reports: bool,
    }

    impl RecorderState {
        fn record(&self, report: Report) {
            if self.reports {
                write_report(&report);
            }
            self.observed.borrow_mut().record(report);
        }
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitAutomationTestRecorder"]
        #[ivars = RecorderState]
        struct Recorder;

        impl Recorder {
            #[unsafe(method(acceptsFirstResponder))]
            fn accepts_first_responder(&self) -> bool {
                true
            }

            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, event: Option<&NSEvent>) -> bool {
                let accepts = self.ivars().accepts_first_mouse;
                tracing::debug!(
                    event = ?event.map(describe_event),
                    accepts,
                    "AppKit asked the view whether it takes a press into a window that is not key"
                );
                accepts
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, event: &NSEvent) {
                let location = event.locationInWindow();
                self.ivars().record(Report::Press {
                    x: location.x,
                    y: location.y,
                    clicks: event.clickCount(),
                });
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, event: &NSEvent) {
                let location = event.locationInWindow();
                self.ivars().record(Report::Release {
                    x: location.x,
                    y: location.y,
                });
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, event: &NSEvent) {
                let characters = event
                    .characters()
                    .map(|characters| characters.to_string())
                    .unwrap_or_default();
                self.ivars().record(Report::Key {
                    code: event.keyCode(),
                    characters,
                });
            }
        }
    );

    impl Recorder {
        fn new(
            mtm: MainThreadMarker,
            frame: CGRect,
            accepts_first_mouse: bool,
            reports: bool,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(RecorderState {
                observed: RefCell::default(),
                accepts_first_mouse,
                reports,
            });
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }
    }

    define_class!(
        /// A borderless window, which `AppKit` keeps where it is put, that can
        /// still become key. It traces every event `AppKit` hands it and every
        /// change of its key state.
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WaterkitAutomationTestWindow"]
        struct KeyableWindow;

        impl KeyableWindow {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key_window(&self) -> bool {
                true
            }

            #[unsafe(method(sendEvent:))]
            fn send_event(&self, event: &NSEvent) {
                let app = NSApplication::sharedApplication(self.mtm());
                let pointer = matches!(
                    event.r#type(),
                    NSEventType::LeftMouseDown | NSEventType::LeftMouseUp
                );
                let hits_view = pointer.then(|| {
                    self.contentView()
                        .and_then(|view| view.hitTest(event.locationInWindow()))
                        .is_some()
                });
                tracing::debug!(
                    event = %describe_event(event),
                    app_active = app.isActive(),
                    window_key = self.isKeyWindow(),
                    ?hits_view,
                    "the window received an event"
                );
                // SAFETY: `sendEvent:` is NSWindow's dispatch of `event`.
                let () = unsafe { msg_send![super(self), sendEvent: event] };
                tracing::debug!(
                    event = %describe_event(event),
                    app_active = app.isActive(),
                    window_key = self.isKeyWindow(),
                    "the window handled the event"
                );
            }

            #[unsafe(method(becomeKeyWindow))]
            fn become_key_window(&self) {
                tracing::debug!(window = self.windowNumber(), "the window becomes key");
                // SAFETY: NSWindow's own `becomeKeyWindow`.
                let () = unsafe { msg_send![super(self), becomeKeyWindow] };
            }

            #[unsafe(method(resignKeyWindow))]
            fn resign_key_window(&self) {
                tracing::debug!(window = self.windowNumber(), "the window resigns key");
                // SAFETY: NSWindow's own `resignKeyWindow`.
                let () = unsafe { msg_send![super(self), resignKeyWindow] };
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
            // A regular application places the content rect in global
            // coordinates on the main screen whatever screen it names, so the
            // window is moved onto `screen` explicitly.
            let screen_origin = screen.frame().origin;
            window.setFrameOrigin(NSPoint::new(
                screen_origin.x + frame.origin.x,
                screen_origin.y + frame.origin.y,
            ));
            window
        }

        fn number(&self) -> u32 {
            u32::try_from(self.windowNumber()).expect("window numbers are 32-bit")
        }
    }

    /// A local event monitor: `NSApp` hands it every event it dispatches,
    /// before dispatching it, until it drops.
    struct EventMonitor(Retained<AnyObject>);

    impl EventMonitor {
        fn install(on_event: impl Fn(&NSEvent) + 'static) -> Self {
            let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: `AppKit` hands the monitor a live event.
                on_event(unsafe { event.as_ref() });
                event.as_ptr()
            });
            // SAFETY: the handler returns the event it is given, so dispatch
            // goes on unchanged.
            let monitor = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Any, &handler)
            }
            .expect("AppKit installs a local event monitor");
            Self(monitor)
        }
    }

    impl Drop for EventMonitor {
        fn drop(&mut self) {
            // SAFETY: the monitor came from
            // `addLocalMonitorForEventsMatchingMask:handler:` and is removed
            // once.
            unsafe { NSEvent::removeMonitor(&self.0) };
        }
    }

    /// Every event `NSApp` dispatches, as a local event monitor sees it.
    struct EventLog {
        _monitor: EventMonitor,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl EventLog {
        fn install() -> Self {
            let events = Rc::new(RefCell::new(Vec::new()));
            let seen = Rc::clone(&events);
            let monitor = EventMonitor::install(move |event| {
                seen.borrow_mut().push(describe_event(event));
            });
            Self {
                _monitor: monitor,
                events,
            }
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

    /// Whether `event` is mouse or keyboard input.
    fn is_input(event: &NSEvent) -> bool {
        matches!(
            event.r#type(),
            NSEventType::LeftMouseDown
                | NSEventType::LeftMouseUp
                | NSEventType::RightMouseDown
                | NSEventType::RightMouseUp
                | NSEventType::OtherMouseDown
                | NSEventType::OtherMouseUp
                | NSEventType::MouseMoved
                | NSEventType::LeftMouseDragged
                | NSEventType::RightMouseDragged
                | NSEventType::OtherMouseDragged
                | NSEventType::ScrollWheel
                | NSEventType::KeyDown
                | NSEventType::KeyUp
                | NSEventType::FlagsChanged
        )
    }

    /// Dispatches every event already queued for this process.
    fn dispatch_queued(mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        let now = NSDate::distantPast();
        // SAFETY: the default run loop mode is an immutable constant.
        while let Some(event) = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&now),
                NSDefaultRunLoopMode,
                true,
            )
        } {
            app.sendEvent(&event);
        }
    }

    /// The user's cursor, read from a fresh event.
    fn cursor() -> CGPoint {
        let event = CGEvent::new(None).expect("CoreGraphics creates an event");
        CGEvent::location(Some(&event))
    }

    fn assert_cursor_unchanged(before: CGPoint) {
        let after = cursor();
        assert!(
            after.x.to_bits() == before.x.to_bits() && after.y.to_bits() == before.y.to_bits(),
            "the cursor moved from {before:?} to {after:?}"
        );
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
        delivered: impl FnMut() -> bool,
    ) -> (Input, Result<(), Error>, bool) {
        let poster = thread::spawn(move || {
            let result = post(&mut input);
            (input, result)
        });
        pump_until(mtm, POST_DEADLINE, || poster.is_finished());
        assert!(
            poster.is_finished(),
            "Input::post did not return within {POST_DEADLINE:?}"
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

    /// `AppKit`'s screen for `display`, once it lists one where
    /// `CoreGraphics` arranges the display. A window the target process
    /// placed from an earlier frame of the screen landed on the main display.
    fn appkit_screen(mtm: MainThreadMarker, display: u32) -> Option<Retained<NSScreen>> {
        let bounds = CGDisplayBounds(display);
        let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
        // `AppKit`'s frames have a bottom-left origin on the main display.
        let arranged = CGRect::new(
            CGPoint::new(
                bounds.origin.x,
                main_height - bounds.origin.y - bounds.size.height,
            ),
            bounds.size,
        );
        NSScreen::screens(mtm)
            .iter()
            .find(|screen| screen_display(screen) == display && screen.frame() == arranged)
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

    /// How `WindowServer` lists window `number`.
    fn describe_listed(inspector: &Input, number: u32) -> String {
        inspector
            .windows()
            .expect("the window list reads")
            .into_iter()
            .find(|listed| listed.id() == number)
            .map_or_else(|| "not at all".to_owned(), |listed| listed.to_string())
    }

    /// Where `AppKit` and `WindowServer` have the test window.
    fn describe_window(window: &KeyableWindow, inspector: &Input) -> String {
        format!(
            "test window {}: AppKit frame {:?} on display {:?}, visible {}, occlusion {:?}; WindowServer lists it as {}",
            window.number(),
            window.frame(),
            window.screen().map(|screen| screen_display(&screen)),
            window.isVisible(),
            window.occlusionState(),
            describe_listed(inspector, window.number()),
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

    /// Where the click lands, in the display's points.
    fn click_point() -> Point {
        Point::new(
            WINDOW_ORIGIN.x + CLICK_IN_WINDOW.x,
            WINDOW_ORIGIN.y + CLICK_IN_WINDOW.y,
        )
    }

    /// `at` on `display` in global display coordinates.
    fn global(display: u32, at: Point) -> CGPoint {
        let origin = CGDisplayBounds(display).origin;
        CGPoint::new(origin.x + at.x(), origin.y + at.y())
    }

    /// Whether `WindowServer` lists window `number` on screen under `at` on
    /// `display`, as an application window.
    fn listed_under(inspector: &Input, display: u32, number: u32, at: Point) -> bool {
        let point = global(display, at);
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
    }

    /// The layer of `number` when `WindowServer` lists it as a visible
    /// application window on screen under `at` on `display`. The full list
    /// is not in front-to-back order, so a window's layer is what puts it in
    /// front of another.
    fn layer_under(inspector: &Input, display: u32, number: u32, at: Point) -> Option<i32> {
        let point = global(display, at);
        inspector
            .windows()
            .expect("the window list reads")
            .iter()
            .find(|listed| {
                listed.id() == number
                    && listed.on_screen()
                    && listed.application()
                    && listed.alpha() > 0.0
                    && holds(listed.bounds(), point)
            })
            .map(ListedWindow::layer)
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

    /// `AppKit`'s screen for `display`, once it lists one.
    fn wait_for_screen(mtm: MainThreadMarker, display: u32) -> Retained<NSScreen> {
        let mut screen = None;
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || {
                screen = appkit_screen(mtm, display);
                screen.is_some()
            }),
            "AppKit never listed display {display} at {:?}: {}",
            CGDisplayBounds(display),
            describe_screens(mtm)
        );
        screen.expect("the wait ends once the screen is listed")
    }

    /// Moves `window` to `offset` from the bottom-left corner of `display`
    /// where it is now. macOS can move a display that just came online after
    /// a process first sees it, and a window placed before the move stays at
    /// its old global position, so a helper places its window again on every
    /// turn of its wait for `WindowServer` to list it under the click point.
    fn follow_display(mtm: MainThreadMarker, window: &NSWindow, display: u32, offset: CGPoint) {
        if let Some(screen) = appkit_screen(mtm, display) {
            let origin = screen.frame().origin;
            window.setFrameOrigin(NSPoint::new(origin.x + offset.x, origin.y + offset.y));
        }
    }

    /// Where [`open_window`] puts its window relative to the bottom-left
    /// corner of a screen `height` points tall.
    fn window_offset(height: f64) -> CGPoint {
        CGPoint::new(
            WINDOW_ORIGIN.x,
            height - WINDOW_ORIGIN.y - WINDOW_SIZE.height,
        )
    }

    /// A window on `display` whose content view records what it receives,
    /// placed on `AppKit`'s screen for the display.
    fn open_window(
        mtm: MainThreadMarker,
        display: u32,
        accepts_first_mouse: bool,
        reports: bool,
    ) -> (Retained<KeyableWindow>, Retained<Recorder>) {
        let screen = wait_for_screen(mtm, display);
        // The screen's frame has a bottom-left origin; the window's top edge
        // sits `WINDOW_ORIGIN.y` below the screen's.
        let frame = CGRect::new(window_offset(screen.frame().size.height), WINDOW_SIZE);
        let window = KeyableWindow::new(mtm, frame, &screen);
        let recorder = Recorder::new(
            mtm,
            CGRect::new(CGPoint::ZERO, WINDOW_SIZE),
            accepts_first_mouse,
            reports,
        );
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

    fn log_macos_version() {
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        eprintln!(
            "macOS {}.{}.{}",
            version.majorVersion, version.minorVersion, version.patchVersion
        );
    }

    /// Input that waits as long as the test gives each event.
    fn open_input(display: u32) -> Input {
        let mut input = Input::new(display).expect("input opens on the virtual display");
        input.set_confirm_timeout(DELIVERY_DEADLINE);
        input
    }

    pub fn a_click_and_keys_reach_a_window_on_a_virtual_display() {
        let mtm = MainThreadMarker::new().expect("libtest-mimic runs one thread on main");
        log_macos_version();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();

        let display = create_display(mtm);
        let (window, recorder) = open_window(mtm, display.id(), true, false);
        let log = EventLog::install();
        let cursor_before = cursor();
        let inspector = open_input(display.id());
        let input = open_input(display.id());
        let at = click_point();

        // `WindowServer` lists the window once it has ordered it in.
        let number = window.number();
        let listed = pump_until(mtm, DELIVERY_DEADLINE, || {
            listed_under(&inspector, display.id(), number, at)
        });
        assert!(
            listed,
            "WindowServer never listed the test window on screen under {at}\n{}\n{}",
            describe_window(&window, &inspector),
            describe_screens(mtm)
        );

        let observed = || recorder.ivars().observed.borrow();
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
        assert_cursor_unchanged(cursor_before);
        drop(log);
        window.close();
    }

    /// The target side of
    /// [`a_click_and_keys_reach_a_window_of_another_process`]: a regular
    /// application, as `TextEdit` is, whose view refuses a first click as a
    /// text view does. It reports when its window is on screen, then every
    /// event the window receives, until it is killed. Its log traces every
    /// event `NSApp` dispatches, with the active and key state it meets.
    pub fn serve_target_window(display: &OsStr) {
        let display = display_id(display);
        let _span = tracing::debug_span!("target", pid = std::process::id()).entered();
        let mtm = MainThreadMarker::new().expect("the target runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();
        let _monitor = EventMonitor::install(move |event| {
            let app = NSApplication::sharedApplication(mtm);
            tracing::debug!(
                event = %describe_event(event),
                app_active = app.isActive(),
                key_window = ?app.keyWindow().map(|key| key.windowNumber()),
                "NSApp is dispatching an event"
            );
        });
        let (window, _recorder) = open_window(mtm, display, false, true);
        let inspector = Input::new(display).expect("the target sees the virtual display");
        let number = window.number();
        let offset = window_offset(CGDisplayBounds(display).size.height);
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || {
                follow_display(mtm, &window, display, offset);
                listed_under(&inspector, display, number, click_point())
            }),
            "WindowServer never listed the target window on screen under {:?} of display {display} {:?}: {}; {}",
            click_point(),
            CGDisplayBounds(display),
            describe_window(&window, &inspector),
            describe_screens(mtm)
        );
        write_report(&Report::Ready {
            pid: std::process::id(),
            window: number,
            active: app.isActive(),
            key: window.isKeyWindow(),
        });
        app.run();
    }

    /// The overlay side of
    /// [`a_click_and_keys_pass_an_overlay_that_ignores_the_mouse`]: an
    /// accessory application, as the system processes that draw over the
    /// main display are, whose borderless, transparent window covers the
    /// whole display above the normal window level and ignores mouse events.
    /// It reports when its window is on screen, then every input event its
    /// application dispatches, and answers each [`SYNC_REQUEST`] on its
    /// standard input once it has dispatched every event queued for it,
    /// until it is killed.
    pub fn serve_overlay_window(display: &OsStr) {
        let display = display_id(display);
        let _span = tracing::debug_span!("overlay", pid = std::process::id()).entered();
        let mtm = MainThreadMarker::new().expect("the overlay runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        let _monitor = EventMonitor::install(|event| {
            if is_input(event) {
                let event = describe_event(event);
                tracing::debug!(%event, "the overlay received input");
                write_report(&OverlayReport::Received { event });
            }
        });
        let screen = wait_for_screen(mtm, display);
        let frame = CGRect::new(CGPoint::ZERO, screen.frame().size);
        // SAFETY: an initializer of NSWindow, with a valid style, backing and
        // screen.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer_screen(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
                Some(&screen),
            )
        };
        // SAFETY: the overlay owns the window through `Retained`; closing it
        // must not release it a second time.
        unsafe { window.setReleasedWhenClosed(false) };
        // As in `KeyableWindow::new`, the window is moved onto `screen`.
        window.setFrameOrigin(screen.frame().origin);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSFloatingWindowLevel);
        window.orderFrontRegardless();

        let inspector = Input::new(display).expect("the overlay sees the virtual display");
        let number = u32::try_from(window.windowNumber()).expect("window numbers are 32-bit");
        assert!(
            pump_until(mtm, DELIVERY_DEADLINE, || {
                follow_display(mtm, &window, display, CGPoint::ZERO);
                listed_under(&inspector, display, number, click_point())
            }),
            "WindowServer never listed the overlay window on screen under {:?} of display {display} {:?}: {}; {}",
            click_point(),
            CGDisplayBounds(display),
            describe_listed(&inspector, number),
            describe_screens(mtm)
        );
        write_report(&OverlayReport::Ready {
            pid: std::process::id(),
            window: number,
        });
        thread::spawn(|| {
            for line in std::io::stdin().lines() {
                let line = line.expect("the overlay's input reads");
                assert_eq!(line, SYNC_REQUEST, "the test sends only sync requests");
                DispatchQueue::main().exec_async(|| {
                    let mtm = MainThreadMarker::new().expect("the main queue runs on main");
                    dispatch_queued(mtm);
                    write_report(&OverlayReport::Synced);
                });
            }
        });
        app.run();
    }

    /// The display id a helper mode is set to.
    fn display_id(display: &OsStr) -> u32 {
        display
            .to_str()
            .and_then(|display| display.parse().ok())
            .expect("the helper's display is a display id")
    }

    /// This binary run again in a helper mode, killed on drop, and the
    /// reports it writes.
    struct Helper<R> {
        child: Child,
        stdin: ChildStdin,
        reports: mpsc::Receiver<R>,
    }

    impl<R: DeserializeOwned + fmt::Debug + Send + 'static> Helper<R> {
        /// This binary with the environment variable `mode` set to
        /// `display`.
        fn spawn(mode: &'static str, display: u32) -> Self {
            let mut child =
                Command::new(std::env::current_exe().expect("the test binary has a path"))
                    .env(mode, display.to_string())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap_or_else(|error| panic!("the {mode} helper does not start: {error}"));
            let stdin = child.stdin.take().expect("the helper's input is piped");
            let stdout = child.stdout.take().expect("the helper's output is piped");
            let (sender, reports) = mpsc::channel();
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let line = line.expect("the helper's output reads");
                    let report = serde_json::from_str(&line).unwrap_or_else(|error| {
                        panic!("the {mode} helper wrote {line:?}: {error}")
                    });
                    tracing::debug!(mode, ?report, "a helper reported");
                    if sender.send(report).is_err() {
                        return;
                    }
                }
            });
            Self {
                child,
                stdin,
                reports,
            }
        }

        /// The next report, within `timeout`.
        fn next(&self, timeout: Duration) -> Option<R> {
            self.reports.recv_timeout(timeout).ok()
        }
    }

    impl<R> Drop for Helper<R> {
        fn drop(&mut self) {
            self.child.kill().expect("the helper process is killed");
            self.child.wait().expect("the helper process is reaped");
        }
    }

    /// The other process's window, and what it received.
    struct TargetProcess {
        process: Helper<Report>,
        pid: u32,
        window: u32,
        /// Whether the process believed it was active, and the window was
        /// key, when the window came on screen.
        active: bool,
        key: bool,
        observed: Observed,
    }

    impl TargetProcess {
        /// The target process, once its window is on screen.
        fn spawn(display: u32) -> Self {
            let process = Helper::spawn(TARGET_DISPLAY, display);
            // Ready comes first: a report before it would fail the match.
            match process.next(TARGET_READY_DEADLINE) {
                Some(Report::Ready {
                    pid,
                    window,
                    active,
                    key,
                }) => Self {
                    process,
                    pid,
                    window,
                    active,
                    key,
                    observed: Observed::default(),
                },
                other => panic!(
                    "the target process did not report its window within {TARGET_READY_DEADLINE:?}, got {other:?}"
                ),
            }
        }

        /// Records reports until `done` holds of what the window received;
        /// false when [`DELIVERY_DEADLINE`] passes first.
        fn until(&mut self, mut done: impl FnMut(&Observed) -> bool) -> bool {
            let deadline = Instant::now() + DELIVERY_DEADLINE;
            while !done(&self.observed) {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.process.reports.recv_timeout(remaining) {
                    Ok(report) => self.observed.record(report),
                    Err(_) => return false,
                }
            }
            true
        }

        /// What the target received and where `WindowServer` has its window.
        fn describe(&self, mtm: MainThreadMarker, input: &Input) -> String {
            let (pid, number) = (self.pid, self.window);
            format!(
                "the target received {:?}; when its window {number} came on screen, process {pid} reported itself active {}, with the window key {}; WindowServer lists the window as {}\n{}\nThe log above lists what Input posted, and under target{{pid={pid}}} every event AppKit dispatched in the target.",
                self.observed,
                self.active,
                self.key,
                describe_listed(input, number),
                describe_screens(mtm),
            )
        }
    }

    /// The overlay process's window, which must receive nothing.
    struct OverlayProcess {
        process: Helper<OverlayReport>,
        pid: u32,
        window: u32,
    }

    impl OverlayProcess {
        /// The overlay process, once its window is on screen.
        fn spawn(display: u32) -> Self {
            let process = Helper::spawn(OVERLAY_DISPLAY, display);
            match process.next(TARGET_READY_DEADLINE) {
                Some(OverlayReport::Ready { pid, window }) => Self {
                    process,
                    pid,
                    window,
                },
                other => panic!(
                    "the overlay process did not report its window within {TARGET_READY_DEADLINE:?}, got {other:?}"
                ),
            }
        }

        /// The input events the overlay has dispatched, once it has
        /// dispatched every event queued for it.
        fn received(&mut self) -> Vec<String> {
            writeln!(self.process.stdin, "{SYNC_REQUEST}")
                .and_then(|()| self.process.stdin.flush())
                .expect("the overlay reads its requests");
            let deadline = Instant::now() + DELIVERY_DEADLINE;
            let mut received = Vec::new();
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.process.reports.recv_timeout(remaining) {
                    Ok(OverlayReport::Received { event }) => received.push(event),
                    Ok(OverlayReport::Synced) => return received,
                    Ok(other) => panic!("the overlay reported {other:?} after it was ready"),
                    Err(error) => panic!(
                        "the overlay did not answer a sync request within {DELIVERY_DEADLINE:?} ({error}), having received {received:?}"
                    ),
                }
            }
        }
    }

    /// Clicks [`click_point`] and types "hi" and the A key with `input`,
    /// asserting that each step reaches `target`'s window, then that the
    /// window received exactly that.
    fn click_and_type(mtm: MainThreadMarker, input: &mut Input, target: &mut TargetProcess) {
        let at = click_point();
        let pressed = input.post(MouseDown::new(at, MouseButton::Left));
        assert!(
            pressed.is_ok() && target.until(|observed| !observed.presses.is_empty()),
            "the press ({}) never reached the other process's window: {}",
            outcome(&pressed),
            target.describe(mtm, input)
        );

        let released = input.post(MouseUp::new(at, MouseButton::Left));
        assert!(
            released.is_ok() && target.until(|observed| !observed.releases.is_empty()),
            "the release ({}) never reached the other process's window: {}",
            outcome(&released),
            target.describe(mtm, input)
        );

        let typed = input
            .post(Text::new("hi"))
            .and_then(|()| input.post(KeyDown::new(Code::KeyA)))
            .and_then(|()| input.post(KeyUp::new(Code::KeyA)));
        assert!(
            typed.is_ok() && target.until(|observed| observed.keys.len() >= 2),
            "the key presses ({}) never reached the other process's window: {}",
            outcome(&typed),
            target.describe(mtm, input)
        );

        assert_delivered(&target.observed);
    }

    pub fn a_click_and_keys_reach_a_window_of_another_process() {
        let mtm = MainThreadMarker::new().expect("libtest-mimic runs one thread on main");
        log_macos_version();
        let display = create_display(mtm);
        let mut target = TargetProcess::spawn(display.id());
        let cursor_before = cursor();
        let mut input = open_input(display.id());
        click_and_type(mtm, &mut input, &mut target);
        assert_cursor_unchanged(cursor_before);
    }

    /// A third process covers the display, the target's window included,
    /// with a transparent window above the normal level that ignores mouse
    /// events, as `NotificationCenter` and the Dock do on the main display.
    /// `WindowServer` lists it in front of the target under the click, so a
    /// press picked by window bounds would go to it; the click and keys still
    /// reach the target, and the overlay receives nothing.
    pub fn a_click_and_keys_pass_an_overlay_that_ignores_the_mouse() {
        let mtm = MainThreadMarker::new().expect("libtest-mimic runs one thread on main");
        log_macos_version();
        let display = create_display(mtm);
        let mut target = TargetProcess::spawn(display.id());
        let mut overlay = OverlayProcess::spawn(display.id());
        let cursor_before = cursor();
        let mut input = open_input(display.id());
        let at = click_point();
        let overlay_layer = layer_under(&input, display.id(), overlay.window, at);
        let target_layer = layer_under(&input, display.id(), target.window, at);
        assert!(
            matches!((overlay_layer, target_layer), (Some(over), Some(under)) if over > under),
            "the overlay window {} is not listed under {at} above the target window {} (layers {overlay_layer:?} over {target_layer:?}), so the press would not pass it: the overlay is listed as {}; {}",
            overlay.window,
            target.window,
            describe_listed(&input, overlay.window),
            target.describe(mtm, &input)
        );

        click_and_type(mtm, &mut input, &mut target);
        let received = overlay.received();
        assert!(
            received.is_empty(),
            "the overlay window {} of pid {} received {received:?}",
            overlay.window,
            overlay.pid
        );
        assert_cursor_unchanged(cursor_before);
    }

    pub fn input_needs_an_online_display() {
        let result = Input::new(u32::MAX);
        assert!(
            matches!(result, Err(Error::DisplayNotFound(u32::MAX))),
            "an unknown display is refused, got {result:?}"
        );
    }

    /// The stock application the launch case starts: one window, which it
    /// opens on launch.
    const LAUNCHED_APP: &str = "com.apple.calculator";

    /// The application a launch started, force-quit on drop.
    struct Launched(Retained<NSRunningApplication>);

    impl Drop for Launched {
        fn drop(&mut self) {
            if !self.0.isTerminated() {
                assert!(
                    self.0.forceTerminate() || thread::panicking(),
                    "the launched application is force-quit"
                );
            }
        }
    }

    /// The front application's process, as `NSWorkspace` reports it once
    /// the main run loop has handled what was queued for it.
    fn frontmost_pid(mtm: MainThreadMarker) -> Option<i32> {
        dispatch_queued(mtm);
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|application| application.processIdentifier())
    }

    /// A new instance of a stock application launches in the background with
    /// its window on the virtual display: the front application stays the
    /// same, the launched one never becomes active, and the cursor stays.
    pub fn an_app_launches_in_the_background_onto_a_virtual_display() {
        let mtm = MainThreadMarker::new().expect("libtest-mimic runs one thread on main");
        log_macos_version();
        // `AppKit` dispatches the events this case pumps; an accessory
        // application never becomes the front one itself.
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        let display = create_display(mtm);
        let front_before = frontmost_pid(mtm);
        let cursor_before = cursor();

        let config = LaunchConfig::bundle_id(LAUNCHED_APP)
            .new_instance(true)
            .display(display.id());
        let launched = LaunchedApplication::launch(&config)
            .unwrap_or_else(|error| panic!("{LAUNCHED_APP} does not launch: {error}"));
        let running = NSRunningApplication::runningApplicationWithProcessIdentifier(launched.pid())
            .expect("the launched application is running");
        let running = Launched(running);
        assert_eq!(launched.bundle_id(), LAUNCHED_APP);

        // An application that activates itself does so while it finishes
        // launching.
        assert!(
            pump_until(mtm, TARGET_READY_DEADLINE, || running
                .0
                .isFinishedLaunching()),
            "{LAUNCHED_APP} (pid {}) did not finish launching within {TARGET_READY_DEADLINE:?}",
            launched.pid()
        );
        assert_eq!(
            frontmost_pid(mtm),
            front_before,
            "the front application changed when {LAUNCHED_APP} (pid {}) launched",
            launched.pid()
        );
        assert!(
            !running.0.isActive(),
            "{LAUNCHED_APP} (pid {}) became active",
            launched.pid()
        );

        let bounds = CGDisplayBounds(display.id());
        let right = bounds.origin.x + bounds.size.width;
        let bottom = bounds.origin.y + bounds.size.height;
        assert!(
            !launched.windows().is_empty(),
            "{LAUNCHED_APP} returned no window"
        );
        for window in launched.windows() {
            let frame = window.bounds();
            let inside = frame.x() >= bounds.origin.x
                && frame.y() >= bounds.origin.y
                && frame.x() + frame.width() <= right
                && frame.y() + frame.height() <= bottom;
            assert!(
                inside,
                "{window} does not lie within display {} {bounds:?}",
                display.id()
            );
            assert_eq!(
                window.pid(),
                launched.pid(),
                "{window} is the launched app's"
            );
        }
        assert_cursor_unchanged(cursor_before);
        drop(running);
    }
}
