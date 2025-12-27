# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
# Check all crates (fast verification)
cargo check --all-features

# Build everything
cargo build --all-features

# Run clippy with all features
cargo clippy --all-targets --all-features -- -D warnings

# Format code
cargo fmt --all

# Run tests (workspace)
cargo test --all-features

# Run a specific crate's tests
cargo test -p waterkit-audio

# Check individual features work
cargo hack check --each-feature --no-dev-deps

# Check unused dependencies
cargo machete
```

## Architecture

**Waterkit** is a modular cross-platform utility kit providing native system capabilities across iOS, Android, macOS, Windows, and Linux.

### Workspace Structure

- **Root crate (`waterkit`)**: Facade that re-exports all modules via feature flags
- **Functional crates**: `audio`, `biometric`, `camera`, `clipboard`, `codec`, `dialog`, `fs`, `haptic`, `location`, `notification`, `permission`, `screen`, `secret`, `sensor`, `system`, `video`
- **`waterkit-build`**: Shared build utilities for Swift/Kotlin compilation
- **`tests/`**: Platform-specific test harnesses (`macos/`, `ios/`, `android/`)

### Crate Internal Structure

Each crate follows this pattern:
```
src/
├── lib.rs           # Public API (types, async functions, Error enum)
├── sys/             # Private platform implementations
│   ├── mod.rs       # cfg-based platform dispatch
│   ├── apple/       # iOS/macOS (Swift bridge)
│   ├── android/     # JNI/Kotlin
│   ├── windows/     # windows-rs
│   └── linux/       # zbus/D-Bus
└── build.rs         # Swift/Kotlin compilation (if needed)
```

### Platform Bridges

- **Apple (iOS/macOS)**: `swift-bridge` for Swift interop, compiled via `waterkit-build::build_apple_bridge()`
- **Android**: JNI with Kotlin helpers, compiled via `waterkit-build::build_kotlin()`
- **Windows**: `windows-rs` crate for Win32 APIs
- **Linux**: `zbus` for D-Bus communication

### Error Handling

All crates use `thiserror::Error` with per-crate error enums:
```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum SomeError {
    #[error("descriptive message")]
    Variant,
}
```

## Coding Guidelines

<important>
- Follow fast fail principle: if an unexpected case is encountered, crash early with a clear error message rather than fallback.
- Utilize rust's type system to enforce invariants at compile time rather than runtime checks.
- Use struct, trait and generic abstractions rather than enum and type-erasure when possible.
- Put shader to a separate file rather than embedding as string literal. Same for large text assets.
- Do not write duplicated code. If you find yourself copying and pasting code, consider refactoring it into a shared function or module.
- Always utilize GPU rather than CPU
- You are not allowed to revert or restore files or hide problems. If you find a bug, fix it properly rather than working around it.
- Do not leave legacy code for fallback. If a feature is deprecated, remove all related code.
- No simplify, no stub, no fallback, no patch.
- Import third-party crates instead of writing your own implementation. Less code is better.
</important>

## Key Dependencies

- **Async**: `futures`, `async-channel`, `tokio` (tests)
- **Multimedia**: `rodio`, `cpal`, `wgpu`, `nokhwa`, `mp4`, `media-codec`
- **Apple objc2 bindings**: `objc2`, `objc2-foundation`, `objc2-core-media`, etc.

## Linting

Workspace enforces strict clippy lints (all categories at warn level). Run `cargo clippy --all-targets --all-features -- -D warnings` before committing.
