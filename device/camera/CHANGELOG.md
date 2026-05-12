# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
