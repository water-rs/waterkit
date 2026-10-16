# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-language-v0.0.0...waterkit-language-v0.2.0) - 2026-10-16

### Added

- NativeCallback/NativeChannel replace id-keyed callback registries ([#335](https://github.com/water-rs/waterkit/pull/335))
- *(language)* translate on device through the OS translation services

### Fixed

- *(language)* report a missing Android translation service as Unavailable ([#338](https://github.com/water-rs/waterkit/pull/338))
- *(otp,language)* own per-request state in Rust handles ([#338](https://github.com/water-rs/waterkit/pull/338))
- *(language)* gate the Swift error mapping on the Translation availability and assert empty pairs with assert_eq

### Other

- release unanswered native peers from a finalizer, not Cleaner
- *(build)* [**breaking**] one Swift bridge builder for every crate
