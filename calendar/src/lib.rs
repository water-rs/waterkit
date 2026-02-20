//! Cross-platform calendar and events access.
//!
//! Provides read/write access to the device's calendar store.
//! Full implementation on iOS, macOS, and Android.

#![warn(missing_docs)]

mod sys;

/// A calendar on the device.
#[derive(Debug, Clone)]
pub struct Calendar {
    /// Platform-specific calendar identifier.
    pub id: String,
    /// Calendar title/name.
    pub title: String,
    /// Calendar color as hex string (e.g., "#FF0000").
    pub color: Option<String>,
    /// Whether the calendar is read-only.
    pub is_read_only: bool,
}

/// Recurrence frequency for repeating events.
#[derive(Debug, Clone, Copy)]
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
pub struct RecurrenceRule {
    /// Frequency of recurrence.
    pub frequency: RecurrenceFrequency,
    /// Interval (e.g., every 2 weeks).
    pub interval: u32,
    /// End date (ISO 8601). None = no end.
    pub end_date: Option<String>,
}

/// A calendar event.
#[derive(Debug, Clone)]
pub struct Event {
    /// Platform-specific event identifier.
    pub id: String,
    /// Event title.
    pub title: String,
    /// Event description/notes.
    pub notes: Option<String>,
    /// Event location.
    pub location: Option<String>,
    /// Start date/time (ISO 8601).
    pub start_date: String,
    /// End date/time (ISO 8601).
    pub end_date: String,
    /// Whether this is an all-day event.
    pub is_all_day: bool,
    /// Calendar this event belongs to.
    pub calendar_id: String,
}

/// Data for creating an event.
#[derive(Debug, Clone)]
pub struct EventData {
    /// Event title.
    pub title: String,
    /// Event description/notes.
    pub notes: Option<String>,
    /// Event location.
    pub location: Option<String>,
    /// Start date/time (ISO 8601).
    pub start_date: String,
    /// End date/time (ISO 8601).
    pub end_date: String,
    /// Whether this is an all-day event.
    pub is_all_day: bool,
    /// Calendar ID to add the event to.
    pub calendar_id: Option<String>,
}

/// List all calendars on the device.
///
/// # Errors
/// Returns error if calendars cannot be accessed.
pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    sys::list_calendars().await
}

/// Fetch events within a date range.
///
/// # Errors
/// Returns error if events cannot be fetched.
pub async fn fetch_events(start_date: &str, end_date: &str) -> Result<Vec<Event>, CalendarError> {
    sys::fetch_events(start_date, end_date).await
}

/// Create a new event.
///
/// # Errors
/// Returns error if creation fails.
pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    sys::create_event(data).await
}

/// Delete an event by ID.
///
/// # Errors
/// Returns error if deletion fails.
pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    sys::delete_event(id).await
}

/// Errors in calendar operations.
#[derive(Debug, Clone, thiserror::Error)]
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
    NotSupported,
    /// Platform error.
    #[error("platform error: {0}")]
    PlatformError(String),
}
