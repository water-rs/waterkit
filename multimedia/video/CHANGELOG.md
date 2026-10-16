# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-video-v0.1.4...waterkit-video-v0.2.0) - 2026-10-16

### Added

- NativeCallback/NativeChannel replace id-keyed callback registries ([#335](https://github.com/water-rs/waterkit/pull/335))
- *(codec)* [**breaking**] import decoded planes as engine-ready integer textures
- [**breaking**] upgrade wgpu to 30.0.1
- *(video)* port the filtrate effect processor to water-rs/cherenkov

### Fixed

- *(camera)* [**breaking**] record YCbCr matrix and range in WKRV raw recordings
- *(camera)* tag desktop recordings with source color
- *(video-core)* decode 10-bit samples from the high bits of their element
- clear the clippy lints of every wasm32 build
- clear the lints and test-build errors that workspace clippy exposes
- *(video-streaming)* hand MoveFileExW verbatim paths on Windows
- *(camera)* address review of the zero-copy frame path
- *(video)* keep keyframe seek safe on empty tracks
- *(video)* chain fragment bases per moof and decode by PTS
- *(video)* honor fragment timing when reading fragmented MP4
- *(container)* preserve 64-bit recording sizes and offsets ([#117](https://github.com/water-rs/waterkit/pull/117))
- *(android)* compile Kotlin helpers on the application classpath ([#108](https://github.com/water-rs/waterkit/pull/108))
- *(video)* gate vaapi behind an opt-in feature on the video path ([#93](https://github.com/water-rs/waterkit/pull/93))

### Other

- *(deps)* update every dependency to its latest release
- *(video-player)* [**breaking**] require the main thread in Apple host entry points
- *(video-player)* [**breaking**] call AVKit/AVFoundation through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- Merge branch 'fix/169-recording-colour-tags' into fix/174-raw-recording-matrix
- Merge branch 'feat/175-frame-colour-description' into fix/169-recording-colour-tags
- *(video-core)* map matrix and range to the shared YCbCr modes once
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- Merge remote-tracking branch 'origin/dev' into fix/136-camera-zero-copy
- Merge remote-tracking branch 'origin/dev' into ci/173-workspace-tests
- satisfy clippy 1.99's assert_is_empty lint
- *(codec,video-core)* share the YCbCr range and matrix WGSL
- Merge branch 'dev' into feat/efv-frames
- *(container)* name fragment run fixture parameters clearly
- Merge remote-tracking branch 'origin/dev' into feat/efv-frames
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader

## [0.1.0](https://github.com/water-rs/waterkit/releases/tag/waterkit-video-v0.1.0) - 2026-02-07

### Fixed

- Correct HEVC codec config extraction and add NV12 to BGRA conversion for Apple video decoding.

### Other

- Fix formatting and remove unused dependencies
- Fix lints
- Add Linux, Android and Windows backends
- use proper README
- Standardize workspace Cargo.toml fields, refine crate descriptions, and introduce `thiserror` for improved error handling.
- use workspace dependencies throughout and remove unused deps
- Run formatter and add READMEs
- Update dependency and fix lints
- Implement manual H.265 playback pipeline (wip)
