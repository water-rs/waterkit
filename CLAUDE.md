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
- No backward compatibility, just remove the old API
- Import third-party crates instead of writing your own implementation. Less code is better.
</important>

## Workspace API conventions

The 27 capability sub-crates share one design vocabulary. New sub-crates
and refactors must follow these conventions; they were established in
the May 2026 ergonomics refactor and any deviation needs an explicit
written justification.

### Foundation
- `waterkit-core` is the only crate that may export shared primitives.
  Sub-crates depend on it for `Subscribed<T>`, `Permission`,
  `PermissionStatus`, `PermissionError`, `HealthDataKind`, the unit
  newtypes (`Brightness`, `Volume`, `Pan`, `PlaybackRate`, `Pitch`,
  `Zoom`, `Latitude`, `Longitude`, `RefreshRate`), `CoreError`, the
  `Capabilities` trait, and re-exports of `jiff::{Timestamp, Zoned,
  Span}`, `core::time::Duration`, `uuid::Uuid`.
- `waterkit-core` has zero `waterui*` dependency and never will. It
  depends on `nami` unconditionally because `Subscribed<T>` is built on
  `nami::Binding` + `nami::binding::BindingMailbox`.

### Reactive surface
- "Continuous state" (current value + future updates) returns
  `Subscribed<T>` (e.g. `screen::brightness()`,
  `system::connectivity()`, `regional::current()`).
- "Discrete events" (no notion of current value) return
  `impl futures::Stream<Item = T> + Send + 'static`. Producers that
  used to expose `(Self, async_channel::Receiver<T>)` tuples must
  expose `Self::new(...)` + `self.events()` instead.
- One-shot queries return `T` (sync) or `impl Future<Output = T>`
  (async). Permissions check / request stays async because the
  underlying OS call is.
- Never expose `Pin<Box<dyn Stream<Item = T> + Send>>` as a public type
  alias. If a platform sys impl wants the alias, keep it under `mod
  sys::*`.

### Errors
- Every crate has its own thiserror-derived error enum. Variants are
  `#[non_exhaustive]`. The platform escape hatch is named `Platform`
  (one word, not `PlatformError` / `Unknown` / `System`).
- File-system errors live in `waterkit_fs::FsError`; do not return
  `std::io::Result<T>` from public surface.
- A single-error escape hatch (`CoreError`) lives in `waterkit-core`
  for crates that want to forward through it; not required.

### Naming
- Resource construction uses `Type::new(...)` for all-defaults paths
  and `Type::open(...)` when explicitly opening an OS resource (audio
  player, video reader). Long-lived sessions pair `Type::new(config)`
  with `events()` returning a stream — never `start_session` returning
  `(Self, Receiver)`.
- Capability probes return a struct named `XxxCapabilities` that
  implements `waterkit_core::Capabilities`. Replace
  `is_available() -> bool` with `capabilities() -> XxxCapabilities`
  whose `available` field carries the bit.
- "Show" methods that prompt the user (`Dialog::confirm`,
  `Dialog::alert`, `ShareSheet::show`, `Notification::show`,
  `PhotoPicker::pick`, `FileDialog::pick_single`,
  `FileDialog::pick_multiple`) are async if the platform call is
  async; sync otherwise. No faux-async `std::future::ready` wrappers.
- Sync getters use the field name. Fluent setters drop the `with_`
  prefix unless a same-named getter exists; if both exist (e.g.
  `Location::altitude()` + `with_altitude(...)`) keep `with_*` on the
  setter and document the rule in the impl.
- Drop the `get_` prefix on getters: `system::connectivity()` not
  `system::get_connectivity_info()`.

### Time and dates
- Instants: `waterkit_core::Timestamp` (re-exported from `jiff`).
- Calendar dates without time: `jiff::civil::Date`.
- Durations: `core::time::Duration`.
- Strings, `u64` epoch ms, `u64` epoch ns are forbidden in public
  API. Platform sys impls parse / format at the boundary.

### Permissions
- Permission types live in `waterkit_core::permission`; the helpers
  (`check`, `request`, `status`) live in `waterkit-permission`.
- Capability crates do **not** expose their own permission helpers.
  Document the required `Permission` variants in the crate-level docs
  and rely on the user calling
  `waterkit_permission::request(Permission::Foo).await` first.

### nami integration
- `nami` is a hard dependency, not a feature. Returning `Subscribed<T>`
  is the single canonical way for a capability crate to expose a
  reactive value; downstream code uses
  `subscribed.get()` / `.watch()` / `.map()` (Signal trait) or
  `subscribed.stream().next().await` (Stream).
- Cross-thread producers (OS callbacks, background workers) hold the
  matching `SubscribedSink<T>` and call `sink.set(value)`. The mailbox
  worker on the binding's home thread drains the closure and invokes
  `binding.set(...)` there, so the `Rc<RefCell<...>>` inside nami
  stays single-threaded.

### Fields and pub access
- Plain data records (`SensorData`, `ConnectivityInfo`, …) keep their
  field set frozen via getters; the struct is `#[non_exhaustive]` so
  new fields stay backward-compatible.
- Builder-style structs (`Notification`, `Dialog`, `FileDialog`,
  `Action`, `TextInputAction`, …) hide their fields with `pub(crate)`
  and expose accessors. Sys impls reach in via `pub(crate)`.



## Key Dependencies

- **Async**: `futures`, `async-channel`, `tokio` (tests)
- **Multimedia**: `rodio`, `cpal`, `wgpu`, `nokhwa`, `mp4`, `media-codec`
- **Apple objc2 bindings**: `objc2`, `objc2-foundation`, `objc2-core-media`, etc.

## Linting

Workspace enforces strict clippy lints (all categories at warn level). Run `cargo clippy --all-targets --all-features -- -D warnings` before committing.
