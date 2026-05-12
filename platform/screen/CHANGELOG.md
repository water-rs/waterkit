# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-screen-v0.1.0) - 2026-02-07

### Added

- *(screen)* GPU-first zero-copy screen capture with wgpu texture output
- introduce AI guidance document and refactor audio player shutdown mechanism
- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- add waterkit-build crate for shared build utilities
- Introduce top-level `waterkit` crate with modular features and refactor `codec` module's Apple platform implementation.
- Implement zero-copy video decoding and rendering on macOS using IOSurface and Metal interop
- Enhance screen capture performance with ScreenCaptureKit and optimized ScreenCapturer; add profiling test for benchmarking
- Add raw screen capture functionality for improved performance; implement capture_screen_raw method and update platform support
- Add documentation and implement screen picker functionality for macOS; enhance platform support for screen capture and brightness control
- Add cross-platform screen capture and brightness control functionality

### Fixed

- Fix Android clippy lint and exclude platform-specific crates from coverage
- Address Linux compilation and clippy issues
- *(screen)* Make waterkit-build a universal build dependency
- Correct HEVC codec config extraction and add NV12 to BGRA conversion for Apple video decoding.
- Android JNI type conversion and array handling

### Other

- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Add Linux, Android and Windows backends
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- add MIT/Apache licenses.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
- Remove outdated README content and streamline documentation
- Immigrated from `waterui` main repository
