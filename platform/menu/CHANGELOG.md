# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/water-rs/waterkit/compare/waterkit-menu-v0.0.0...waterkit-menu-v0.2.0) - 2026-10-16

### Added

- *(menu)* native menu bar crate

### Fixed

- *(menu)* test Windows item ids without giant menus
- *(menu)* destroy a failed Windows bar's menu once
- *(menu)* gate crate and macOS harness to their platforms
- *(menu)* expect on fallible Win32 calls in drop and cleanup paths
- *(menu)* address review findings on the menu bar crate

### Other

- build the shared windows-menu test only where the API exists
- Window menu standard items and windows-menu registration

### Added

- *(menu)* `StandardItem::Minimize`, `StandardItem::Zoom` and
  `StandardItem::BringAllToFront` — the macOS windows-menu items with the
  platform's standard actions; `Submenu::windows_menu` marks a submenu for
  registration as `NSApp.windowsMenu` on install. Both are macOS-only and
  fail `MenuBar::new` elsewhere; a second mark fails with
  `MenuError::DuplicateWindowsMenu`.
