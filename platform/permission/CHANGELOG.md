# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-permission-v0.1.4...waterkit-permission-v0.2.0) - 2026-10-16

### Fixed

- clear the clippy lints of every wasm32 build
- *(permission)* await Apple permission answers instead of blocking
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- share the usage-key check through waterkit-core
- fail fast on missing usage keys and gate the harness on permission
- *(waterkit-permission)* keep stores alive through access completion
- *(permission)* [**breaking**] call the Apple privacy frameworks through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-permission-v0.1.0) - 2026-02-07

### Added

- Refactor error enums to use `thiserror` and privatize `sys` modules.
- Implement real iOS permission request functions
- add waterkit-build crate for shared build utilities
- Add media crate with cross-platform media session control and introduce platform-specific tests for location and media.**
- Add cross-platform permission crate and refactor location with platform-specific modules and FFI implementations.

### Fixed

- Fix CI failures for cross-platform build
- *(permission)* Fix clippy lints for Windows module
- Address clippy lints (GeoClue backticks, map_unwrap_or)
- *(permission)* Change pub(crate) to pub for re-exported functions
- Android JNI type conversion and array handling

### Other

- Fix Windows async operations for windows-rs 0.62
- Refactor Android module visibility and improve documentation
- Improve code consistency and clarity across multiple modules
- update waterkit-build and permission build scripts
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
- move android-build dependency declaration to top-level build-dependencies
