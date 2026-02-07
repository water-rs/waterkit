# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-biometric-v0.1.0) - 2026-02-07

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- Mobile test framework for sensor, biometric, location
- add waterkit-build crate for shared build utilities
- Add cross-platform biometric authentication support for iOS, Android, and Windows

### Fixed

- Fix HSTRING API and remove unused import for windows-rs 0.62
- *(biometric)* Fix clippy lints in Linux implementation
- Address Linux compilation and clippy issues

### Other

- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Fix lints
- Add Linux, Android and Windows backends
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
