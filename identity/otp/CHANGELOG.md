# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-otp-v0.0.0...waterkit-otp-v0.2.0) - 2026-10-16

### Added

- NativeCallback/NativeChannel replace id-keyed callback registries ([#335](https://github.com/water-rs/waterkit/pull/335))
- *(otp)* add SMS one-time-code retrieval crate

### Fixed

- *(otp,language)* own per-request state in Rust handles ([#338](https://github.com/water-rs/waterkit/pull/338))
- *(otp)* use AtomicU64::try_update, the new name of the deprecated fetch_update

### Other

- decode OtpEvent by type, associate vision delegates, host-test peer state
