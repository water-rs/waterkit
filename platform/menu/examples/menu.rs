//! Builds a `MenuBar`, installs it, and logs every activation.
//!
//! On macOS the bar is installed as `NSApp.mainMenu` and `NSApp.run()` pumps
//! the events. On Windows a small window is shown, the bar attached to it,
//! and a `GetMessageW` loop runs with `TranslateAcceleratorW` so chords work
//! too. Open the menus and pick items — their `CommandId`s are logged.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use futures::StreamExt;
#[cfg(target_os = "macos")]
use waterkit_menu::StandardItem;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use waterkit_menu::{Command, Entry, Key, MenuBar, Modifiers, NamedKey, Shortcut, Submenu};

/// Command ids are caller-supplied; the example numbers them by hand.
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod ids {
    use waterkit_menu::CommandId;

    pub const OPEN: CommandId = CommandId::new(1);
    pub const SAVE: CommandId = CommandId::new(2);
    pub const EXPORT_PNG: CommandId = CommandId::new(3);
    pub const DELETE: CommandId = CommandId::new(4);
    pub const CHECKABLE: CommandId = CommandId::new(5);
    pub const DISABLED: CommandId = CommandId::new(6);
    pub const QUIT: CommandId = CommandId::new(7);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn key(c: char) -> Key {
    Key::Character(c.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn main() {
    tracing_subscriber::fmt().init();

    let file = Submenu::new("File")
        .entry(
            Command::new(ids::OPEN, "Open").shortcut(Shortcut::new(key('o'), Modifiers::COMMAND)),
        )
        .entry(
            Command::new(ids::SAVE, "Save").shortcut(Shortcut::new(key('s'), Modifiers::COMMAND)),
        )
        .entry(Entry::Separator)
        .entry(Submenu::new("Export").entry(Command::new(ids::EXPORT_PNG, "As PNG")));
    let edit = Submenu::new("Edit")
        .entry(
            Command::new(ids::DELETE, "Delete")
                .shortcut(Shortcut::new(NamedKey::Delete, Modifiers::empty())),
        )
        .entry(Command::new(ids::CHECKABLE, "Checkable").checked(true))
        .entry(Command::new(ids::DISABLED, "Disabled").enabled(false))
        .entry(Command::new(ids::QUIT, "Quit"));

    let menus = [
        // The application menu's standard items exist only on macOS.
        #[cfg(target_os = "macos")]
        Submenu::new("Example")
            .entry(StandardItem::About {
                name: "Example".to_owned(),
            })
            .entry(StandardItem::Quit {
                name: "Example".to_owned(),
            }),
        file,
        edit,
    ];
    let bar = MenuBar::new(menus).expect("failed to build the menu bar");

    // `events()` is `Send`, so a worker thread owns the log loop while the
    // main thread runs the platform event loop.
    let events = bar.events();
    std::thread::spawn(move || {
        let mut events = std::pin::pin!(events);
        while let Some(id) = futures::executor::block_on(events.next()) {
            tracing::info!(%id, "menu command activated");
            if id == ids::QUIT {
                tracing::info!("quit selected — exiting");
                std::process::exit(0);
            }
        }
    });

    run(&bar);
}

/// Runs the platform event loop. Never returns.
#[cfg(target_os = "macos")]
fn run(bar: &MenuBar) -> ! {
    let mtm = objc2::MainThreadMarker::new().expect("must run on the main thread");
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    bar.install(mtm);
    tracing::info!("menu bar installed as NSApp.mainMenu");
    app.run();
    std::process::exit(0)
}

/// Shows a window and pumps its messages. Never returns.
#[cfg(target_os = "windows")]
fn run(bar: &MenuBar) -> ! {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
        PostQuitMessage, RegisterClassW, SW_SHOW, ShowWindow, TranslateAcceleratorW,
        WINDOW_EX_STYLE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::PCWSTR;

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_DESTROY {
            // SAFETY: posts a quit on this thread's queue.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        // SAFETY: forwards every other message to the default procedure.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    let to_wide = |text: &str| {
        text.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let class = to_wide("WaterkitMenuExample");
    // SAFETY: registers a private class and creates an overlapped window on
    // this thread, then shows it.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&raw const wnd_class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("CreateWindowExW failed");
        let _ = ShowWindow(hwnd, SW_SHOW);
        hwnd
    };

    // The attachment borrows the bar and stays alive for the loop.
    let _attachment = bar.attach(hwnd).expect("failed to attach the menu bar");
    let accel = bar.accelerator_table();
    tracing::info!("menu bar attached; Ctrl+O works through TranslateAcceleratorW");

    // SAFETY: the pump runs on the window's thread; `accel` is the bar's
    // table, so translated chords report through the attachment.
    unsafe {
        let mut msg = MSG::default();
        loop {
            let result = GetMessageW(&raw mut msg, None, 0, 0);
            assert!(result.0 >= 0, "GetMessageW failed");
            if !result.as_bool() {
                break; // WM_QUIT
            }
            if let Some(accel) = accel
                && TranslateAcceleratorW(hwnd, accel, &raw const msg) != 0
            {
                continue;
            }
            let _ = DispatchMessageW(&raw const msg);
        }
    }
    std::process::exit(0)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn main() {
    tracing_subscriber::fmt().init();
    tracing::error!("waterkit-menu runs on macOS and Windows only");
}
