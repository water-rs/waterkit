# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-fs-v0.1.0...waterkit-fs-v0.1.1) - 2026-02-21

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- add waterkit-build crate for shared build utilities
- Add cross-platform file system utilities with support for iOS, Android, and desktop

### Other

- release v0.1.0
- Refactor Android module visibility and improve documentation
- Fix lints
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-fs-v0.1.0) - 2026-02-07

### Added

- Make platform modules private, adopt `thiserror`, and simplify camera API by removing frame structs.
- add waterkit-build crate for shared build utilities
- Add cross-platform file system utilities with support for iOS, Android, and desktop

### Other

- Refactor Android module visibility and improve documentation
- Fix lints
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
