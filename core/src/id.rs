//! Identifier types used across the waterkit workspace.
//!
//! Currently re-exports [`Uuid`] from the `uuid` crate. Capability crates
//! should use this for service / characteristic / device UUIDs (Bluetooth,
//! NFC, etc.) instead of inventing string newtypes.

#[doc(no_inline)]
pub use uuid::Uuid;
