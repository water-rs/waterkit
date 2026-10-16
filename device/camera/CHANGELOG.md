# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-camera-v0.1.4...waterkit-camera-v0.2.0) - 2026-10-16

### Added

- *(camera)* CPU-readable analysis frames for image analysis
- *(camera)* expose the Apple frame's CVPixelBuffer
- *(camera)* [**breaking**] adopt wgpu-external-frame 0.3.0
- *(camera)* [**breaking**] carry the full colour description on every Frame
- *(camera)* [**breaking**] import Android camera buffers without a copy
- *(camera)* [**breaking**] import Apple capture buffers without a copy
- *(camera)* [**breaking**] expose frame planes and orientation; add FrameConverter
- [**breaking**] upgrade wgpu to 30.0.1
- record compressed and raw camera video on desktop

### Fixed

- *(camera)* make the P010 test upload available on every test host ([#216](https://github.com/water-rs/waterkit/pull/216))
- *(camera)* answer Android camera operations through NativeCallback
- *(camera)* reclaim capture buffers while the stream is starved
- *(camera)* lease Android analysis images like preview frames
- *(camera)* bound the Android preview images in flight below maxImages
- *(camera)* read the SMPTE C transfer name only on macOS, which alone exports it
- *(camera)* build the session's host tests only on targets with threads
- *(camera)* scope Core Video's deprecated SMPTE C transfer name
- *(camera)* [**breaking**] record YCbCr matrix and range in WKRV raw recordings
- *(camera)* tag desktop recordings with source color
- *(camera)* tear the Android camera down exactly once
- *(camera)* measure frame timestamps from the first captured frame
- *(video-core)* decode 10-bit samples from the high bits of their element
- *(vision)* drop an unused import and link the vision module in the README
- clear the clippy lints of every wasm32 build
- clear the lints and test-build errors that workspace clippy exposes
- *(camera)* address review of the zero-copy frame path
- *(camera)* end the Android stream when a frame cannot be imported
- *(camera)* start and stop Android capture safely
- *(camera)* build the frame converter on Direct3D and GL
- *(camera)* satisfy Linux clippy and build the macOS harness
- *(camera)* satisfy clippy::assert_is_empty in the recording test
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))
- *(video)* gate vaapi behind an opt-in feature on the video path ([#93](https://github.com/water-rs/waterkit/pull/93))

### Other

- *(deps)* update every dependency to its latest release
- *(ios)* run the suite through xcodebuild test
- *(camera)* run the 10-bit FrameConverter test on every test host ([#216](https://github.com/water-rs/waterkit/pull/216))
- *(camera)* [**breaking**] call AVFoundation through objc2 instead of Swift
- Merge pull request #255 from water-rs/feat/240-swift-bridges
- *(build)* [**breaking**] one Swift bridge builder for every crate
- Merge remote-tracking branch 'origin/dev' into feat/131-vision-apple
- Merge remote-tracking branch 'origin/feat/138-vision-windows-ocr' into feat/131-vision-apple
- Merge remote-tracking branch 'origin/dev' into fix/174-raw-recording-matrix
- *(camera)* assert the recorded RGBA remainder with assert_eq
- *(camera)* give the raw NV12 remainder comparison an explicit slice type
- Merge remote-tracking branch 'origin/dev' into fix/174-raw-recording-matrix
- *(camera)* give the empty NV12 remainder comparison an explicit slice type
- *(camera)* assert the NV12 remainder with assert_eq
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- Merge pull request #227 from water-rs/feat/213-wef-0-3
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- *(camera)* tag pooled Apple test buffers with SDR primaries and transfer
- *(camera)* gate the capture clock once as its own module
- *(camera)* show the leftover bytes when a WKRV RGBA conversion is ragged
- Merge remote-tracking branch 'origin/dev' into fix/176-timestamps-teardown
- Merge remote-tracking branch 'origin/dev' into fix/176-timestamps-teardown
- *(video-core)* map matrix and range to the shared YCbCr modes once
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- Merge remote-tracking branch 'origin/dev' into fix/170-p010-precision
- Merge remote-tracking branch 'origin/fix/136-camera-zero-copy' into feat/129-vision-framework
- *(core)* share the frame Orientation from waterkit-core
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- *(camera)* check the pending desktop frame is a frame, not an error
- drive the camera, clipboard and screen examples without tokio
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- *(camera)* format the frame upload constructor call
- *(camera)* pin wgpu-external-frame's push-descriptor conversion
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-camera-v0.1.0) - 2026-02-07

### Added

- *(camera)* use ndk-context for standard access to JVM
- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- Implement full iOS app build, install, and launch process in `waterkit-test` tool, and integrate `swift-bridge` for camera and notification modules.
- Enable and test Android camera functionality with improved JNI handling and codec runtime verification.
- *(mobile)* fix audio builds on android, add ios harness, update gitignore
- add waterkit-build crate for shared build utilities
- Add photo capture and video recording functionalities across platforms; implement JPEG format support and enhance error handling
- Implement HDR support across platforms with default settings and error handling

### Fixed

- *(camera)* Fix remaining clippy lints for Linux camera
- *(camera)* Add SendableCamera wrapper for Linux V4L2 thread safety
- *(camera)* Fix RequestedFormat generic parameter syntax
- Address clippy lints and add Windows pkg-config

### Other

- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Fix lints
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- *(camera)* implement RAII pattern with GPU-first frame delivery
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
- Implement platform-specific camera functionality using Camera2 API for Android, AVCaptureSession for iOS/macOS, and Nokhwa for desktop environments
