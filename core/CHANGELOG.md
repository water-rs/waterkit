# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/water-rs/waterkit/compare/waterkit-core-v0.2.0...waterkit-core-v0.3.0) - 2026-11-14

### Added

- *(core)* add macOS privacy grant permissions and honest statuses

### Fixed

- *(codec)* restore exact timestamps from Android MediaCodec

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-core-v0.1.4...waterkit-core-v0.2.0) - 2026-10-16

### Added

- *(core)* add a non-blocking main-thread hop for Apple realizations

### Fixed

- *(core)* inherit futures from the workspace as declared

### Other

- share the usage-key check through waterkit-core
- *(core)* share the frame Orientation from waterkit-core
- [**breaking**] migrate to nami's Signal::snapshot ([#94](https://github.com/water-rs/waterkit/pull/94))
