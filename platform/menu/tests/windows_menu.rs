//! Windows integration test: builds a `MenuBar`, attaches it to a real
//! `HWND`, checks the menu is on the window, updates item state, and detaches
//! on guard drop.
//!
//! Runs on the CI Windows leg as part of `cargo nextest run --workspace`;
//! compiled cross-platform via
//! `cargo check --target x86_64-pc-windows-msvc --all-targets`.
#![cfg(target_os = "windows")]

use waterkit_menu::{Command, CommandId, Entry, MenuBar, MenuError, StandardItem, Submenu};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetMenu, RegisterClassW,
    WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `DefWindowProcW` cannot be used as a `WNDPROC` directly (it is a
/// Rust-ABI wrapper); this forwards to it with the right convention.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: forwards every message to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A test window using `DefWindowProc` so `WM_COMMAND` reaches the subclass.
struct TestWindow {
    hwnd: HWND,
}

impl TestWindow {
    fn new() -> Self {
        let class_name = to_wide("WaterkitMenuTestWindow");
        // SAFETY: registers a private class (ignoring an existing
        // registration from another test) and creates an overlapped window.
        unsafe {
            let instance = GetModuleHandleW(None).expect("GetModuleHandleW failed");
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&raw const class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_OVERLAPPED,
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
            Self { hwnd }
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: destroys the window created in `new`. The class registration
        // is shared and left in place for the process.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn test_bar() -> (MenuBar, CommandId) {
    let open = Command::new("Open");
    let open_id = open.id();
    let bar = MenuBar::new([
        Submenu::new("File")
            .entry(open)
            .entry(Entry::Separator)
            .entry(Command::new("Toggle").checked(true).enabled(false)),
        Submenu::new("Help").entry(Command::new("About")),
    ])
    .expect("MenuBar::new failed");
    (bar, open_id)
}

#[test]
fn attach_places_the_menu_on_the_window_and_detach_removes_it() {
    let window = TestWindow::new();
    let (bar, _open_id) = test_bar();

    let attachment = bar.attach(window.hwnd).expect("attach failed");
    unsafe {
        assert!(!GetMenu(window.hwnd).is_invalid());
    }

    drop(attachment);
    unsafe {
        assert!(GetMenu(window.hwnd).is_invalid());
    }
}

#[test]
fn set_enabled_and_set_checked_do_not_panic_for_bar_ids() {
    let window = TestWindow::new();
    let (bar, open_id) = test_bar();
    let _attachment = bar.attach(window.hwnd).expect("attach failed");

    bar.set_enabled(open_id, false);
    bar.set_enabled(open_id, true);
    bar.set_checked(open_id, true);
    bar.set_checked(open_id, false);
}

#[test]
#[should_panic(expected = "not in this bar")]
fn set_enabled_unknown_id_panics() {
    let (bar, _open_id) = test_bar();
    bar.set_enabled(Command::new("elsewhere").id(), false);
}

#[test]
fn standard_items_are_rejected_on_windows() {
    let error = MenuBar::new([Submenu::new("App").entry(StandardItem::ShowAll)])
        .expect_err("StandardItem must fail on Windows");
    assert!(matches!(error, MenuError::StandardItemUnsupported));
}
