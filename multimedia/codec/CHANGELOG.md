# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-codec-v0.1.0) - 2026-02-07

### Added

- *(codec)* GPU-first zero-copy video codec with streaming API
- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- *(mobile)* fix audio builds on android, add ios harness, update gitignore
- Introduce top-level `waterkit` crate with modular features and refactor `codec` module's Apple platform implementation.
- Implement zero-copy video decoding and rendering on macOS using IOSurface and Metal interop
- Implement video codec support with AV1 encoding/decoding; add platform-specific implementations for Apple, Android, and Windows

### Fixed

- *(codec)* Fix remaining 2 clippy lints (raw pointer, const fn)
- *(codec)* Fix all Windows clippy lints
- *(codec)* Fix GetOutputStreamInfo API for windows-rs 0.62
- Fix Windows clippy lints and codec API for windows-rs 0.62
- *(codec)* Add backticks around FFmpeg in doc comments
- *(codec)* Add clippy allow attributes and fix doc comments for Linux
- *(codec)* Fix Linux FFmpeg context consumption errors
- add clippy allows and missing metadata for waterkit-build
- Correct HEVC codec config extraction and add NV12 to BGRA conversion for Apple video decoding.

### Other

- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Improve code consistency and clarity across multiple modules
- Add Linux, Android and Windows backends
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
