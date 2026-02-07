# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-audio-v0.1.0) - 2026-02-07

### Added

- introduce AI guidance document and refactor audio player shutdown mechanism
- Refactor error enums to use `thiserror` and privatize `sys` modules.
- *(audio)* add read_blocking() for synchronous audio reading
- Introduce async audio streaming via `async-channel` and `futures::Stream` with a dedicated test.
- *(mobile)* fix audio builds on android, add ios harness, update gitignore
- add waterkit-build crate for shared build utilities

### Fixed

- *(audio)* Allow needless_pass_by_value on error helper fns
- *(audio)* Suppress clippy pedantic lints on legacy MediaSessionInner
- *(audio)* Fix remaining to_string_lossy in button handler
- Fix HSTRING API and remove unused import for windows-rs 0.62
- Fix Windows clippy lints and codec API for windows-rs 0.62
- *(audio)* Clone MediaCommand before pushing to pending queue
- *(audio)* Use Ref::as_ref() for TypedEventHandler args in windows-rs 0.62
- *(audio)* Implement both MediaSessionInner and MediaCenterInner for Windows
- *(audio)* Fix Windows audio module for windows-rs 0.62
- *(audio)* Fix remaining Linux clippy lints
- *(audio)* Add clippy allow attributes for Linux MPRIS module
- *(audio)* Use map_or pattern for Result handling in Linux MPRIS
- *(audio)* Add MediaCenterInner struct for Linux MPRIS
- *(audio)* Rename MediaSessionInner to MediaCenterInner for consistency
- Update zbus and sysinfo API usage for Linux
- Address clippy lints and add Windows pkg-config

### Other

- Format audio linux module imports
- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Fix lints
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- Rework audio player to extract metadata with `lofty` and manage playback in a background thread, replacing the builder pattern and using `thiserror`.
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement platform-specific media control and audio recording features
