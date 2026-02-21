# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-dialog-v0.1.0...waterkit-dialog-v0.1.1) - 2026-02-21

### Other

- auto-resolve JNI context via ndk-context

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-dialog-v0.1.0) - 2026-02-07

### Added

- Add native photo picker functionality and introduce `DialogError` for improved error handling.
- add waterkit-build crate for shared build utilities
- Enhance Swift bridge generation and compilation for Apple platforms; update clipboard handling in Swift

### Other

- Fix formatting and remove unused dependencies
- Refactor Android module visibility and improve documentation
- Improve code consistency and clarity across multiple modules
- Fix lints
- apply rustfmt to dialog, haptic, camera, audio, biometric, location
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Rename alert to dialog. Introduce file picker & printer dialog
