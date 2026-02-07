# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-build-v0.1.0) - 2026-02-07

### Added

- add iOS test harness and fix Android runtime issues (DEX loading, threading)
- add waterkit-build crate for shared build utilities

### Fixed

- Move HashSet import inside function to avoid unused warning on non-Apple
- add clippy allows and missing metadata for waterkit-build

### Other

- Fix formatting and remove unused dependencies
- support iOS Simulator swift builds
- prepare workspace for crates.io publishing
- update waterkit-build and permission build scripts
- Refractor location API. Enhance build utils
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Relocate `Command` and `fs` imports to local scope within `compile_swift` and adjust an error message.
