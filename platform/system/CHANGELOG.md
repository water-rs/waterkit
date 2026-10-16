# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-system-v0.1.4...waterkit-system-v0.2.0) - 2026-10-16

### Fixed

- *(system)* [**breaking**] make load() async and wait out the CPU sample window via futures-timer
- *(system)* read desktop connectivity from the platform's network state
- *(system)* [**breaking**] report bridge failures as SystemError instead of defaults
- clear the workspace lints in waterkit-system, waterkit-build, waterkit-test and the facade tests
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- *(system)* declare Network.framework in-crate and make connectivity async
- *(system)* call Mach host APIs through mach2
- *(system)* [**breaking**] call Network and Foundation through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- *(lints)* opt every workspace member into the workspace lints

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-system-v0.1.0) - 2026-02-07

### Added

- Extend mobile test harness to all crates
- add waterkit-build crate for shared build utilities
- Implement system information retrieval for macOS, iOS, Android, and desktop platforms; add connectivity, thermal state, and system load functionalities

### Fixed

- Address remaining clippy lints for Linux targets
- Update zbus and sysinfo API usage for Linux
- Update API usage for sysinfo 0.37 and zbus

### Other

- Refactor Android module visibility and improve documentation
- prepare workspace for crates.io publishing
- *(android)* migrate sensor and system to ndk-context
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
