# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-background-v0.1.4...waterkit-background-v0.2.0) - 2026-10-16

### Fixed

- *(waterkit-background)* expect unused_async only where the lint fires
- *(waterkit-background)* make complete_task async on every platform

### Other

- *(waterkit-background)* hop through waterkit_core::apple::on_main
- *(waterkit-background)* type task handles and stay on the main queue
- *(waterkit-background)* [**breaking**] call BackgroundTasks through objc2 instead of Swift
- *(build)* [**breaking**] one Swift bridge builder for every crate
