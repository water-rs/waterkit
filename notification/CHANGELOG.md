# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-notification-v0.1.0...waterkit-notification-v0.1.1) - 2026-02-21

### Added

- *(notification)* add quick reply and updatable notification handle
- *(notification)* Add full notification API with actions, icons, and sounds
- Implement full iOS app build, install, and launch process in `waterkit-test` tool, and integrate `swift-bridge` for camera and notification modules.
- add waterkit-build crate for shared build utilities
- Implement cross-platform notification system with support for Android, iOS, and desktop platforms

### Fixed

- *(notification)* Address clippy lints for Linux

### Other

- Merge branch 'dev' into main
- Fix repo-wide robustness and test-suite regressions
- Refactor Android module visibility and improve documentation
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-notification-v0.1.0) - 2026-02-07

### Added

- *(notification)* add quick reply and updatable notification handle
- *(notification)* Add full notification API with actions, icons, and sounds
- Implement full iOS app build, install, and launch process in `waterkit-test` tool, and integrate `swift-bridge` for camera and notification modules.
- add waterkit-build crate for shared build utilities
- Implement cross-platform notification system with support for Android, iOS, and desktop platforms

### Fixed

- *(notification)* Address clippy lints for Linux

### Other

- Refactor Android module visibility and improve documentation
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
