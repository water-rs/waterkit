# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-sensor-v0.1.4...waterkit-sensor-v0.2.0) - 2026-10-16

### Added

- *(sensor)* stream Android watches through one listener each ([#324](https://github.com/water-rs/waterkit/pull/324))

### Fixed

- *(sensor)* finish Android JNI work before the read future awaits ([#340](https://github.com/water-rs/waterkit/pull/340))
- *(sensor)* drive Android reads through NativeCallback ([#340](https://github.com/water-rs/waterkit/pull/340))
- *(sensor)* [**breaking**] keep one running manager per Apple watch and yield Result items
- clear the clippy lints of every wasm32 build
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- drive Apple watches from the main queue, not a worker thread
- Merge remote-tracking branch 'origin/dev' into feat/262-sensor-objc2
- *(sensor)* [**breaking**] call CoreMotion and IOKit through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader

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
