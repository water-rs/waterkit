# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-bluetooth-v0.1.4...waterkit-bluetooth-v0.2.0) - 2026-10-16

### Fixed

- *(bluetooth)* keep the facade platform-neutral on Apple
- clear the clippy lints of every wasm32 build
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- *(bluetooth)* downcast paired devices with a checked cast
- *(waterkit-bluetooth)* hop to the main queue through waterkit_core::apple::on_main
- *(waterkit-bluetooth)* hold main-thread objects in MainThreadBound and hop with exec_async
- *(bluetooth)* [**breaking**] call CoreBluetooth and IOBluetooth through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
