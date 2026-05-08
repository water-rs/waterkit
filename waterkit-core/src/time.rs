//! Standard time types used across the waterkit workspace.
//!
//! Every time-bearing API in waterkit uses one of these. Strings, `u64`
//! milliseconds, `u64` nanoseconds — none of those are acceptable in public
//! signatures. Use:
//!
//! - [`Timestamp`] for absolute instants. Use [`Zoned`] when a wall-clock
//!   time with timezone is needed.
//! - [`Duration`] for elapsed time / delta values. Use [`Span`] for
//!   calendar-aware differences (months, years).

#[doc(no_inline)]
pub use core::time::Duration;
#[doc(no_inline)]
pub use jiff::{Span, Timestamp, Zoned};
