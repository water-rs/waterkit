# waterkit-screen

Cross-platform screen capture and brightness control for Rust.

Part of the [WaterKit](https://github.com/water-rs/kit) ecosystem.

## Features

- Enumerate monitors and display properties.
- Capture screenshots (PNG encoded).
- Get and set screen brightness levels.
- macOS 14+ System Picker support for privacy-first screen capture.

## Platform Support

| Feature | Windows | macOS | Linux | iOS | Android |
| :--- | :---: | :---: | :---: | :---: | :---: |
| Screen Listing | ✅ | ✅ | ✅ | ⚠️ | ⚠️ |
| Screen Capture | ✅ | ✅ | ✅ | ✅ | ❌ |
| Brightness | ✅* | ⚠️* | ✅* | ✅ | ✅ |
| System Picker | ❌ | ✅ | ❌ | ❌ | ❌ |

*\* Desktop brightness control is currently experimental or stubbed on some platforms.*

## Usage

### Listing Screens

```rust
use waterkit_screen::screens;

let screen_list = screens().expect("Failed to list screens");
for screen in screen_list {
    println!("{}: {}x{}", screen.name, screen.width, screen.height);
}
```

### Capturing a Screenshot

```rust
use waterkit_screen::capture_screen;

let png_bytes = capture_screen(0).expect("Failed to capture");
std::fs::write("screenshot.png", png_bytes).unwrap();
```

### Brightness Control

```rust
use waterkit_screen::{get_brightness, set_brightness};

let current = get_brightness().await?;
set_brightness(0.5).await?;
```

### macOS System Picker (Privacy First)

On macOS 14.0+, you can use the system content picker which doesn't require the "Screen Recording" permission:

```rust
let png_bytes = waterkit_screen::pick_and_capture().await?;
```

## Android Initialization

On Android, initialize with a `Context` first:

```rust
#[no_mangle]
pub extern "C" fn Java_com_example_MainActivity_initScreen(
    mut env: jni::JNIEnv,
    _: jni::objects::JClass,
    context: jni::objects::JObject
) {
    waterkit_screen::init(&mut env, &context).unwrap();
}
```

## License

MIT
