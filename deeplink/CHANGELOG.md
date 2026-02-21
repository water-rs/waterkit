# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/water-rs/waterkit/compare/waterkit-deeplink-v0.1.0...waterkit-deeplink-v0.1.1) - 2026-02-21

### Added

- *(deeplink)* implement windows deeplink receive hooks
- *(deeplink)* implement android deeplink listener handler

### Fixed

- *(ci)* address windows deeplink const and unused-self lints
- *(ci)* address android deeplink clippy regressions
- *(ci)* resolve remaining clippy lints on android linux windows

### Other

- *(crates)* add missing README files for publish verification
- Fix repo-wide robustness and test-suite regressions
- Add Android JNI context APIs and TTS implementation
- Add platform feature crates
