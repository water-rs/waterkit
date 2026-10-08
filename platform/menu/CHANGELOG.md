# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(menu)* `StandardItem::Minimize`, `StandardItem::Zoom` and
  `StandardItem::BringAllToFront` — the macOS windows-menu items with the
  platform's standard actions; `Submenu::windows_menu` marks a submenu for
  registration as `NSApp.windowsMenu` on install. Both are macOS-only and
  fail `MenuBar::new` elsewhere; a second mark fails with
  `MenuError::DuplicateWindowsMenu`.
