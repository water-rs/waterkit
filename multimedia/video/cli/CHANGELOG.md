# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-video-cli-v0.1.4...waterkit-video-cli-v0.2.0) - 2026-10-16

### Added

- *(store)* in-app purchases and subscriptions ([#153](https://github.com/water-rs/waterkit/pull/153))
- *(wallet)* add waterkit-wallet for Google Wallet and Apple Wallet passes
- *(menu)* native menu bar crate
- *(otp)* add SMS one-time-code retrieval crate
- *(language)* translate on device through the OS translation services
- *(vision)* add the waterkit-vision request framework
- *(codec)* [**breaking**] import decoded planes as engine-ready integer textures
- [**breaking**] upgrade wgpu to 30.0.1
- record compressed and raw camera video on desktop

### Fixed

- *(vision)* drop an unused import and link the vision module in the README
- *(video)* gate vaapi behind an opt-in feature on the video path ([#93](https://github.com/water-rs/waterkit/pull/93))

### Other

- Merge pull request #208 from water-rs/feat/159-translation
- Merge pull request #236 from water-rs/feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge origin/dev into feat/156-wallet; save passes through a library-owned activity
- Merge remote-tracking branch 'origin/dev' into feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/151-sms-otp
- *(codec)* [**breaking**] split AV1 software decode and encode; gate the Swift bridge toolchain on Apple hosts; package the YUV shader

## [0.1.2](https://github.com/water-rs/waterkit/compare/waterkit-video-cli-v0.1.1...waterkit-video-cli-v0.1.2) - 2026-09-11

### Other

- update Cargo.lock dependencies
