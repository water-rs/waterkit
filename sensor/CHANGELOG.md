# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-sensor-v0.1.0...waterkit-sensor-v0.1.1) - 2026-02-21

### Added

- Refactor error enums to use `thiserror` and privatize `sys` modules.
- add iOS test harness and fix Android runtime issues (DEX loading, threading)
- add waterkit-build crate for shared build utilities

### Fixed

- *(audio)* Fix Windows audio module for windows-rs 0.62
- Fix Windows clippy lints and location API for windows-rs 0.62
- *(sensor)* Add ambient light sensor and fix type mismatches for Windows
- Address remaining clippy lints for Linux targets
- *(sensor)* Address clippy lints for Linux module
- Update zbus and sysinfo API usage for Linux
- Update API usage for sysinfo 0.37 and zbus
- *(sensor)* Update zbus API usage for newer version
- Address Linux platform compile errors
- add HIGH_SAMPLING_RATE_SENSORS permission and use SENSOR_DELAY_GAME for Android tests
- use main looper for sensor events in android tests to support background execution
- *(ci)* fix typos and doctest issues

### Other

- release v0.1.0
- Refactor Android module visibility and improve documentation
- Fix lints
- *(android)* migrate sensor and system to ndk-context
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement platform-specific sensor support for Android, iOS/macOS, Linux, and Windows

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-sensor-v0.1.0) - 2026-02-07

### Added

- Refactor error enums to use `thiserror` and privatize `sys` modules.
- add iOS test harness and fix Android runtime issues (DEX loading, threading)
- add waterkit-build crate for shared build utilities

### Fixed

- *(audio)* Fix Windows audio module for windows-rs 0.62
- Fix Windows clippy lints and location API for windows-rs 0.62
- *(sensor)* Add ambient light sensor and fix type mismatches for Windows
- Address remaining clippy lints for Linux targets
- *(sensor)* Address clippy lints for Linux module
- Update zbus and sysinfo API usage for Linux
- Update API usage for sysinfo 0.37 and zbus
- *(sensor)* Update zbus API usage for newer version
- Address Linux platform compile errors
- add HIGH_SAMPLING_RATE_SENSORS permission and use SENSOR_DELAY_GAME for Android tests
- use main looper for sensor events in android tests to support background execution
- *(ci)* fix typos and doctest issues

### Other

- Refactor Android module visibility and improve documentation
- Fix lints
- *(android)* migrate sensor and system to ndk-context
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement platform-specific sensor support for Android, iOS/macOS, Linux, and Windows
