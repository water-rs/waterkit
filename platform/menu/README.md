# waterkit-menu

Native application menu bar for macOS and Windows.

`MenuBar` projects a platform-neutral menu description (`Submenu`s of
`Entry` items) onto the platform's own menu-bar object:

- **macOS** — an `NSMenu` installed as `NSApp.mainMenu` (through `objc2` /
  `objc2-app-kit`). `MenuBar::install` replaces the current bar; the
  `StandardItem`s give the application menu its About, Services, Hide, Show
  All and Quit items with the platform's standard actions and equivalents.
- **Windows** — a Win32 `HMENU` attached to an `HWND` (through the `windows`
  crate). `MenuBar::attach` subclasses the window with `SetWindowSubclass` so
  `WM_COMMAND` activations reach the bar's event stream; the returned
  `Attachment` removes the subclass and detaches the menu on drop.
- **Other platforms** — no application menu-bar object exists, so
  `MenuBar::new` fails with `MenuError::Unsupported`.

## Shortcuts

A `Shortcut` is a W3C `KeyboardEvent.key` (`keyboard_types::Key`) plus
`Modifiers`. `Modifiers::COMMAND` is the platform's menu accelerator modifier:
⌘ on macOS, Ctrl on Windows. Keys map to the platform's real menu key
equivalent — on macOS `Delete` is `NSDeleteFunctionKey` (U+F728) and
`Backspace` is `NSDeleteCharacter` (U+007F); on Windows a key maps to its
virtual-key code and accelerator text. A key with no mapping fails
`MenuBar::new` with `MenuError::UnmappableKey` — never silently dropped.

On Windows the shortcut shows as accelerator text next to the item title.
Message pumps such as winit's never call `TranslateAcceleratorW`, so chords do
not fire through the menu: the host dispatches the chord itself.

## Activation

Choosing an item yields its `CommandId` on `MenuBar::events` — a `Send`
`futures::Stream`. Activation never goes through a process-global channel:
each bar owns its stream.

`MenuBar::set_enabled` / `set_checked` update items after construction and
panic on a `CommandId` that is not in the bar.

## Threading

All native objects stay on the main thread; `MenuBar` is `!Send`. Build and
install it from the main thread and keep it alive for as long as it is
installed.

## Example

```sh
cargo run -p waterkit-menu --example menu
```
