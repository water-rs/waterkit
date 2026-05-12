//! Cross-platform calendar and events access.
//!
//! Provides read/write access to the device's calendar store.
//! iOS/macOS and Android use native system stores. Windows/Linux use a
//! persistent desktop store under the user's local data directory.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

use waterkit_core::Timestamp;

/// A calendar on the device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Calendar {
    /// Platform-specific calendar identifier.
    pub id: String,
    /// Calendar title/name.
    pub title: String,
    /// Calendar color as hex string (e.g., `#FF0000`).
    pub color: Option<String>,
    /// Whether the calendar is read-only.
    pub is_read_only: bool,
}

/// Recurrence frequency for repeating events.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum RecurrenceFrequency {
    /// Daily recurrence.
    Daily,
    /// Weekly recurrence.
    Weekly,
    /// Monthly recurrence.
    Monthly,
    /// Yearly recurrence.
    Yearly,
}

/// A recurrence rule.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RecurrenceRule {
    /// Frequency of recurrence.
    pub frequency: RecurrenceFrequency,
    /// Interval (e.g., every 2 weeks).
    pub interval: u32,
    /// End instant; `None` means no end.
    pub end: Option<Timestamp>,
}

/// A calendar event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Event {
    /// Platform-specific event identifier.
    pub id: String,
    /// Event title.
    pub title: String,
    /// Event description/notes.
    pub notes: Option<String>,
    /// Event location.
    pub location: Option<String>,
    /// Start instant.
    pub start: Timestamp,
    /// End instant.
    pub end: Timestamp,
    /// Whether this is an all-day event.
    pub is_all_day: bool,
    /// Calendar this event belongs to.
    pub calendar_id: String,
}

/// Data for creating an event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct EventData {
    /// Event title.
    pub title: String,
    /// Event description/notes.
    pub notes: Option<String>,
    /// Event location.
    pub location: Option<String>,
    /// Start instant.
    pub start: Timestamp,
    /// End instant.
    pub end: Timestamp,
    /// Whether this is an all-day event.
    pub is_all_day: bool,
    /// Calendar ID to add the event to.
    pub calendar_id: Option<String>,
}

/// Lists all calendars on the device.
///
/// # Errors
///
/// Returns [`CalendarError`] when calendars cannot be accessed.
pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    sys::list_calendars().await
}

/// Fetches events within a date range.
///
/// # Errors
///
/// Returns [`CalendarError`] when events cannot be fetched.
pub async fn fetch_events(start: Timestamp, end: Timestamp) -> Result<Vec<Event>, CalendarError> {
    sys::fetch_events(start, end).await
}

/// Creates a new event.
///
/// # Errors
///
/// Returns [`CalendarError`] when creation fails.
pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    sys::create_event(data).await
}

/// Deletes an event by ID.
///
/// # Errors
///
/// Returns [`CalendarError`] when deletion fails.
pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    sys::delete_event(id).await
}

/// Errors in calendar operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum CalendarError {
    /// Calendar access not available.
    #[error("calendar not available")]
    NotAvailable,
    /// Permission denied.
    #[error("calendar permission denied")]
    PermissionDenied,
    /// Event not found.
    #[error("event not found: {0}")]
    NotFound(String),
    /// Calendar is read-only.
    #[error("calendar is read-only")]
    ReadOnly,
    /// Not supported on this platform.
    #[error("not supported")]
    Unsupported,
    /// Platform error.
    #[error("platform error: {0}")]
    Platform(String),
}
