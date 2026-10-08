//! Windows integration test: builds a `MenuBar`, attaches it to a real
//! `HWND`, checks the menu is on the window, updates item state, and detaches
//! on guard drop.
//!
//! Runs on the CI Windows leg as part of `cargo nextest run --workspace`;
//! compiled cross-platform via
//! `cargo check --target x86_64-pc-windows-msvc --all-targets`.
#![cfg(target_os = "windows")]

use waterkit_menu::{
    Command, CommandId, Entry, Key, MenuBar, MenuError, Modifiers, Shortcut, StandardItem, Submenu,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetMenu, GetMenuItemID,
    GetSubMenu, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
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

const OPEN: CommandId = CommandId::new(1);
const TOGGLE: CommandId = CommandId::new(2);
const ABOUT: CommandId = CommandId::new(3);

fn test_bar() -> MenuBar {
    MenuBar::new([
        Submenu::new("File")
            .entry(Command::new(OPEN, "Open"))
            .entry(Entry::Separator)
            .entry(Command::new(TOGGLE, "Toggle").checked(true).enabled(false)),
        Submenu::new("Help").entry(Command::new(ABOUT, "About")),
    ])
    .expect("MenuBar::new failed")
}

#[test]
fn attach_places_the_menu_on_the_window_and_detach_removes_it() {
    let window = TestWindow::new();
    let bar = test_bar();

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
fn the_borrowed_attachment_cannot_outlive_the_bar() {
    // `Attachment<'_>` borrows the bar; this compiles only because the borrow
    // ends with the guard.
    let window = TestWindow::new();
    let bar = test_bar();
    let attachment = bar.attach(window.hwnd).expect("attach failed");
    drop(attachment);
    drop(bar);
}

#[test]
fn set_enabled_and_set_checked_do_not_panic_for_bar_ids() {
    let window = TestWindow::new();
    let bar = test_bar();
    let _attachment = bar.attach(window.hwnd).expect("attach failed");

    bar.set_enabled(OPEN, false);
    bar.set_enabled(OPEN, true);
    bar.set_checked(OPEN, true);
    bar.set_checked(OPEN, false);
}

#[test]
#[should_panic(expected = "not in this bar")]
fn set_enabled_unknown_id_panics() {
    let bar = test_bar();
    bar.set_enabled(CommandId::new(999), false);
}

#[test]
fn standard_items_are_rejected_on_windows() {
    let error = MenuBar::new([Submenu::new("App").entry(StandardItem::ShowAll)])
        .expect_err("StandardItem must fail on Windows");
    assert!(matches!(error, MenuError::StandardItemUnsupported));
}

#[test]
fn duplicate_command_ids_are_rejected() {
    let error = MenuBar::new([Submenu::new("File")
        .entry(Command::new(CommandId::new(1), "One"))
        .entry(Command::new(CommandId::new(1), "Also one"))])
    .expect_err("a repeated CommandId must fail");
    assert!(matches!(error, MenuError::DuplicateCommandId(_)));
}

#[test]
fn bars_with_shortcuts_expose_an_accelerator_table() {
    let bar = MenuBar::new([
        Submenu::new("File").entry(Command::new(OPEN, "Open").shortcut(Shortcut::new(
            Key::Character("o".into()),
            Modifiers::COMMAND,
        ))),
    ])
    .expect("MenuBar::new failed");
    assert!(bar.accelerator_table().is_some());

    let bar = test_bar();
    assert!(bar.accelerator_table().is_none());
}

/// Win32 item ids are allocated per bar: a bar built after another was
/// built and dropped starts its ids at 1 again rather than continuing a
/// process-global count. Exhaustion of the 16-bit id space is covered by
/// the allocator's unit test.
#[test]
fn item_ids_are_per_bar_across_rebuilds() {
    let window = TestWindow::new();
    // A first bar of 3 commands, then dropped: a process-global counter
    // would hand the next bar id 4.
    let file = Submenu::new("File")
        .entry(Command::new(CommandId::new(1), "One"))
        .entry(Command::new(CommandId::new(2), "Two"))
        .entry(Command::new(CommandId::new(3), "Three"));
    drop(MenuBar::new([file]).expect("MenuBar::new failed"));

    // The next bar's first item carries Win32 id 1 again.
    let last = Submenu::new("File").entry(Command::new(CommandId::new(4), "Last"));
    let bar = MenuBar::new([last]).expect("MenuBar::new failed");
    let _attachment = bar.attach(window.hwnd).expect("attach failed");
    let item_id = unsafe { GetMenuItemID(GetSubMenu(GetMenu(window.hwnd), 0), 0) };
    assert_eq!(item_id, 1);
}
