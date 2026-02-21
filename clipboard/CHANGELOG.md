# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-clipboard-v0.1.0...waterkit-clipboard-v0.1.1) - 2026-02-21

### Added

- *(clipboard)* complete API redesign with streaming and custom data types
- Complete Android mobile test coverage for 10 crates
- add waterkit-build crate for shared build utilities
- Enhance Swift bridge generation and compilation for Apple platforms; update clipboard handling in Swift
- Implement cross-platform clipboard access with support for text and image retrieval

### Fixed

- Address clippy lints (GeoClue backticks, map_unwrap_or)

### Other

- Merge branch 'dev' into main
- Fix repo-wide robustness and test-suite regressions
- Refactor Android module visibility and improve documentation
- Improve code consistency and clarity across multiple modules
- Decouple kit from WaterUI and update audio dependencies by adding `lofty` and `smol`.
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)

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
