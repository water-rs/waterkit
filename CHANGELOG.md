# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-v0.1.0) - 2026-02-07

### Added

- *(codec)* GPU-first zero-copy video codec with streaming API
- *(screen)* GPU-first zero-copy screen capture with wgpu texture output
- *(clipboard)* complete API redesign with streaming and custom data types
- *(notification)* add quick reply and updatable notification handle
- *(camera)* use ndk-context for standard access to JVM
- *(notification)* Add full notification API with actions, icons, and sounds
- introduce AI guidance document and refactor audio player shutdown mechanism
- Refactor error enums to use `thiserror` and privatize `sys` modules.
- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- Add native photo picker functionality and introduce `DialogError` for improved error handling.
- *(audio)* add read_blocking() for synchronous audio reading
- Introduce async audio streaming via `async-channel` and `futures::Stream` with a dedicated test.
- Implement full iOS app build, install, and launch process in `waterkit-test` tool, and integrate `swift-bridge` for camera and notification modules.
- Enable and test Android camera functionality with improved JNI handling and codec runtime verification.
- *(mobile)* fix audio builds on android, add ios harness, update gitignore
- Implement real iOS permission request functions
- Complete Android mobile test coverage for 10 crates
- Extend mobile test harness to all crates
- Mobile test framework for sensor, biometric, location
- add iOS test harness and fix Android runtime issues (DEX loading, threading)
- add waterkit-build crate for shared build utilities
- Introduce top-level `waterkit` crate with modular features and refactor `codec` module's Apple platform implementation.
- Implement zero-copy video decoding and rendering on macOS using IOSurface and Metal interop
- Implement macOS zero-copy video rendering with IOSurface, add raw frame capture toggle, and introduce GitHub Actions CI/CD.
- Enhance screen capture performance with ScreenCaptureKit and optimized ScreenCapturer; add profiling test for benchmarking
- Add optimized screen recording test with async capture; implement H.265 encoding and performance monitoring
- Implement video codec support with AV1 encoding/decoding; add platform-specific implementations for Apple, Android, and Windows
- Add raw screen capture functionality for improved performance; implement capture_screen_raw method and update platform support
- Add photo capture and video recording functionalities across platforms; implement JPEG format support and enhance error handling
- Implement system information retrieval for macOS, iOS, Android, and desktop platforms; add connectivity, thermal state, and system load functionalities
- Add documentation and implement screen picker functionality for macOS; enhance platform support for screen capture and brightness control
- Add cross-platform screen capture and brightness control functionality
- Implement HDR support across platforms with default settings and error handling
- Add macOS tests for camera, audio, and sensor functionalities; update dependencies and implement sensor reading logic
- Enhance Swift bridge generation and compilation for Apple platforms; update clipboard handling in Swift
- Update workspace members and add Android/macOS test harness for WaterKit integration
- Implement cross-platform secure storage with platform-specific APIs for iOS, Android, Windows, and Linux
- Add cross-platform file system utilities with support for iOS, Android, and desktop
- Add cross-platform biometric authentication support for iOS, Android, and Windows
- Implement cross-platform clipboard access with support for text and image retrieval
- Add macOS alert testing with confirmation dialog and alert types
- Add cross-platform alert system with native implementations for iOS, Android, and desktop
- Implement cross-platform notification system with support for Android, iOS, and desktop platforms
- Add comprehensive README documentation and enhance media command handling
- Add cross-platform haptic feedback support with platform-specific implementations
- Refactor audio player structure and enhance command handling with default behavior
- Enhance audio playback capabilities with rodio integration and media center support
- Implement AudioPlayer component with platform-specific backends and command handling.
- Add Android test harness and macOS test binaries for waterkit crates.
- Introduce `media` crate for cross-platform media session control with Android, Apple, Linux, and Windows implementations and associated tests.
- Add cross-platform media crate and dedicated Android and macOS test harnesses.
- Add media crate with cross-platform media session control and introduce platform-specific tests for location and media.**
- Add cross-platform permission crate and refactor location with platform-specific modules and FFI implementations.

### Fixed

- *(codec)* Fix remaining 2 clippy lints (raw pointer, const fn)
- *(codec)* Fix all Windows clippy lints
- *(location)* Move const to module scope to fix items_after_statements
- Fix location clippy lints and limit feature-powerset depth
- *(haptic)* Collapse nested if-let for clippy
- *(audio)* Allow needless_pass_by_value on error helper fns
- *(codec)* Fix GetOutputStreamInfo API for windows-rs 0.62
- *(audio)* Suppress clippy pedantic lints on legacy MediaSessionInner
- *(haptic)* Fix VibrationDevice API for windows-rs 0.62
- *(audio)* Fix remaining to_string_lossy in button handler
- Fix HSTRING API and remove unused import for windows-rs 0.62
- Fix Windows clippy lints and codec API for windows-rs 0.62
- *(audio)* Clone MediaCommand before pushing to pending queue
- *(audio)* Use Ref::as_ref() for TypedEventHandler args in windows-rs 0.62
- *(location)* Fix Windows Accuracy() return type for windows-rs 0.62
- *(audio)* Implement both MediaSessionInner and MediaCenterInner for Windows
- *(audio)* Fix Windows audio module for windows-rs 0.62
- Fix Windows clippy lints and location API for windows-rs 0.62
- *(sensor)* Add ambient light sensor and fix type mismatches for Windows
- Fix Android clippy lint and exclude platform-specific crates from coverage
- Fix CI failures for cross-platform build
- *(permission)* Fix clippy lints for Windows module
- *(camera)* Fix remaining clippy lints for Linux camera
- *(camera)* Add SendableCamera wrapper for Linux V4L2 thread safety
- *(camera)* Fix RequestedFormat generic parameter syntax
- *(codec)* Add backticks around FFmpeg in doc comments
- *(codec)* Add clippy allow attributes and fix doc comments for Linux
- *(codec)* Fix Linux FFmpeg context consumption errors
- *(audio)* Fix remaining Linux clippy lints
- *(audio)* Add clippy allow attributes for Linux MPRIS module
- *(audio)* Use map_or pattern for Result handling in Linux MPRIS
- *(audio)* Add MediaCenterInner struct for Linux MPRIS
- *(audio)* Rename MediaSessionInner to MediaCenterInner for consistency
- *(location)* Refactor to use async function instead of closure
- *(location)* Use TryFrom for f64 conversion from OwnedValue
- *(location)* Fix Linux implementation for zbus 5.x API
- *(biometric)* Fix clippy lints in Linux implementation
- Address Linux compilation and clippy issues
- *(notification)* Address clippy lints for Linux
- Address remaining clippy lints for Linux targets
- *(sensor)* Address clippy lints for Linux module
- Update zbus and sysinfo API usage for Linux
- Update API usage for sysinfo 0.37 and zbus
- *(sensor)* Update zbus API usage for newer version
- Address Linux platform compile errors
- Address clippy lints and add Windows pkg-config
- Address clippy lints (GeoClue backticks, map_unwrap_or)
- *(permission)* Change pub(crate) to pub for re-exported functions
- *(screen)* Make waterkit-build a universal build dependency
- Move HashSet import inside function to avoid unused warning on non-Apple
- add HIGH_SAMPLING_RATE_SENSORS permission and use SENSOR_DELAY_GAME for Android tests
- use main looper for sensor events in android tests to support background execution
- add clippy allows and missing metadata for waterkit-build
- *(ci)* use relative path for android test dependency
- *(ci)* fix typos and doctest issues
- Correct HEVC codec config extraction and add NV12 to BGRA conversion for Apple video decoding.
- Android JNI type conversion and array handling

### Other

- Remove unnecessary ffmpeg from Windows vcpkg install
- Make Codecov upload non-blocking
- Fix Linux dependencies and Android NDK setup
- Fix Windows async operations for windows-rs 0.62
- Format audio linux module imports
- Add Windows vcpkg dependencies for dav1d and ffmpeg
- Add FFmpeg and clang dependencies for native builds
- Install system dependencies for native builds
- Fix formatting and remove unused dependencies
- Modernize macOS and Android tests to GPU streaming
- Refactor Android module visibility and improve documentation
- support iOS Simulator swift builds
- Improve code consistency and clarity across multiple modules
- Fix lints
- run clippy for iOS and Android targets
- add multi-platform clippy and cross-compilation checks
- prepare workspace for crates.io publishing
- Add Linux, Android and Windows backends
- update workspace Cargo.toml and simplify lib.rs docs
- update tests for new clipboard, codec, and dialog APIs
- update waterkit-build and permission build scripts
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- *(android)* migrate sensor and system to ndk-context
- update CLAUDE.md with no backward compatibility rule
- *(camera)* implement RAII pattern with GPU-first frame delivery
- Remove patch
- Refractor location API. Enhance build utils
- Refractor haptic API
- Rework audio player to extract metadata with `lofty` and manage playback in a background thread, replacing the builder pattern and using `thiserror`.
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Relocate `Command` and `fs` imports to local scope within `compile_swift` and adjust an error message.
- add MIT/Apache licenses.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
- Implement platform-specific camera functionality using Camera2 API for Android, AVCaptureSession for iOS/macOS, and Nokhwa for desktop environments
- Implement platform-specific media control and audio recording features
- Implement platform-specific sensor support for Android, iOS/macOS, Linux, and Windows
- Remove outdated README content and streamline documentation
- Rename alert to dialog. Introduce file picker & printer dialog
- Clean up unused imports and improve AudioController debug implementation
- move android-build dependency declaration to top-level build-dependencies
- Immigrated from `waterui` main repository
