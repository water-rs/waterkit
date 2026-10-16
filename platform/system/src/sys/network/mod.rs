//! Interpretation of desktop network state.
//!
//! The Linux and Windows bridges in `sys::desktop` only read what the
//! platform reports; every decision about what that state means — which
//! interface carries the connection, whether it is usable, what transport it
//! is — lives here, free of OS calls, so the tests run on every host.

#[cfg(any(test, target_os = "windows"))]
pub mod adapters;
#[cfg(any(test, target_os = "linux"))]
pub mod kernel;
#[cfg(any(test, target_os = "linux"))]
pub mod network_manager;
