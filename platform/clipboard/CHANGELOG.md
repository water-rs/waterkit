# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-clipboard-v0.1.4...waterkit-clipboard-v0.2.0) - 2026-10-16

### Added

- *(clipboard)* expose the Linux primary selection (X11 and Wayland)

### Fixed

- *(clipboard)* [**breaking**] watch the iOS pasteboard through notifications instead of polling
- clear the clippy lints of every wasm32 build
- *(clipboard)* write file URLs other apps can open on iOS and Android
- *(clipboard)* [**breaking**] report query failures instead of an empty clipboard
- *(clipboard)* serve the X11 selections through x11rb
- *(clipboard)* keep set_html's alt text on macOS and Windows
- *(clipboard)* choose the CLIPBOARD display server explicitly
- *(clipboard)* choose the PRIMARY display server explicitly
- *(clipboard)* keep the Linux-only PRIMARY example on the Linux-only type
- *(android)* emit clipboard watch events for same-type clip updates
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- *(clipboard)* await watch in the stream example
- *(clipboard)* take the main-thread marker checked when removing observers
- *(clipboard)* drop the link-only build script
- *(clipboard)* [**breaking**] call UIPasteboard through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- *(clipboard)* run the Unix path helper's tests only on Unix hosts
- Merge remote-tracking branch 'origin/dev' into fix/202-clipboard-file-urls
- *(clipboard)* declare the clipboard file provider in crate metadata
- drive the camera, clipboard and screen examples without tokio
- *(clipboard)* run the PRIMARY round trip against Xvfb and xclip

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-clipboard-v0.1.0) - 2026-02-07

### Added

- *(clipboard)* complete API redesign with streaming and custom data types
- Complete Android mobile test coverage for 10 crates
- add waterkit-build crate for shared build utilities
- Enhance Swift bridge generation and compilation for Apple platforms; update clipboard handling in Swift
- Implement cross-platform clipboard access with support for text and image retrieval

### Fixed

- Address clippy lints (GeoClue backticks, map_unwrap_or)

### Other

- Refactor Android module visibility and improve documentation
- Improve code consistency and clarity across multiple modules
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
