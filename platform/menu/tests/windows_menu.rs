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

/// The bar assigns Win32 item ids from 1 upward per bar, so a process can
/// build far more than 65,535 commands across rebuilds.
#[test]
fn item_ids_are_per_bar_across_rebuilds() {
    let window = TestWindow::new();
    let mut next = 0u64;
    // 7 × 10,000 = 70,000 commands, beyond the 16-bit Win32 id space.
    for _ in 0..7 {
        let mut file = Submenu::new("File");
        for _ in 0..10_000 {
            next += 1;
            file = file.entry(Command::new(CommandId::new(next), "x"));
        }
        let bar = MenuBar::new([file]).expect("MenuBar::new failed");
        drop(bar);
    }

    // The latest bar's first item carries Win32 id 1 again.
    let mut file = Submenu::new("File");
    next += 1;
    let last_id = CommandId::new(next);
    file = file.entry(Command::new(last_id, "Last"));
    let bar = MenuBar::new([file]).expect("MenuBar::new failed");
    let _attachment = bar.attach(window.hwnd).expect("attach failed");
    let item_id = unsafe { GetMenuItemID(GetSubMenu(GetMenu(window.hwnd), 0), 0) };
    assert_eq!(item_id, 1);
}

/// A single bar that runs out of Win32 item ids reports it.
#[test]
fn a_bar_beyond_65535_items_errors() {
    let mut file = Submenu::new("File");
    for id in 1..=0x1_0000 {
        file = file.entry(Command::new(CommandId::new(id), "x"));
    }
    let error = MenuBar::new([file]).expect_err("more than 0xFFFF items must fail");
    assert!(matches!(error, MenuError::ItemLimitExceeded));
}
