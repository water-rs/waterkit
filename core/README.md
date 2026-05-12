# waterkit-core

Shared primitives for the [waterkit](../) utility kit.

This crate exists so the 27 capability sub-crates (camera, audio, screen,
sensor, bluetooth, …) can agree on a single vocabulary for cross-cutting
concepts without depending on each other or on any UI framework.

## Modules

| Module | Purpose |
|--------|---------|
| [`subscribed`] | [`Subscribed<T>`] / [`SubscribedSink<T>`] — reactive state primitive with a single concrete return type that implements `nami::Signal` and can be turned into `futures::Stream`. Producers on any thread push values via the sink; readers on the binding's home thread observe via `get` / `watch` / `stream`. |
| [`units`] | Typed scalar wrappers with validated ranges: `Brightness`, `Volume`, `Pan`, `PlaybackRate`, `Pitch`, `Zoom`, `Latitude`, `Longitude`, `RefreshRate`. |
| [`time`] | Re-exports of `jiff::Timestamp` / `core::time::Duration`. The whole workspace standardizes on these. |
| [`id`] | Re-export of `uuid::Uuid`. |
| [`error`] | Shared `CoreError` for variants several crates need (Unsupported, PermissionDenied, Platform). |
| [`capability`] | Tiny `Capabilities` trait every capability-probe struct implements (`fn available(&self) -> bool`). |

## Decoupling

`waterkit-core` has **zero** dependency on `waterui*` and never will. It does
depend on `nami` because `Subscribed<T>` is built on `nami::Binding<T>` +
`nami::binding::BindingMailbox<T>` — the natural cross-thread reactive cell.
That keeps waterkit usable both inside waterui apps and in any other Rust
program with a nami-compatible executor.
