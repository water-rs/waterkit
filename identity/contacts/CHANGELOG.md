# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-contacts-v0.1.4...waterkit-contacts-v0.2.0) - 2026-10-16

### Fixed

- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))

### Other

- share the usage-key check through waterkit-core
- fail fast on missing usage keys and gate the harness on permission
- *(waterkit-contacts)* [**breaking**] call Contacts through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
