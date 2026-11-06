//! Display streams on a virtual display this test creates, so nothing on the
//! machine's own displays is captured. Needs the Screen Recording grant for
//! the process responsible for the test binary.

//!
//! The target owns the process main thread (`harness = false`, one test
//! thread): `CoreGraphics` refreshes its per-process display view only when
//! callbacks are serviced on the event thread, so the display is created there
//! and the main run loop is spun while it is awaited.

use libtest_mimic::{Arguments, Trial};

/// Hidden argument: print `CGGetOnlineDisplayList` as seen by a fresh process
/// and exit.
#[cfg(target_os = "macos")]
const PRINT_ONLINE_ARG: &str = "--print-online-displays";

fn main() {
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some(PRINT_ONLINE_ARG) {
        println!("{:?}", mac::online_displays());
        return;
    }
    let mut arguments = Arguments::from_args();
    arguments.test_threads = Some(1);
    #[cfg(target_os = "macos")]
    let trials = vec![
        Trial::test("a_virtual_display_streams_frames", || {
            mac::a_virtual_display_streams_frames();
            Ok(())
        }),
        Trial::test("an_unknown_display_is_not_found", || {
            mac::an_unknown_display_is_not_found();
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
    use std::ffi::{CStr, c_char, c_void};
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use dispatch2::DispatchQueue;
    use futures::future::{self, Either};
    use futures::{Stream, StreamExt};
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Method};
    use objc2::{msg_send, sel};
    use objc2_core_foundation::{CFRunLoop, CGSize, kCFRunLoopDefaultMode};
    use objc2_core_graphics::{
        CGDirectDisplayID, CGDisplayChangeSummaryFlags, CGDisplayIsActive, CGDisplayIsBuiltin,
        CGDisplayIsInMirrorSet, CGDisplayIsOnline, CGDisplayModelNumber,
        CGDisplayRegisterReconfigurationCallback, CGDisplayRemoveReconfigurationCallback,
        CGDisplayVendorNumber, CGError, CGGetOnlineDisplayList,
    };
    use objc2_foundation::{NSArray, NSNumber, NSString};
    use waterkit_screen::{
        DisplayStream, DisplayStreamConfig, Error, PixelBuffer, PixelFormat, ScreenInfo,
        ScreenStream, StreamConfig, screens,
    };

    #[link(name = "SkyLight", kind = "framework")]
    unsafe extern "C" {
        /// The displays `WindowServer` has registered, online or not.
        fn SLSGetDisplayList(max: u32, list: *mut u32, count: *mut u32) -> i32;
        /// The displays `WindowServer` has brought into the topology.
        fn SLSGetOnlineDisplayList(max: u32, list: *mut u32, count: *mut u32) -> i32;
    }

    unsafe extern "C" {
        fn method_getTypeEncoding(method: *const Method) -> *const c_char;
    }

    /// The encoding of `-[SLWindowMirroringManager extend:]` the fallback is
    /// written against.
    const EXTEND_ENCODING: &CStr = c"B24@0:8@16";

    /// How long after `applySettings:` a registered, offline display waits before
    /// `extend:` is tried.
    const EXTEND_AFTER: Duration = Duration::from_secs(2);

    /// Interval between probes.
    const PROBE_INTERVAL: Duration = Duration::from_millis(250);

    /// How long the capture service gets to deliver a frame.
    const FRAME_DEADLINE: Duration = Duration::from_secs(5);

    fn class(name: &CStr) -> &'static AnyClass {
        AnyClass::get(name)
            .unwrap_or_else(|| panic!("{} is unavailable on this macOS", name.to_string_lossy()))
    }

    /// The displays `CoreGraphics` reports online.
    pub fn online_displays() -> Vec<u32> {
        let mut ids = [0_u32; 32];
        let mut count = 0_u32;
        // SAFETY: `ids` holds 32 entries and `count` is a valid out pointer.
        let status = unsafe { CGGetOnlineDisplayList(32, ids.as_mut_ptr(), &raw mut count) };
        assert_eq!(status, CGError::Success, "CGGetOnlineDisplayList failed");
        ids.into_iter().take(count as usize).collect()
    }

    /// How long a new virtual display gets to go online.
    const ONLINE_DEADLINE: Duration = Duration::from_secs(5);

    /// The displays `CoreGraphics` reported added since `watch`.
    struct DisplayAdditions {
        added: Box<Mutex<Vec<u32>>>,
    }

    unsafe extern "C-unwind" fn record_addition(
        display: CGDirectDisplayID,
        flags: CGDisplayChangeSummaryFlags,
        added: *mut c_void,
    ) {
        if flags.contains(CGDisplayChangeSummaryFlags::AddFlag) {
            // SAFETY: `added` is the `Mutex` `DisplayAdditions` registered, which
            // outlives the callback's registration.
            let added = unsafe { &*added.cast::<Mutex<Vec<u32>>>() };
            added
                .lock()
                .expect("the addition list is not poisoned")
                .push(display);
        }
    }

    impl DisplayAdditions {
        fn watch() -> Self {
            let added = Box::new(Mutex::new(Vec::new()));
            let info = (&raw const *added).cast_mut().cast::<c_void>();
            // SAFETY: the callback matches the registered signature and `info`
            // stays valid until `drop` unregisters it.
            let status =
                unsafe { CGDisplayRegisterReconfigurationCallback(Some(record_addition), info) };
            assert_eq!(
                status,
                CGError::Success,
                "registering the display callback failed"
            );
            Self { added }
        }

        fn added(&self, id: u32) -> bool {
            self.added
                .lock()
                .expect("the addition list is not poisoned")
                .contains(&id)
        }

        /// Waits until `CoreGraphics` lists `id` online and active, probing as it
        /// goes and trying `extend:` when `WindowServer` registered the display
        /// without bringing it online. The reconfiguration callbacks are
        /// delivered through the main run loop, which this spins.
        fn wait_until_online(&self, id: u32, log: &mut ProbeLog) {
            let start = Instant::now();
            let mut extended = false;
            let mut tick = 0_u32;
            log.probe("apply", id, self);
            loop {
                if CGDisplayIsOnline(id) && CGDisplayIsActive(id) {
                    return;
                }
                if start.elapsed() >= ONLINE_DEADLINE {
                    return;
                }
                if !extended
                    && start.elapsed() >= EXTEND_AFTER
                    && sls_contains(SLS_REGISTERED, id)
                    && !sls_contains(SLS_ONLINE, id)
                {
                    extended = true;
                    let result = extend(id);
                    log.note(&format!("vd-extend id={id} result={result}"));
                }
                CFRunLoop::run_in_mode(
                    unsafe { kCFRunLoopDefaultMode },
                    PROBE_INTERVAL.as_secs_f64(),
                    false,
                );
                tick += 1;
                log.probe(
                    &format!("t+{}ms", u128::from(tick) * PROBE_INTERVAL.as_millis()),
                    id,
                    self,
                );
            }
        }
    }

    type SlsList = unsafe extern "C" fn(u32, *mut u32, *mut u32) -> i32;
    const SLS_REGISTERED: SlsList = SLSGetDisplayList;
    const SLS_ONLINE: SlsList = SLSGetOnlineDisplayList;

    fn sls_list(list: SlsList) -> Vec<u32> {
        let mut ids = [0_u32; 32];
        let mut count = 0_u32;
        // SAFETY: `ids` holds 32 entries and `count` is a valid out pointer.
        let status = unsafe { list(32, ids.as_mut_ptr(), &raw mut count) };
        assert_eq!(status, 0, "a SkyLight display list call failed");
        ids.into_iter().take(count as usize).collect()
    }

    fn sls_contains(list: SlsList, id: u32) -> bool {
        sls_list(list).contains(&id)
    }

    /// `-[SLWindowMirroringManager extend:]`, the only call that brings a
    /// registered display into the topology. Returns the call's result.
    fn extend(id: u32) -> bool {
        let manager_class = class(c"SLWindowMirroringManager");
        let encoding = manager_class
            .instance_method(sel!(extend:))
            .map(|method| {
                // SAFETY: `method` is a live method of the class; the encoding is
                // a NUL-terminated string owned by the runtime.
                unsafe { CStr::from_ptr(method_getTypeEncoding(core::ptr::from_ref(method))) }
            })
            .expect("SLWindowMirroringManager has no extend:");
        assert_eq!(
            encoding, EXTEND_ENCODING,
            "-[SLWindowMirroringManager extend:] has encoding {encoding:?}, expected {EXTEND_ENCODING:?}"
        );
        assert!(
            manager_class
                .metaclass()
                .instance_method(sel!(shared))
                .is_some(),
            "SLWindowMirroringManager has no +shared"
        );
        let number = NSNumber::new_u32(id);
        // SAFETY: `+shared` takes nothing and returns the manager; `extend:`
        // matches the checked encoding (BOOL, one object argument).
        unsafe {
            let manager: Retained<AnyObject> = msg_send![manager_class, shared];
            msg_send![&manager, extend: &*number]
        }
    }

    /// The probe lines of one run, echoed to stderr as they are taken.
    struct ProbeLog {
        lines: Vec<String>,
    }

    impl ProbeLog {
        fn note(&mut self, line: &str) {
            eprintln!("{line}");
            self.lines.push(line.to_owned());
        }

        fn probe(&mut self, phase: &str, id: u32, additions: &DisplayAdditions) {
            let sls_registered = sls_contains(SLS_REGISTERED, id);
            let sls_online = sls_contains(SLS_ONLINE, id);
            let child = child_online_displays();
            let line = format!(
                "vd-probe phase={phase} id={id} cb_add={} sls_registered={} sls_online={} cg_online={} cg_active={} in_mirror_set={} child_cg_online={child}",
                u8::from(additions.added(id)),
                u8::from(sls_registered),
                u8::from(sls_online),
                u8::from(CGDisplayIsOnline(id)),
                u8::from(CGDisplayIsActive(id)),
                u8::from(CGDisplayIsInMirrorSet(id)),
            );
            self.note(&line);
        }
    }

    /// `CGGetOnlineDisplayList` as a freshly started process reads it.
    fn child_online_displays() -> String {
        let output = Command::new(std::env::current_exe().expect("the test binary has a path"))
            .arg(crate::PRINT_ONLINE_ARG)
            .output()
            .expect("the test binary re-executes");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// The lines of `command`'s output that mention one of `needles`.
    fn filtered_output(command: &mut Command, needles: &[&str]) -> String {
        match command.output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| needles.iter().any(|needle| line.contains(needle)))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => format!("(could not run: {error})"),
        }
    }

    fn command_line(program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command.args(args);
        command
    }

    fn stdout_of(program: &str, args: &[&str]) -> String {
        let output = command_line(program, args)
            .output()
            .unwrap_or_else(|error| panic!("{program} did not run: {error}"));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    impl Drop for DisplayAdditions {
        fn drop(&mut self) {
            let info = (&raw const *self.added).cast_mut().cast::<c_void>();
            // SAFETY: the registration made in `watch` is removed with the same
            // callback and pointer.
            let status =
                unsafe { CGDisplayRemoveReconfigurationCallback(Some(record_addition), info) };
            assert!(
                status == CGError::Success || std::thread::panicking(),
                "unregistering the display callback failed"
            );
        }
    }

    /// Panel size in points; the descriptor's maximum is twice that in pixels.
    const WIDTH: u32 = 1280;
    const HEIGHT: u32 = 800;

    /// A `CGVirtualDisplay` (private `CoreGraphics` API) of 1280x800 points at 2x
    /// backing scale, described as a 13" panel. Dropping it removes the display.
    struct VirtualDisplay {
        _display: Retained<AnyObject>,
        id: u32,
    }

    impl VirtualDisplay {
        /// Must run on the main thread, which it spins while the display comes
        /// online. Fails with the probe log and system diagnostics when the
        /// display never becomes online and active outside a mirror set.
        fn create() -> Self {
            let (width, height) = (WIDTH, HEIGHT);
            let mut log = ProbeLog { lines: Vec::new() };
            log.note(&format!(
                "vd-env macos={} display1_vendor={:#x} display1_model={:#x} display1_builtin={}",
                stdout_of("sw_vers", &["-productVersion"]),
                CGDisplayVendorNumber(1),
                CGDisplayModelNumber(1),
                u8::from(CGDisplayIsBuiltin(1)),
            ));
            // Registered first: the display goes online asynchronously after
            // `applySettings:`.
            let additions = DisplayAdditions::watch();
            log.probe("baseline", 0, &additions);
            let queue = DispatchQueue::main();
            // SAFETY: the messages match the private classes' selectors; every
            // object is owned by a `Retained`; the main queue is an object
            // (`OS_dispatch_queue`) that outlives the descriptor.
            let (display, id) = unsafe {
                let descriptor: Retained<AnyObject> =
                    msg_send![class(c"CGVirtualDisplayDescriptor"), new];
                let queue_object: &AnyObject = &*(&raw const *queue).cast::<AnyObject>();
                let _: () = msg_send![&descriptor, setQueue: queue_object];
                let name = NSString::from_str("waterkit display stream test");
                let _: () = msg_send![&descriptor, setName: &*name];
                let _: () = msg_send![&descriptor, setMaxPixelsWide: width * 2];
                let _: () = msg_send![&descriptor, setMaxPixelsHigh: height * 2];
                let millimetres = CGSize::new(296.0, 185.0);
                let _: () = msg_send![&descriptor, setSizeInMillimeters: millimetres];
                let _: () = msg_send![&descriptor, setVendorID: 0xF0F0_u32];
                let _: () = msg_send![&descriptor, setProductID: 0xF0F1_u32];
                let serial = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("the clock is past the epoch")
                    .subsec_nanos()
                    | 1;
                let _: () = msg_send![&descriptor, setSerialNum: serial];

                let allocated: Allocated<AnyObject> = msg_send![class(c"CGVirtualDisplay"), alloc];
                let display: Retained<AnyObject> =
                    msg_send![allocated, initWithDescriptor: &*descriptor];

                // A HiDPI mode is sized in points; its pixels are twice that,
                // within the descriptor's maximum.
                let settings: Retained<AnyObject> =
                    msg_send![class(c"CGVirtualDisplaySettings"), new];
                let _: () = msg_send![&settings, setHiDPI: 1_u32];
                let allocated: Allocated<AnyObject> =
                    msg_send![class(c"CGVirtualDisplayMode"), alloc];
                let mode: Retained<AnyObject> = msg_send![
                    allocated,
                    initWithWidth: width,
                    height: height,
                    refreshRate: 60.0_f64
                ];
                let modes = NSArray::from_retained_slice(&[mode]);
                let _: () = msg_send![&settings, setModes: &*modes];
                let applied: bool = msg_send![&display, applySettings: &*settings];
                assert!(applied, "CoreGraphics refused the virtual display settings");
                let id: u32 = msg_send![&display, displayID];
                (display, id)
            };
            assert_ne!(id, 0, "CoreGraphics assigned the virtual display no id");
            additions.wait_until_online(id, &mut log);
            let healthy =
                CGDisplayIsOnline(id) && CGDisplayIsActive(id) && !CGDisplayIsInMirrorSet(id);
            if !healthy {
                let needles = [
                    id.to_string(),
                    "tvConnected".into(),
                    "pending".into(),
                    "noChoiceMade".into(),
                    "bring display".into(),
                ];
                let needles: Vec<&str> = needles.iter().map(String::as_str).collect();
                let profiler = filtered_output(
                    &mut command_line("system_profiler", &["SPDisplaysDataType"]),
                    &needles,
                );
                let windowserver = filtered_output(
                    &mut command_line(
                        "log",
                        &[
                            "show",
                            "--last",
                            "2m",
                            "--style",
                            "compact",
                            "--predicate",
                            "process == \"WindowServer\" OR process == \"ControlCenter\"",
                        ],
                    ),
                    &needles,
                );
                panic!(
                    "virtual display {id} is not online, active and outside a mirror set\n{}\n--- system_profiler SPDisplaysDataType (filtered)\n{profiler}\n--- log show WindowServer/ControlCenter (filtered)\n{windowserver}",
                    log.lines.join("\n")
                );
            }
            Self {
                _display: display,
                id,
            }
        }
    }

    const fn size(pixels: u32) -> NonZeroU32 {
        NonZeroU32::new(pixels).expect("a test size is non-zero")
    }

    /// The stream's next item, or `None` when it yields nothing in time.
    async fn within_deadline<S: Stream + Unpin>(items: &mut S) -> Option<Option<S::Item>> {
        match future::select(items.next(), futures_timer::Delay::new(FRAME_DEADLINE)).await {
            Either::Left((item, _)) => Some(item),
            Either::Right(((), _)) => None,
        }
    }

    /// The next frame, failing the test when none arrives in time.
    async fn next_frame(frames: &mut (impl Stream<Item = PixelBuffer> + Unpin)) -> PixelBuffer {
        within_deadline(frames)
            .await
            .unwrap_or_else(|| panic!("no frame arrived within {FRAME_DEADLINE:?}"))
            .expect("the frame stream ended")
    }

    fn assert_frame(frame: &PixelBuffer, width: u32, height: u32, format: PixelFormat) {
        assert_eq!((frame.width(), frame.height()), (width, height));
        assert_eq!(frame.pixel_format(), format);
        let surface = frame.io_surface();
        assert_eq!(
            (surface.width(), surface.height()),
            (width as usize, height as usize),
            "the pixel buffer's IOSurface holds the frame"
        );
    }

    const fn assert_send<T: Send>(value: T) -> T {
        value
    }

    async fn display_stream_delivers_requested_frames(display_id: u32) {
        let config = DisplayStreamConfig::new(size(1280), size(800))
            .frame_rate(size(30))
            .pixel_format(PixelFormat::Bgra8);
        let mut stream = assert_send(DisplayStream::new(display_id, config))
            .await
            .expect("a display stream opens on the virtual display");
        let mut frames = Box::pin(stream.frames());

        assert_send(stream.resume()).await.expect("capture starts");
        assert_frame(
            &next_frame(&mut frames).await,
            1280,
            800,
            PixelFormat::Bgra8,
        );

        assert_send(stream.pause()).await.expect("capture pauses");
        stream.resume().await.expect("capture resumes");
        assert_frame(
            &next_frame(&mut frames).await,
            1280,
            800,
            PixelFormat::Bgra8,
        );

        drop(stream);
        // Frames captured before the drop may still be queued; then the frames
        // end.
        loop {
            match within_deadline(&mut frames).await {
                Some(Some(_queued)) => {}
                Some(None) => break,
                None => panic!("the frames did not end after their DisplayStream dropped"),
            }
        }

        let config =
            DisplayStreamConfig::new(size(640), size(400)).pixel_format(PixelFormat::Ycbcr420Video);
        let mut stream = DisplayStream::new(display_id, config)
            .await
            .expect("a second display stream opens");
        let mut frames = Box::pin(stream.frames());
        stream.resume().await.expect("capture starts");
        assert_frame(
            &next_frame(&mut frames).await,
            640,
            400,
            PixelFormat::Ycbcr420Video,
        );
    }

    /// The existing wgpu stream on the same display. Its `try_next_frame` does
    /// not wait, so the frame is polled for.
    #[expect(
        clippy::future_not_send,
        reason = "the wgpu ScreenStream's capturer is not Send on macOS"
    )]
    async fn wgpu_stream_delivers_frames(info: &ScreenInfo) {
        let adapter = wgpu::Instance::default()
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("a GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("a GPU device");
        let stream = ScreenStream::start(
            info,
            Arc::new(device),
            Arc::new(queue),
            &StreamConfig::default(),
        )
        .await
        .expect("the wgpu stream starts on the virtual display");
        let deadline = std::time::Instant::now() + FRAME_DEADLINE;
        let frame = loop {
            if let Some(frame) = stream.try_next_frame() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the wgpu stream delivered no frame within {FRAME_DEADLINE:?}"
            );
            futures_timer::Delay::new(Duration::from_millis(20)).await;
        };
        assert!(frame.width() > 0 && frame.height() > 0);
        assert_eq!(
            (frame.texture().width(), frame.texture().height()),
            (frame.width(), frame.height())
        );
    }

    /// One virtual display serves every capture check: creating several at once
    /// makes `CoreGraphics` refuse some of them.
    pub fn a_virtual_display_streams_frames() {
        let display = VirtualDisplay::create();
        let info = screens()
            .expect("displays are listed")
            .into_iter()
            .find(|screen| screen.id() == display.id)
            .expect("the virtual display is listed");
        pollster::block_on(async {
            display_stream_delivers_requested_frames(display.id).await;
            wgpu_stream_delivers_frames(&info).await;
        });
    }

    pub fn an_unknown_display_is_not_found() {
        let result = pollster::block_on(DisplayStream::new(
            u32::MAX,
            DisplayStreamConfig::new(size(64), size(64)),
        ));
        assert!(
            matches!(result, Err(Error::MonitorNotFound)),
            "got {result:?}"
        );
    }
}
