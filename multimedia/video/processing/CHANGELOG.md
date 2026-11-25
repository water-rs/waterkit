# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/water-rs/waterkit/compare/waterkit-video-processing-v0.2.0...waterkit-video-processing-v0.2.1) - 2026-11-25

### Other

- updated the following local packages: waterkit-video-core

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-video-processing-v0.1.4...waterkit-video-processing-v0.2.0) - 2026-11-16

### Added

- *(automation)* virtual displays and cursor-free input on macOS
- *(store)* in-app purchases and subscriptions ([#153](https://github.com/water-rs/waterkit/pull/153))
- *(wallet)* add waterkit-wallet for Google Wallet and Apple Wallet passes
- *(menu)* native menu bar crate
- *(otp)* add SMS one-time-code retrieval crate
- *(language)* translate on device through the OS translation services
- *(vision)* add the waterkit-vision request framework
- [**breaking**] upgrade wgpu to 30.0.1
- *(video)* port the filtrate effect processor to water-rs/cherenkov

### Fixed

- *(vision)* drop an unused import and link the vision module in the README
- clear the clippy lints of every wasm32 build

### Other

- *(video)* [**breaking**] drop the filtrate dependency from waterkit-video-processing
- *(release)* keep waterkit-video-processing at its published 0.1.4
- Merge pull request #208 from water-rs/feat/159-translation
- Merge pull request #236 from water-rs/feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge origin/dev into feat/156-wallet; save passes through a library-owned activity
- Merge remote-tracking branch 'origin/dev' into feat/223-menu-bar
- Merge remote-tracking branch 'origin/dev' into feat/159-translation
- Merge remote-tracking branch 'origin/dev' into feat/151-sms-otp
- Merge remote-tracking branch 'origin/fix/136-camera-zero-copy' into feat/129-vision-framework
