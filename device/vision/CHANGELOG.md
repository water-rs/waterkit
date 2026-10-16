# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-vision-v0.0.0...waterkit-vision-v0.2.0) - 2026-10-16

### Added

- NativeCallback/NativeChannel replace id-keyed callback registries ([#335](https://github.com/water-rs/waterkit/pull/335))
- *(vision)* pass document scanner pages as typed pixel data
- *(vision)* structured document recognition through Apple Vision
- *(camera)* CPU-readable analysis frames for image analysis
- *(vision)* barcode detection and text recognition through Apple Vision
- *(vision)* widen the scanner vocabulary and split its result type
- *(vision)* serve barcode and text requests from ML Kit on Android
- *(vision)* recognize text on Windows through Windows.Media.Ocr
- *(vision)* add one-shot system code scanner
- *(vision)* add the waterkit-vision request framework

### Fixed

- *(vision)* answer ML Kit tasks through NativeCallback instead of Tasks.await
- *(vision)* leave the system scanner out of Mac Catalyst builds
- *(vision)* spell unparsable consistently
- *(vision)* build the Apple Vision change on every non-Apple target
- *(vision)* gate the frame pixel test helper to the test that uses it
- *(vision)* clear the Windows, wasm and spelling findings of CI
- *(vision)* build the text capability cleanly where no native recognizer exists
- *(vision)* bound futures with wgpu::WasmNotSend instead of the fragile wasm feature
- *(vision)* prepare tuple elements concurrently

### Other

- *(deps)* update every dependency to its latest release
- decode OtpEvent by type, associate vision delegates, host-test peer state
- Merge origin/dev into feat/137-vision-mlkit
- Merge remote-tracking branch 'origin/dev' into feat/137-vision-mlkit
- Merge origin/dev into feat/137-vision-mlkit
- *(vision)* system document scanner
- Merge remote-tracking branch 'origin/dev' into feat/142-recognize-document
- Merge pull request #255 from water-rs/feat/240-swift-bridges
- Merge remote-tracking branch 'origin/dev' into feat/137-vision-mlkit
- Merge pull request #253 from water-rs/feat/247-analysis-frames
- Merge remote-tracking branch 'origin/dev' into feat/240-swift-bridges
- *(build)* [**breaking**] one Swift bridge builder for every crate
- move gated Android metadata to feature subtables
- Merge remote-tracking branch 'origin/dev' into feat/131-vision-apple
- Merge remote-tracking branch 'origin/feat/138-vision-windows-ocr' into feat/131-vision-apple
- Merge remote-tracking branch 'origin/dev' into feat/135-system-code-scanner
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- Merge remote-tracking branch 'origin/dev' into feat/175-frame-colour-description
- *(vision)* state why wasm32 futures aren't Send
- *(vision)* say why tuple runs stay sequential
