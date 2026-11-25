# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1](https://github.com/water-rs/waterkit/compare/waterkit-passkey-v0.3.0...waterkit-passkey-v0.3.1) - 2026-11-25

### Other

- declare exact R8 keep rules for all Kotlin helpers

## [0.3.0](https://github.com/water-rs/waterkit/compare/waterkit-passkey-v0.2.0...waterkit-passkey-v0.3.0) - 2026-11-16

### Fixed

- replace zbus 6 zvariant aliases
- port call sites to zbus 6 and sysinfo 0.40

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-passkey-v0.1.4...waterkit-passkey-v0.2.0) - 2026-10-16

### Fixed

- *(passkey)* gate the wire helpers by the platforms that use them
- clear the lints and test-build errors that workspace clippy exposes
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- *(deps)* update every dependency to its latest release
- *(waterkit-passkey)* hop through waterkit_core::apple::on_main
- *(passkey)* indent the method cfg attributes
- *(waterkit-passkey)* [**breaking**] call AuthenticationServices through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
