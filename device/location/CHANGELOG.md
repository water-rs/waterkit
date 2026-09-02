# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-location-v0.1.0) - 2026-02-07

### Added

- Refactor error enums to use `thiserror` and privatize `sys` modules.
- Mobile test framework for sensor, biometric, location
- add waterkit-build crate for shared build utilities
- Add cross-platform media crate and dedicated Android and macOS test harnesses.
- Add cross-platform permission crate and refactor location with platform-specific modules and FFI implementations.

### Fixed

- *(location)* Move const to module scope to fix items_after_statements
- Fix location clippy lints and limit feature-powerset depth
- *(location)* Fix Windows Accuracy() return type for windows-rs 0.62
- Fix Windows clippy lints and location API for windows-rs 0.62
- *(location)* Refactor to use async function instead of closure
- *(location)* Use TryFrom for f64 conversion from OwnedValue
- *(location)* Fix Linux implementation for zbus 5.x API
- Android JNI type conversion and array handling

### Other

- Fix Windows async operations for windows-rs 0.62
- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- Refractor location API. Enhance build utils
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
- move android-build dependency declaration to top-level build-dependencies
- Immigrated from `waterui` main repository
