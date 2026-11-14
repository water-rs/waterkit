# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/water-rs/waterkit/compare/waterkit-store-v0.2.0...waterkit-store-v0.3.0) - 2026-11-14

### Added

- *(build)* compile header-free Swift modules

### Fixed

- *(store)* read finish state from the session's record, not Transaction.unfinished
- *(store)* finish the transaction StoreKit delivered instead of searching unfinished

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-store-v0.0.0...waterkit-store-v0.2.0) - 2026-10-16

### Added

- *(store)* buy through the Microsoft Store on Windows
- *(store)* in-app purchases and subscriptions ([#153](https://github.com/water-rs/waterkit/pull/153))

### Fixed

- *(store)* keep a purchase's own transaction off the Apple event stream

### Other

- end events stream on drop; restore EOF newlines
- typed error wire and StoreEvents from connect (in progress)
