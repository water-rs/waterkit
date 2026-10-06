# iOS Test Framework

iOS test harness for waterkit crates. It runs on the iOS simulator and on a
physical iPhone or iPad.

## Structure

```
tests/ios/
├── app/                        # SwiftUI app around the Rust library
│   ├── WaterKitTest.xcodeproj  # builds and signs the app
│   ├── Info.plist
│   └── WaterKitTest/
│       ├── WaterKitTestApp.swift
│       ├── ContentView.swift
│       └── Generated/          # swift-bridge glue, written by rust/build.rs
└── rust/                       # the test cases, as a static library
    ├── Cargo.toml
    ├── build.rs
    └── src/
```

## Usage

Run through the `waterkit-test` CLI from the workspace root. The command builds
the Rust library for the selected crate's feature, builds the app around it with
`xcodebuild`, installs and launches it, reads the structured JSON report the app
writes into its data container, prints every case, and fails if any case
failed.

### Simulator

Boot an iOS simulator, then:

```bash
cargo run -p waterkit-test -- ios device/sensor
```

The simulator has no camera, and its GPU and Metal behaviour differ from a
device's. Anything that needs real hardware runs on a device.

### Physical device

```bash
cargo run -p waterkit-test -- ios device/camera \
    --device <UDID or name> --team <TEAM ID>
```

- `--device` is the device as `xcrun devicectl list devices` shows it. It must
  be paired, connected and unlocked.
- `--team` is the development team that signs the app. `xcodebuild` runs with
  automatic provisioning (`-allowProvisioningUpdates`): it registers the
  `com.waterkit.test` App ID and creates or renews the development profile.
  For this to work, Xcode on the building Mac must be signed in to an Apple ID
  in that team (Xcode > Settings > Accounts), and the device must be registered
  to the team. The team ID is the `OU` of an "Apple Development" certificate,
  or Xcode's Signing & Capabilities team list.

Run the command on the Mac the device is attached to. When the device is
attached to another Mac, add `--ssh <destination>`. The app is then built and
signed on this Mac, copied to the other Mac with `scp`, and installed, launched
and read back through `devicectl` over `ssh`. The other Mac needs only Xcode
and the pairing, not a Rust toolchain or a checkout:

```bash
cargo run -p waterkit-test -- ios device/camera \
    --device 00008140-001845681E98801C --team 4AZ53N9R83 \
    --ssh lexoliu@lexos-mac-mini
```

On a device the runner:

1. installs the app with `xcrun devicectl device install app`;
2. launches it with `xcrun devicectl device process launch --console`, which
   shows the app's output and returns when the app exits;
3. copies `Documents/waterkit-test-reports/<run-id>.json` out of the app's
   data container with `xcrun devicectl device copy from`.

The report comes back as a file rather than as console output. The app writes
it atomically, under a run ID the runner chose for that launch, so a report
that comes back is complete and belongs to that run. A console stream can be
cut short by a dropped connection or an unflushed buffer, and nothing in it
identifies the launch that produced it.

### Permissions on a device

`simctl privacy` exists only for the simulator. On a device, iOS grants a
privacy permission only when someone answers the system prompt on the device.
The harness does not try to get around this. Each case that needs a permission
requests it and reports what it got:

- The first camera run on a device shows "WaterKitTest Would Like to Access the
  Camera" on the device. Tap **Allow** while it is showing. The camera case
  waits up to 60 s for the answer. If nobody answers, `camera.permission` fails
  with "camera access is not determined"; the prompt closes when the app exits,
  so run the harness again and tap **Allow** during that run. Opening
  WaterKitTest on the device and tapping "Run All Tests" shows the same prompt.
- If access was denied, `camera.permission` fails and names the switch to turn
  on: Settings > Privacy & Security > Camera > WaterKitTest.

The answer persists until the app is deleted, so it is needed once per device.
Reinstalling the app over an existing installation, which every run does, keeps
it.

The device must stay unlocked while the app is launched. The app keeps the
screen awake during a run.

## Cases

The camera case lists the cameras, asks for camera access, and streams five
frames from every camera (`camera.frames.<camera id>`), dropping each before
taking the next. It reports each camera's name, whether it faces the user, the
plane layout, encoding and stored size of its frames, their orientations, and
the upright size `FrameConverter` turns the last frame into.

## Manual runs

`WaterKitTest.xcodeproj` can be opened in Xcode. The project links the Rust
library named by the `WATERKIT_RUST_LIBRARY` build setting, which the runner
passes to `xcodebuild`; a build from Xcode needs it set to the library that
`cargo build -p waterkit-test-ios --target <target> --features <feature>`
produced. The app has a "Run All Tests" button for local exploration.

## Requirements

- Xcode 16 or later.
- Rust with the `aarch64-apple-ios-sim` target, and `aarch64-apple-ios` for
  devices.
