# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-wallet-v0.0.0...waterkit-wallet-v0.2.0) - 2026-10-16

### Added

- *(wallet)* add waterkit-wallet for Google Wallet and Apple Wallet passes

### Other

- *(deps)* update every dependency to its latest release
- *(wallet)* build PKPass objects on the main queue
- *(wallet)* [**breaking**] call PassKit through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
- Merge origin/dev into feat/156-wallet; save passes through a library-owned activity
