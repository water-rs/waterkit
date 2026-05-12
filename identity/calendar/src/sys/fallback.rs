use crate::{Calendar, CalendarError, Event, EventData};
use waterkit_core::Timestamp;

#[allow(clippy::unused_async)]
pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    Err(CalendarError::Unsupported)
}

#[allow(clippy::unused_async)]
pub async fn fetch_events(
    _start: Timestamp,
    _end: Timestamp,
) -> Result<Vec<Event>, CalendarError> {
    Err(CalendarError::Unsupported)
}

#[allow(clippy::unused_async)]
pub async fn create_event(_data: EventData) -> Result<Event, CalendarError> {
    Err(CalendarError::Unsupported)
}

#[allow(clippy::unused_async)]
pub async fn delete_event(_id: &str) -> Result<(), CalendarError> {
    Err(CalendarError::Unsupported)
}
