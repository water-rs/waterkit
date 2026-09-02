# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-haptic-v0.1.0) - 2026-02-07

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- add waterkit-build crate for shared build utilities
- Add cross-platform haptic feedback support with platform-specific implementations

### Fixed

- *(haptic)* Collapse nested if-let for clippy
- *(haptic)* Fix VibrationDevice API for windows-rs 0.62

### Other

- Fix Windows async operations for windows-rs 0.62
- Refactor Android module visibility and improve documentation
- Fix lints
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- Refractor haptic API
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
