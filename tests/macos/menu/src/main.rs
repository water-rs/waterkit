//! macOS structured test for `waterkit-menu`.
//!
//! Builds a `MenuBar` on the main thread, installs it as `NSApp.mainMenu`,
//! inspects the resulting `NSMenu` tree (titles, key equivalents, modifier
//! masks, enabled/checked state, standard items), and activates an item
//! through `-[NSMenu performActionForItemAtIndex:]` to check the `CommandId`
//! arrives on `events()`.

use std::process::ExitCode;

use futures::{FutureExt, StreamExt};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use waterkit_menu::{
    Command, CommandId, Entry, Key, MenuBar, Modifiers, NamedKey, Shortcut, StandardItem, Submenu,
};
use waterkit_test_report::{TestCase, TestReport, write_report_block_to_stdout};

fn main() -> ExitCode {
    let mut report = TestReport::new("macos", "waterkit-menu");
    run(&mut report);
    finish(&report)
}

#[expect(
    clippy::too_many_lines,
    reason = "the harness is one linear sequence of independent checks sharing the report"
)]
fn run(report: &mut TestReport) {
    let Some(mtm) = MainThreadMarker::new() else {
        report.push(TestCase::failed(
            "main-thread",
            "test harness is not running on the main thread",
        ));
        return;
    };

    let open_id = CommandId::new(1);
    let save_id = CommandId::new(2);
    let word_wrap_id = CommandId::new(3);
    let open = Command::new(open_id, "Open").shortcut(Shortcut::new(
        Key::Character("o".into()),
        Modifiers::COMMAND,
    ));
    let save = Command::new(save_id, "Save")
        .shortcut(Shortcut::new(
            Key::Character("s".into()),
            Modifiers::COMMAND | Modifiers::SHIFT,
        ))
        .enabled(false);
    let word_wrap = Command::new(word_wrap_id, "Word Wrap").checked(true);
    let delete_forward = Command::new(CommandId::new(4), "Delete Forward")
        .shortcut(Shortcut::new(NamedKey::Delete, Modifiers::empty()));
    let delete_backward = Command::new(CommandId::new(5), "Delete Backward")
        .shortcut(Shortcut::new(NamedKey::Backspace, Modifiers::empty()));
    let close_window = Command::new(CommandId::new(6), "Close Tab").shortcut(Shortcut::new(
        NamedKey::Tab,
        Modifiers::COMMAND | Modifiers::CONTROL,
    ));

    let bar = match MenuBar::new([
        Submenu::new("Test App")
            .entry(StandardItem::About {
                name: "Test App".to_owned(),
            })
            .entry(Entry::Separator)
            .entry(StandardItem::Services)
            .entry(Entry::Separator)
            .entry(StandardItem::Hide {
                name: "Test App".to_owned(),
            })
            .entry(StandardItem::HideOthers)
            .entry(StandardItem::ShowAll)
            .entry(Entry::Separator)
            .entry(StandardItem::Quit {
                name: "Test App".to_owned(),
            }),
        Submenu::new("File")
            .entry(open)
            .entry(save)
            .entry(Entry::Separator)
            .entry(Submenu::new("Recent").entry(Command::new(CommandId::new(7), "One"))),
        Submenu::new("Edit")
            .entry(word_wrap)
            .entry(delete_forward)
            .entry(delete_backward)
            .entry(close_window),
    ]) {
        Ok(bar) => bar,
        Err(error) => {
            report.push(TestCase::failed(
                "menubar.new",
                format!("MenuBar::new failed: {error}"),
            ));
            return;
        }
    };
    report.push(TestCase::passed("menubar.new"));

    bar.install(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let Some(main_menu) = app.mainMenu() else {
        report.push(TestCase::failed(
            "menubar.install",
            "NSApp.mainMenu is empty after install",
        ));
        return;
    };
    report.push(TestCase::passed("menubar.install"));

    // ---- Bar shape: three pull-downs, each carrying a submenu. ----
    check(
        report,
        "menubar.top-level",
        main_menu.numberOfItems() == 3,
        format!(
            "expected 3 top-level items, found {}",
            main_menu.numberOfItems()
        ),
    );
    let top = menu_items(&main_menu);
    let titles: Vec<String> = top.iter().map(|item| item.title().to_string()).collect();
    check(
        report,
        "menubar.top-level-titles",
        titles == ["Test App", "File", "Edit"],
        format!("titles: {titles:?}"),
    );
    let submenus: Vec<Option<Retained<NSMenu>>> = top.iter().map(|item| item.submenu()).collect();
    check(
        report,
        "menubar.submenus-attached",
        submenus.iter().all(Option::is_some),
        "every top-level item should carry a submenu".to_owned(),
    );

    // ---- Application menu: standard items with the platform actions. ----
    let app_menu = submenus[0].as_ref().expect("checked above");
    let app_items = menu_items(app_menu);
    let app_titles: Vec<String> = app_items
        .iter()
        .map(|item| item.title().to_string())
        .collect();
    check(
        report,
        "app-menu.standard-titles",
        app_titles
            == [
                "About Test App",
                "",
                "Services",
                "",
                "Hide Test App",
                "Hide Others",
                "Show All",
                "",
                "Quit Test App",
            ],
        format!("titles: {app_titles:?}"),
    );
    check(
        report,
        "app-menu.separators",
        [1usize, 3, 7]
            .iter()
            .all(|index| app_items[*index].isSeparatorItem()),
        "expected separator items at 1, 3, 7".to_owned(),
    );
    check(
        report,
        "app-menu.about-action",
        app_items[0].action() == Some(sel!(orderFrontStandardAboutPanel:)),
        format!("About action: {:?}", app_items[0].action()),
    );
    check(
        report,
        "app-menu.standard-targets",
        // The Services item is excluded: AppKit assigns its target when it
        // wires the submenu to `servicesMenu`, and the timing is not
        // deterministic.
        app_items
            .iter()
            .filter(|item| item.title().to_string() != "Services")
            .all(|item| item.target().is_none()),
        format!(
            "standard items keep a nil target so the responder chain handles them; \
             non-nil at {:?}",
            app_items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.target().is_some())
                .map(|(index, item)| (index, item.title().to_string()))
                .collect::<Vec<_>>()
        ),
    );
    check(
        report,
        "app-menu.services",
        app_items[2].hasSubmenu() && app.servicesMenu().is_some(),
        "Services must carry a submenu wired to NSApp.servicesMenu".to_owned(),
    );
    check(
        report,
        "app-menu.standard-equivalents",
        app_items[4].keyEquivalent().to_string() == "h"
            && app_items[5].keyEquivalent().to_string() == "h"
            && app_items[5].keyEquivalentModifierMask()
                == (NSEventModifierFlags::Command | NSEventModifierFlags::Option)
            && app_items[8].keyEquivalent().to_string() == "q",
        format!(
            "hide={:?}@{:?} quit={:?}",
            app_items[4].keyEquivalent().to_string(),
            app_items[5].keyEquivalentModifierMask(),
            app_items[8].keyEquivalent().to_string(),
        ),
    );

    // ---- File menu: commands with shortcuts and states. ----
    let file_menu = submenus[1].as_ref().expect("checked above");
    let file_items = menu_items(file_menu);
    check(
        report,
        "file-menu.shortcuts",
        file_items[0].keyEquivalent().to_string() == "o"
            && file_items[0].keyEquivalentModifierMask() == NSEventModifierFlags::Command
            && file_items[1].keyEquivalent().to_string() == "s"
            && file_items[1].keyEquivalentModifierMask()
                == (NSEventModifierFlags::Command | NSEventModifierFlags::Shift),
        format!(
            "open={:?}@{:?} save={:?}@{:?}",
            file_items[0].keyEquivalent().to_string(),
            file_items[0].keyEquivalentModifierMask(),
            file_items[1].keyEquivalent().to_string(),
            file_items[1].keyEquivalentModifierMask(),
        ),
    );
    check(
        report,
        "file-menu.separator",
        file_items[2].isSeparatorItem(),
        "expected a separator after Save".to_owned(),
    );
    check(
        report,
        "file-menu.nested-submenu",
        file_items[3].hasSubmenu(),
        "expected a nested submenu".to_owned(),
    );
    check(
        report,
        "file-menu.command-targets",
        [0usize, 1]
            .iter()
            .all(|index| file_items[*index].target().is_some()),
        "command items must have a target to report their CommandId".to_owned(),
    );

    // ---- Edit menu: named-key equivalents and checked state. ----
    let edit_menu = submenus[2].as_ref().expect("checked above");
    let edit_items = menu_items(edit_menu);
    let equivalents: Vec<String> = edit_items
        .iter()
        .skip(1)
        .map(|item| item.keyEquivalent().to_string())
        .collect();
    check(
        report,
        "edit-menu.named-key-equivalents",
        equivalents == ["\u{F728}", "\u{7F}", "\u{9}"],
        format!(
            "Delete=NSDeleteFunctionKey Backspace=NSDeleteCharacter Tab=\\t; found {equivalents:?}"
        ),
    );
    check(
        report,
        "edit-menu.initial-state",
        edit_items[0].state() == NSControlStateValueOn && !file_items[1].isEnabled(),
        format!(
            "word-wrap state={:?} save enabled={:?}",
            edit_items[0].state(),
            file_items[1].isEnabled(),
        ),
    );

    // ---- set_enabled / set_checked ----
    bar.set_enabled(save_id, true);
    bar.set_checked(word_wrap_id, false);
    check(
        report,
        "menubar.set_enabled",
        file_items[1].isEnabled(),
        "Save should be enabled after set_enabled".to_owned(),
    );
    check(
        report,
        "menubar.set_checked",
        edit_items[0].state() == NSControlStateValueOff,
        format!(
            "word-wrap state after set_checked(false): {:?}",
            edit_items[0].state()
        ),
    );

    // ---- Activation: performActionForItemAtIndex delivers CommandId. ----
    let mut events = std::pin::pin!(bar.events());
    file_menu.performActionForItemAtIndex(0);
    match events.next().now_or_never().flatten() {
        Some(id) => check(
            report,
            "activation.command-id",
            id == open_id,
            format!("expected {open_id:?}, got {id:?}"),
        ),
        None => report.push(TestCase::failed(
            "activation.command-id",
            "no CommandId arrived on events()",
        )),
    }

    // A disabled item is skipped by performActionForItemAtIndex: no id.
    bar.set_enabled(save_id, false);
    file_menu.performActionForItemAtIndex(1);
    check(
        report,
        "activation.disabled-item",
        events.next().now_or_never().is_none(),
        "disabled item still delivered a CommandId".to_owned(),
    );

    // Re-enabled, the same item reports its id.
    bar.set_enabled(save_id, true);
    file_menu.performActionForItemAtIndex(1);
    match events.next().now_or_never().flatten() {
        Some(id) => check(
            report,
            "activation.re-enabled-item",
            id == save_id,
            format!("expected {save_id:?}, got {id:?}"),
        ),
        None => report.push(TestCase::failed(
            "activation.re-enabled-item",
            "no CommandId arrived on events()",
        )),
    }

    // Nested submenu activation reports the nested command's id.
    let recent = file_items[3].submenu().expect("nested submenu missing");
    recent.performActionForItemAtIndex(0);
    let nested = events.next().now_or_never().flatten();
    check(
        report,
        "activation.nested-submenu",
        nested.is_some() && nested != Some(open_id),
        format!("nested event: {nested:?}"),
    );
}

/// Every item of `menu`, in order.
fn menu_items(menu: &NSMenu) -> Vec<Retained<NSMenuItem>> {
    (0..menu.numberOfItems())
        .map(|index| menu.itemAtIndex(index).expect("index within numberOfItems"))
        .collect()
}

fn check(report: &mut TestReport, name: &'static str, ok: bool, failure: String) {
    if ok {
        report.push(TestCase::passed(name));
    } else {
        report.push(TestCase::failed(name, failure));
    }
}

fn finish(report: &TestReport) -> ExitCode {
    write_report_block_to_stdout(report).expect("failed to write structured test report");

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
