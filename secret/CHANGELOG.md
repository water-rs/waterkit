# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-secret-v0.1.0...waterkit-secret-v0.1.1) - 2026-02-21

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- Implement cross-platform secure storage with platform-specific APIs for iOS, Android, Windows, and Linux

### Fixed

- *(ci)* resolve android secret clippy doc and style lints
- Address Linux platform compile errors

### Other

- Merge branch 'dev' into main
- Implement keystore-backed secret storage and Android biometric runtime APIs
- Refactor Android module visibility and improve documentation
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-secret-v0.1.0) - 2026-02-07

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- Implement cross-platform secure storage with platform-specific APIs for iOS, Android, Windows, and Linux

### Fixed

- Address Linux platform compile errors

### Other

- Refactor Android module visibility and improve documentation
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
