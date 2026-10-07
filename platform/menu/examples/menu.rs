//! Builds a `MenuBar`, installs it, and prints every activation.
//!
//! On macOS the bar is installed as `NSApp.mainMenu`. On Windows a small
//! window is created and the bar attached to it. Run it, open the menus, and
//! pick items — their `CommandId`s print on stdout.

use futures::StreamExt;
use waterkit_menu::{
    Command, Entry, Key, MenuBar, Modifiers, NamedKey, Shortcut, StandardItem, Submenu,
};

fn key(c: char) -> Key {
    Key::Character(c.to_string())
}

fn main() {
    let quit = Command::new("Quit");
    let quit_id = quit.id();

    let bar = MenuBar::new([
        Submenu::new("Example")
            .entry(StandardItem::About {
                name: "Example".to_owned(),
            })
            .entry(StandardItem::Quit {
                name: "Example".to_owned(),
            }),
        Submenu::new("File")
            .entry(Command::new("Open").shortcut(Shortcut::new(key('o'), Modifiers::COMMAND)))
            .entry(Command::new("Save").shortcut(Shortcut::new(key('s'), Modifiers::COMMAND)))
            .entry(Entry::Separator)
            .entry(Submenu::new("Export").entry(Command::new("As PNG"))),
        Submenu::new("Edit")
            .entry(
                Command::new("Delete")
                    .shortcut(Shortcut::new(NamedKey::Delete, Modifiers::empty())),
            )
            .entry(Command::new("Checkable").checked(true))
            .entry(Command::new("Disabled").enabled(false))
            .entry(quit),
    ])
    .expect("failed to build the menu bar");

    // The attachment must outlive the window; it is kept for the rest of
    // `main`.
    #[cfg(target_os = "windows")]
    let _attachment = install(&bar);
    #[cfg(not(target_os = "windows"))]
    install(&bar);
    println!("menu bar installed — activate items to see their CommandId");

    // Poll activations until the Quit command fires. `events()` is `Send`, so
    // a real app can drive this loop on any thread.
    let mut events = std::pin::pin!(bar.events());
    while let Some(id) = futures::executor::block_on(events.next()) {
        println!("activated {id}");
        if id == quit_id {
            break;
        }
    }
}

#[cfg(target_os = "macos")]
fn install(bar: &MenuBar) {
    let mtm = objc2::MainThreadMarker::new().expect("must run on the main thread");
    bar.install(mtm);
}

#[cfg(target_os = "windows")]
fn install(bar: &MenuBar) -> waterkit_menu::Attachment {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
        WS_OVERLAPPEDWINDOW,
    };
    use windows::core::PCWSTR;

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: forwards every message to the default procedure.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    let to_wide = |text: &str| {
        text.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let class = to_wide("WaterkitMenuExample");
    // SAFETY: registers a private class and creates an overlapped window on
    // this thread; the window lives for the rest of the process.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&raw const wnd_class);
        CreateWindowExW(
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
        .expect("CreateWindowExW failed")
    };

    bar.attach(hwnd).expect("failed to attach the menu bar")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install(_bar: &MenuBar) {
    eprintln!("this platform has no application menu bar");
}
