//! # waterkit-core
//!
//! Shared primitives for the [waterkit](https://github.com/water-rs/waterkit)
//! utility kit. See `README.md` for the full design rationale.
//!
//! Re-exports the most-used items so capability crates can write
//! `use waterkit_core::{Subscribed, Brightness, Permission, Timestamp};`
//! without fishing through submodules.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![forbid(unsafe_code)]

pub mod capability;
pub mod error;
pub mod id;
pub mod permission;
pub mod subscribed;
pub mod time;
pub mod units;

pub use capability::Capabilities;
pub use error::CoreError;
pub use id::Uuid;
pub use permission::{HealthDataKind, Permission, PermissionError, PermissionStatus};
pub use subscribed::{Subscribed, SubscribedSink, subscribed, subscribed_with_executor};
pub use time::{Duration, Timestamp};
pub use units::{
    Brightness, Latitude, Longitude, OutOfRange, Pan, Pitch, PlaybackRate, RefreshRate, Volume,
    Zoom,
};
