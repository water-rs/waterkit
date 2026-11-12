# waterkit-automation

Virtual displays and cursor-free input, for driving a desktop that is not the
user's. macOS only; on every other platform the crate is empty.

- `VirtualDisplay` creates a private-API `CGVirtualDisplay` of a chosen size
  in points, backing scale and refresh rate. It exists on none of the user's
  monitors and is removed when dropped. Creation goes through a
  `DisplayHost`, made on the main thread, and services the main run loop
  until the display is online, active and outside any mirror set, bringing a
  display `WindowServer` left offline into the topology with
  `SLWindowMirroringManager` when it has to. The user's main display stays
  main; on a Mac with no monitor the host first replaces the system's
  headless fallback display with a stand-in of the same mode, which keeps the
  user's desktop and is removed with the host's last display.
- `Input` posts typed events (`MouseDown`, `MouseUp`, `MouseMove`, `Scroll`,
  `KeyDown`, `KeyUp`, `Text`) to the application that owns the window under a
  point on one display, never through the HID event tap: the user's cursor
  does not move and `WindowServer`'s front process stays the user's. Before a
  press or a key event the target app is made to report itself active with
  the target window focused, without raising it; key events follow the last
  press. `post` returns once the target process has every event, and fails
  when the app does not take focus or an event does not arrive within
  `Input::confirm_timeout`.
- `LaunchedApplication::launch` starts the app a `LaunchConfig` names, by
  bundle id or bundle path and optionally with URLs and files to open,
  through `NSWorkspace` without activating it, and returns its pid, bundle id
  and windows once it shows one. With `LaunchConfig::display` set, the app's
  windows are moved onto that display through Accessibility first. A launch
  that fails, wedges or shows no window within `LaunchConfig::timeout` is an
  error.

Posting to another process needs the Accessibility grant: request
`Permission::Accessibility` through `waterkit-permission` first.

The private `SkyLight` and `HIServices` functions are resolved by name at
runtime, so nothing links a private framework or symbol; a missing or changed
private API is an error at the call that needs it.

```rust,ignore
use std::num::NonZeroU32;
use waterkit_automation::{
    DisplayHost, Input, MainThreadMarker, MouseButton, MouseDown, MouseUp, Point, Text,
    VirtualDisplay, VirtualDisplayConfig,
};

let mut host = DisplayHost::new(MainThreadMarker::new().expect("on the main thread"));
let config = VirtualDisplayConfig::new(
    "Agent desktop",
    NonZeroU32::new(1280).unwrap(),
    NonZeroU32::new(800).unwrap(),
    0x5EED,
);
let display = VirtualDisplay::new(&config, &mut host)?;
let mut input = Input::new(display.id())?;
let at = Point::new(200.0, 120.0);
input.post(MouseDown::new(at, MouseButton::Left))?;
input.post(MouseUp::new(at, MouseButton::Left))?;
input.post(Text::new("hello"))?;
```
