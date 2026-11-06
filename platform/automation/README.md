# waterkit-automation

Virtual displays and cursor-free input, for driving a desktop that is not the
user's. macOS only; on every other platform the crate is empty.

- `VirtualDisplay` creates a private-API `CGVirtualDisplay` of a chosen size
  in points, backing scale and refresh rate. It exists on none of the user's
  monitors and is removed when dropped. Creation takes a `MainThreadMarker`
  and services the main run loop until the display is online, active and
  outside any mirror set, bringing a display `WindowServer` left offline into
  the topology with `SLWindowMirroringManager` when it has to.
- `Input` posts typed events (`MouseDown`, `MouseUp`, `MouseMove`, `Scroll`,
  `KeyDown`, `KeyUp`, `Text`) to the application that owns the window under a
  point on one display, never through the HID event tap: the user's cursor
  does not move and the user's front app keeps its focus. A press makes its
  window the key window of its app without raising it; key events follow the
  last press.

Posting to another process needs the Accessibility grant: request
`Permission::Accessibility` through `waterkit-permission` first.

The private `SkyLight` framework is opened at runtime and its functions
resolved by name, so nothing links a private framework; a missing or changed
private API is an error at the call that needs it.

```rust,ignore
use std::num::NonZeroU32;
use waterkit_automation::{
    Input, MainThreadMarker, MouseButton, MouseDown, MouseUp, Point, Text, VirtualDisplay,
    VirtualDisplayConfig,
};

let mtm = MainThreadMarker::new().expect("on the main thread");
let config = VirtualDisplayConfig::new(
    "Agent desktop",
    NonZeroU32::new(1280).unwrap(),
    NonZeroU32::new(800).unwrap(),
    0x5EED,
);
let display = VirtualDisplay::new(&config, mtm)?;
let mut input = Input::new(display.id())?;
let at = Point::new(200.0, 120.0);
input.post(MouseDown::new(at, MouseButton::Left))?;
input.post(MouseUp::new(at, MouseButton::Left))?;
input.post(Text::new("hello"))?;
```
