# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-notification-v0.1.4...waterkit-notification-v0.2.0) - 2026-10-16

### Added

- *(notification)* WinRT toast notifications on Windows

### Fixed

- *(notification)* [**breaking**] show Windows toasts under the application's own identity
- *(waterkit-notification)* drop unused_async allows where the lint never fires
- *(waterkit-notification)* keep the async sys signature on every platform
- *(waterkit-notification)* keep the center alive until the deliver block fires
- *(waterkit-notification)* fail fast when the host owns the notification center delegate
- *(notification)* reject unsupported Windows toast inputs
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- *(notification)* await show in the examples
- *(waterkit-notification)* [**breaking**] resolve show through a oneshot and make it async
- *(notification)* [**breaking**] call UserNotifications through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-notification-v0.1.0) - 2026-02-07

### Added

- *(notification)* add quick reply and updatable notification handle
- *(notification)* Add full notification API with actions, icons, and sounds
- Implement full iOS app build, install, and launch process in `waterkit-test` tool, and integrate `swift-bridge` for camera and notification modules.
- add waterkit-build crate for shared build utilities
- Implement cross-platform notification system with support for Android, iOS, and desktop platforms

### Fixed

- *(notification)* Address clippy lints for Linux

### Other

- Refactor Android module visibility and improve documentation
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
