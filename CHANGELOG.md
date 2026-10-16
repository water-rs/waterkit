# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-v0.1.4...waterkit-v0.2.0) - 2026-10-16

### Added

- *(core)* [**breaking**] migrate to nami 0.13 / nami-core 0.5
- *(store)* buy through the Microsoft Store on Windows
- *(store)* in-app purchases and subscriptions ([#153](https://github.com/water-rs/waterkit/pull/153))
- *(sensor)* stream Android watches through one listener each ([#324](https://github.com/water-rs/waterkit/pull/324))
- *(core)* add a non-blocking main-thread hop for Apple realizations
- *(vision)* pass document scanner pages as typed pixel data
- *(vision)* structured document recognition through Apple Vision
- *(vision)* barcode detection and text recognition through Apple Vision
- *(wallet)* add waterkit-wallet for Google Wallet and Apple Wallet passes
- *(notification)* WinRT toast notifications on Windows
- *(menu)* native menu bar crate
- *(vision)* serve barcode and text requests from ML Kit on Android
- *(camera)* [**breaking**] adopt wgpu-external-frame 0.3.0
- *(vision)* recognize text on Windows through Windows.Media.Ocr
- *(vision)* add one-shot system code scanner
- *(build)* add shared Android activity-result bridge
- *(otp)* add SMS one-time-code retrieval crate
- *(language)* translate on device through the OS translation services
- *(vision)* add the waterkit-vision request framework
- *(location)* Fused Location Provider on devices with Play services
- *(camera)* [**breaking**] import Android camera buffers without a copy
- *(tests/ios)* run the harness on a physical device
- *(camera)* [**breaking**] import Apple capture buffers without a copy
- *(camera)* [**breaking**] expose frame planes and orientation; add FrameConverter
- [**breaking**] upgrade wgpu to 30.0.1
- *(video)* port the filtrate effect processor to water-rs/cherenkov
- *(clipboard)* expose the Linux primary selection (X11 and Wayland)
- record compressed and raw camera video on desktop

### Fixed

- *(notification)* [**breaking**] show Windows toasts under the application's own identity
- *(audio)* keep the rest of the lockfile as it was
- *(audio)* depend on the cpal rodio uses
- *(location)* answer location fixes through NativeCallback instead of a parked latch
- *(system)* [**breaking**] make load() async and wait out the CPU sample window via futures-timer
- *(clipboard)* [**breaking**] watch the iOS pasteboard through notifications instead of polling
- *(test)* link the iOS harness with rustc's native-static-libs
- *(build)* fail fast when the link deployment target is below the Swift floor
- *(menu)* gate crate and macOS harness to their platforms
- *(menu)* address review findings on the menu bar crate
- *(camera)* tag desktop recordings with source color
- *(video-core)* decode 10-bit samples from the high bits of their element
- *(vision)* drop an unused import and link the vision module in the README
- *(vision)* prepare tuple elements concurrently
- *(clipboard)* write file URLs other apps can open on iOS and Android
- *(clipboard)* serve the X11 selections through x11rb
- *(clipboard)* choose the CLIPBOARD display server explicitly
- *(clipboard)* choose the PRIMARY display server explicitly
- *(system)* read desktop connectivity from the platform's network state
- *(test)* keep the harness screen on only while a run is in flight
- *(test)* drain child output while waiting on a command deadline
- *(test)* wake and unlock the Android device before launching the harness
- *(system)* [**breaking**] report bridge failures as SystemError instead of defaults
- clear the workspace lints in waterkit-system, waterkit-build, waterkit-test and the facade tests
- *(camera)* address review of the zero-copy frame path
- *(clipboard)* keep the Linux-only PRIMARY example on the Linux-only type
- *(android)* emit clipboard watch events for same-type clip updates
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))
- *(deps)* resolve gpu-allocator against windows 0.62
- *(codec)* gate the Linux VA-API stack behind a vaapi feature

### Other

- *(release)* skip waterkit-video's semver check while filtrate is unpublished
- Merge pull request #366 from water-rs/chore/21-deps-latest
- *(release)* hold waterkit-video-processing back until filtrate is published
- *(deps)* update every dependency to its latest release
- *(typos)* accept the Store API's InsufficentQuantity spelling
- Merge pull request #353 from water-rs/feat/153-store-purchases
- run iOS E2E on the xcode-27 image
- *(ios)* register the StoreKit Test environment in its own launch
- install the iOS 27.0 release runtime, 24A434
- run iOS E2E with Xcode 26.6 on an installed iOS 27.0 runtime
- install the Metal toolchain on the xcode-27 image
- run iOS E2E on the xcode-27 image
- run iOS E2E on the newest installed iOS simulator runtime
- *(ios)* rewrite the storekit config to the Xcode 26 schema
- *(ios)* run the suite through xcodebuild test
- release unanswered native peers from a finalizer, not Cleaner
- *(camera)* [**breaking**] call AVFoundation through objc2 instead of Swift
- Merge pull request #333 from water-rs/fix/331-location-no-timeout
- [**breaking**] drop the library's fixed request deadline on every platform
- log which Apple path denies a request, and surface harness logs in CI
- Merge pull request #328 from water-rs/feat/275-screen-objc2
- Merge pull request #311 from water-rs/feat/281-calendar-objc2
- Merge pull request #327 from water-rs/feat/266-audio-objc2
- Merge pull request #306 from water-rs/feat/279-notification-objc2
- share the usage-key check through waterkit-core
- fail fast on missing usage keys and gate the harness on permission
- *(waterkit-calendar)* [**breaking**] call EventKit through objc2 instead of Swift
- Merge remote-tracking branch 'origin/dev' into feat/275-screen-objc2
- Merge pull request #329 from water-rs/fix/291-system-load-async
- Merge remote-tracking branch 'origin/dev' into feat/266-audio-objc2
- Merge pull request #326 from water-rs/feat/269-video-player
- Merge pull request #309 from water-rs/feat/257-bluetooth-objc2
- Merge remote-tracking branch 'origin/dev' into feat/279-notification-objc2
- Merge pull request #322 from water-rs/fix/314-clipboard-ios-notifications
- Merge pull request #325 from water-rs/fix/301-sensor-watch-streams
- *(screen)* [**breaking**] await completions and return typed errors
- *(screen)* [**breaking**] call ScreenCaptureKit and UIKit through objc2 instead of Swift
- *(audio)* [**breaking**] hold Apple players behind MainThreadBound
- *(audio)* [**breaking**] use objc2-avf-audio, promote previous session, share on_main
- *(audio)* [**breaking**] call MediaPlayer and AVFoundation through objc2 instead of Swift
- *(video-player)* [**breaking**] require the main thread in Apple host entry points
- *(video-player)* [**breaking**] call AVKit/AVFoundation through objc2 instead of Swift
- Merge remote-tracking branch 'origin/dev' into feat/257-bluetooth-objc2
- Merge pull request #312 from water-rs/feat/60-health-objc2
- Merge pull request #310 from water-rs/feat/276-system-objc2
- drive Apple watches from the main queue, not a worker thread
- Merge pull request #316 from water-rs/feat/272-deeplink-objc2
- Merge remote-tracking branch 'origin/dev' into feat/257-bluetooth-objc2
- Merge remote-tracking branch 'origin/dev' into feat/60-health-objc2
- Merge remote-tracking branch 'origin/dev' into feat/276-system-objc2
- Merge pull request #304 from water-rs/feat/270-background-objc2
- Merge remote-tracking branch 'origin/dev' into feat/272-deeplink-objc2
- Merge pull request #305 from water-rs/feat/278-dialog-objc2
- *(waterkit-notification)* [**breaking**] resolve show through a oneshot and make it async
- *(notification)* [**breaking**] call UserNotifications through objc2 instead of Swift
- *(waterkit-background)* hop through waterkit_core::apple::on_main
- *(waterkit-background)* type task handles and stay on the main queue
- *(waterkit-background)* [**breaking**] call BackgroundTasks through objc2 instead of Swift
- Merge pull request #315 from water-rs/feat/259-haptic-objc2
- Merge pull request #318 from water-rs/feat/271-clipboard-objc2
- *(waterkit-dialog)* hop through waterkit_core::apple::on_main
- *(waterkit-dialog)* [**breaking**] call UIKit, PhotosUI and Photos through objc2 instead of Swift
- Merge pull request #317 from water-rs/feat/280-share-objc2
- Merge remote-tracking branch 'origin/dev' into feat/272-deeplink-objc2
- *(haptic)* [**breaking**] call Core Haptics through objc2 instead of Swift
- Merge pull request #313 from water-rs/feat/308-adopt-on-main
- Merge pull request #307 from water-rs/feat/261-nfc-objc2
- *(clipboard)* [**breaking**] call UIPasteboard through objc2 instead of Swift
- *(share)* [**breaking**] call UIKit and AppKit through objc2 instead of Swift
- *(deeplink)* [**breaking**] call UIKit and Core Services through objc2 instead of Swift
- Merge pull request #293 from water-rs/feat/268-speech-objc2
- *(waterkit-passkey)* hop through waterkit_core::apple::on_main
- *(waterkit-health)* [**breaking**] call HealthKit through objc2 instead of Swift
- *(system)* declare Network.framework in-crate and make connectivity async
- *(system)* call Mach host APIs through mach2
- *(system)* [**breaking**] call Network and Foundation through objc2 instead of Swift
- *(waterkit-bluetooth)* hop to the main queue through waterkit_core::apple::on_main
- *(bluetooth)* [**breaking**] call CoreBluetooth and IOBluetooth through objc2 instead of Swift
- Merge pull request #295 from water-rs/feat/265-passkey-objc2
- *(nfc)* use waterkit_core::apple::on_main
- *(nfc)* [**breaking**] call CoreNFC through objc2 instead of Swift
- *(speech)* [**breaking**] call AVFAudio and Speech through objc2 instead of Swift
- Merge pull request #299 from water-rs/feat/298-core-on-main
- Merge remote-tracking branch 'origin/dev' into feat/265-passkey-objc2
- Merge pull request #303 from water-rs/feat/274-permission-objc2
- Merge remote-tracking branch 'origin/dev' into feat/274-permission-objc2
- *(ios)* leave the harness project's frameworks to rustc's link flags
- *(permission)* [**breaking**] call the Apple privacy frameworks through objc2 instead of Swift
- Merge remote-tracking branch 'origin/dev' into feat/262-sensor-objc2
- *(sensor)* [**breaking**] call CoreMotion and IOKit through objc2 instead of Swift
- Merge remote-tracking branch 'origin/dev' into feat/265-passkey-objc2
- Merge remote-tracking branch 'origin/dev' into feat/277-wallet-objc2
- Merge pull request #283 from water-rs/feat/137-vision-mlkit
- Merge remote-tracking branch 'origin/dev' into feat/277-wallet-objc2
- Merge remote-tracking branch 'origin/dev' into feat/265-passkey-objc2
- *(waterkit-passkey)* [**breaking**] call AuthenticationServices through objc2 instead of Swift
- *(wallet)* [**breaking**] call PassKit through objc2 instead of Swift
- Merge pull request #288 from water-rs/feat/267-codec-objc2
- Merge origin/dev into feat/137-vision-mlkit
- drop the duplicate block2 workspace dependency
- Merge remote-tracking branch 'origin/dev' into feat/264-contacts-objc2
- Merge pull request #287 from water-rs/feat/144-document-scanner
- *(waterkit-contacts)* [**breaking**] call Contacts through objc2 instead of Swift
- Merge pull request #286 from water-rs/feat/263-biometric-objc2
- Merge remote-tracking branch 'origin/dev' into feat/137-vision-mlkit
- *(codec)* [**breaking**] call ImageIO and Core Image through objc2 instead of Swift
- *(biometric)* [**breaking**] call LocalAuthentication through objc2 instead of Swift
- *(fs)* [**breaking**] call Foundation through objc2 instead of Swift
- *(location)* [**breaking**] call Core Location through objc2 instead of Swift
- Merge origin/dev into feat/137-vision-mlkit
- *(vision)* system document scanner
- Merge remote-tracking branch 'origin/dev' into feat/142-recognize-document
- Merge remote-tracking branch 'origin/dev' into feat/137-vision-mlkit
- *(build)* [**breaking**] one Swift bridge builder for every crate
- move gated Android metadata to feature subtables
- pass the release pull request's gate without CI
- Merge pull request #246 from water-rs/build/242-shaderloom-0.2
- Merge pull request #232 from water-rs/fix/224-winrt-notification
- Merge remote-tracking branch 'origin/dev' into build/242-shaderloom-0.2
- take shaderloom 0.2.1 from crates.io
- bound every PR-gate job at 30 minutes
- Merge remote-tracking branch 'origin/dev' into feat/131-vision-apple
- Merge pull request #208 from water-rs/feat/159-translation
- Merge pull request #236 from water-rs/feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/223-menu-bar
- Merge origin/dev into feat/156-wallet; save passes through a library-owned activity
- pin shaderloom to the explicit Metal target fix
- Merge origin/dev into feat/131-vision-apple
- Merge remote-tracking branch 'origin/dev' into feat/131-vision-apple
- Merge remote-tracking branch 'origin/feat/138-vision-windows-ocr' into feat/131-vision-apple
- Merge pull request #210 from water-rs/fix/205-activity-results
- Merge remote-tracking branch 'origin/dev' into fix/205-activity-results
- Merge remote-tracking branch 'origin/dev' into feat/135-system-code-scanner
- Merge remote-tracking branch 'origin/dev' into feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/223-menu-bar
- bound and retry every apt connection
- Merge remote-tracking branch 'origin/dev' into fix/224-winrt-notification
- exclude the macOS vision harness from the wasm and coverage legs
- Merge remote-tracking branch 'origin/dev' into feat/135-system-code-scanner
- Merge remote-tracking branch 'origin/dev' into fix/205-activity-results
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/151-sms-otp
- Merge remote-tracking branch 'origin/dev' into fix/205-activity-results
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/151-sms-otp
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge pull request #221 from water-rs/feat/138-vision-windows-ocr
- Merge pull request #227 from water-rs/feat/213-wef-0-3
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- record that no dependency comes from the Tauri project
- Merge remote-tracking branch 'origin/dev' into feat/138-vision-windows-ocr
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge remote-tracking branch 'origin/dev' into feat/135-system-code-scanner
- Merge remote-tracking branch 'origin/dev' into fix/170-p010-precision
- Merge pull request #207 from water-rs/feat/129-vision-framework
- Merge remote-tracking branch 'origin/dev' into fix/205-activity-results
- Merge remote-tracking branch 'origin/dev' into feat/151-sms-otp
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge pull request #206 from water-rs/feat/147-fused-location
- clippy the facade's vision feature on wasm32
- Merge remote-tracking branch 'origin/fix/136-camera-zero-copy' into feat/129-vision-framework
- *(camera)* depend on the published wgpu-external-frame 0.2.0
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- Merge remote-tracking branch 'origin/dev' into fix/202-clipboard-file-urls
- lint every member that builds for wasm32
- drive the camera, clipboard and screen examples without tokio
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- Merge remote-tracking branch 'origin/dev' into fix/178-primary-clipboard-test
- start the X server from a shared action in the test and coverage jobs
- stop installing dav1d, FFmpeg and other libraries nothing links
- Merge pull request #192 from water-rs/fix/189-desktop-connectivity
- *(clipboard)* run the PRIMARY round trip against Xvfb and xclip
- Merge pull request #191 from water-rs/fix/190-android-harness-wake
- negotiate the Windows AAC decoder's offered float PCM output type
- *(lints)* opt every workspace member into the workspace lints
- *(tests)* drop the macOS biometric harness's no-op build script
- lint every workspace member in the clippy jobs
- run the doctests even when a unit test failed
- run every workspace member's tests in the per-OS test jobs
- *(camera)* pin wgpu-external-frame's push-descriptor conversion
- *(ios)* report camera frames through the new Frame API
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- *(typos)* accept UDID, Apple's device identifier
- *(typos)* accept nokhwa's set_camera_requset spelling
- *(codec,video-core)* share the YCbCr range and matrix WGSL
- consume nami 0.12 / nami-core 0.4 from crates.io
- derive test-app manifest paths from the package location ([#68](https://github.com/water-rs/waterkit/pull/68))
- Merge branch 'dev' into feat/efv-frames
- *(deps)* repin cherenkov engine crates to dev f2209fb4
- pin shaderloom to its merged wgpu 30 revision
- repin cherenkov to its rewritten history
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader
- [**breaking**] migrate to nami's Signal::snapshot ([#94](https://github.com/water-rs/waterkit/pull/94))

## [0.1.4](https://github.com/water-rs/waterkit/compare/waterkit-v0.1.3...waterkit-v0.1.4) - 2026-09-17

### Other

- updated the following local packages: waterkit-fs, waterkit-dialog, waterkit-audio, waterkit-background, waterkit-biometric, waterkit-bluetooth, waterkit-calendar, waterkit-codec, waterkit-camera, waterkit-clipboard, waterkit-contacts, waterkit-deeplink, waterkit-haptic, waterkit-health, waterkit-permission, waterkit-location, waterkit-nfc, waterkit-notification, waterkit-passkey, waterkit-regional, waterkit-screen, waterkit-secret, waterkit-sensor, waterkit-share, waterkit-speech, waterkit-system, waterkit-video

## [0.1.3](https://github.com/water-rs/waterkit/compare/waterkit-v0.1.2...waterkit-v0.1.3) - 2026-09-17

### Fixed

- *(ci)* drop the removed "tools" package from the Android SDK setup
- *(audio)* build waterkit-audio for wasm32-unknown-unknown ([#63](https://github.com/water-rs/waterkit/pull/63))

### Other

- let the PR source gate accept release-plz release branches ([#77](https://github.com/water-rs/waterkit/pull/77))
- Merge remote-tracking branch 'origin/main' into chore/sync-main-0.1.2
- prebuilt cargo-outdated + incremental/debuginfo trims ([#66](https://github.com/water-rs/waterkit/pull/66))
- run tests with cargo nextest ([#65](https://github.com/water-rs/waterkit/pull/65))
- run full feature matrix on simulators and add macOS passkey harness ([#61](https://github.com/water-rs/waterkit/pull/61))
- publish to crates.io via OIDC trusted publishing ([#57](https://github.com/water-rs/waterkit/pull/57))
- gate pull requests into main so only dev may merge ([#58](https://github.com/water-rs/waterkit/pull/58))

## [0.1.2](https://github.com/water-rs/waterkit/compare/waterkit-v0.1.1...waterkit-v0.1.2) - 2026-09-11

### Added

- *(codec)* put GPU texture output behind a `gpu` feature

### Fixed

- *(ci)* preserve a reproducible dialog wasm dependency graph
- *(dialog)* admit that the browser dialog futures cannot be Send
- *(dialog)* call the real builder on the web file dialog
- *(fs)* port the web backend to the current indexed_db_futures API
- stop ignoring the tracked iOS test-app Info.plist

### Other

- install the Linux native dependencies from one place ([#54](https://github.com/water-rs/waterkit/pull/54))
- *(deps)* zenwave 0.6.1

## [0.1.1](https://github.com/water-rs/waterkit/releases/tag/waterkit-v0.1.1) - 2026-08-25

### Added

- Added AV1 track detection, demuxing, decoding, and cross-platform video pipeline support.
- Added GPU image filtering, HDR capture profiles, media streaming, and platform-native device integrations.
- Added regional, passkey, background, deeplink, health, contact, speech, NFC, and sharing packages to the publishable workspace.

### Changed

- Migrated reactive state to Nami 0.11 and updated the shared GPU and platform dependency stack.
- Reorganized WaterKit into independently publishable core, platform, device, multimedia, identity, and sharing packages.
- Made every first-party dependency require the 0.1.1 release cohort and moved Shaderloom to its registry contract.

### Fixed

- Fixed Android filesystem resolution, JNI integration, DEX loading, camera, sensor, and media behavior.
- Fixed Apple audio feature gating, location continuations, capture profiles, and video timestamp handling.
- Fixed Windows decoder parameter sets and timestamps, decoder end-of-stream handling, and cross-platform correctness gates.
- Fixed Web target support and eliminated placeholder device results in favor of real platform behavior.

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
