use crate::{Calendar, CalendarError, Event, EventData};

#[allow(clippy::unused_async)]
pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    Err(CalendarError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn fetch_events(_start: &str, _end: &str) -> Result<Vec<Event>, CalendarError> {
    Err(CalendarError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn create_event(_data: EventData) -> Result<Event, CalendarError> {
    Err(CalendarError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn delete_event(_id: &str) -> Result<(), CalendarError> {
    Err(CalendarError::NotSupported)
}
