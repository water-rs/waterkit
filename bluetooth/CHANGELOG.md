# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-bluetooth-v0.1.0...waterkit-bluetooth-v0.1.1) - 2026-02-21

### Fixed

- *(bluetooth)* make windows async paths send-safe without future_not_send allow
- *(ci)* resolve windows nfc and bluetooth clippy lints
- *(ci)* silence const lint on cross-platform bluetooth wrappers
- *(ci)* resolve android nfc and linux bluetooth clippy lints
- *(ci)* resolve mobile clippy regressions in background and bluetooth

### Other

- *(crates)* add missing README files for publish verification
- Fix repo-wide robustness and test-suite regressions
- Add Android JNI context APIs and TTS implementation
- Add Linux and Windows test crates and guards
- Add platform feature crates
