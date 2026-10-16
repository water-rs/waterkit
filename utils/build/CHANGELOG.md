# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-build-v0.1.4...waterkit-build-v0.2.0) - 2026-10-16

### Added

- NativeCallback/NativeChannel replace id-keyed callback registries ([#335](https://github.com/water-rs/waterkit/pull/335))
- *(build)* add shared Android activity-result bridge

### Fixed

- *(build)* fail fast when the link deployment target is below the Swift floor
- *(build)* compile Swift helpers for Mac Catalyst targets
- *(build)* keep the Java exception's class and message in Android JNI errors
- *(build)* fail loudly when Swift is compiled on a non-macOS host
- clear the workspace lints in waterkit-system, waterkit-build, waterkit-test and the facade tests
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))
- *(build)* accept android-<major>.<minor> platform dirs

### Other

- register peer natives on the top-level PeerNatives class
- release unanswered native peers from a finalizer, not Cleaner
- decode OtpEvent by type, associate vision delegates, host-test peer state
- Merge remote-tracking branch 'origin/dev' into feat/240-swift-bridges
- *(build)* [**breaking**] one Swift bridge builder for every crate
- move gated Android metadata to feature subtables
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- pin shaderloom to the explicit Metal target fix
- Merge remote-tracking branch 'origin/dev' into fix/205-activity-results
- *(lints)* opt every workspace member into the workspace lints
- *(build)* share one Swift compile path between compile_swift and compile_multi_swift
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader
- *(build)* spell unparsable as the typos gate expects
- *(build)* cover minor-versioned android platform dirs

## [0.1.4](https://github.com/water-rs/waterkit/compare/waterkit-build-v0.1.3...waterkit-build-v0.1.4) - 2026-09-17

### Fixed

- *(build)* carry the compiler-rt builtins to dependent application links

## [0.1.3](https://github.com/water-rs/waterkit/compare/waterkit-build-v0.1.2...waterkit-build-v0.1.3) - 2026-09-17

### Fixed

- *(build)* link clang compiler-rt builtins alongside the Swift runtime ([#70](https://github.com/water-rs/waterkit/pull/70))

### Other

- Merge remote-tracking branch 'origin/main' into chore/sync-main-0.1.2
