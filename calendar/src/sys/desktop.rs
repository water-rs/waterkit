use crate::{Calendar, CalendarError, Event, EventData};
use std::path::{Path, PathBuf};

const STORE_FILE_NAME: &str = "calendar.json";
const DEFAULT_CALENDAR_ID: &str = "desktop-default";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CalendarStore {
    next_event_id: u64,
    calendars: Vec<Calendar>,
    events: Vec<Event>,
}

impl Default for CalendarStore {
    fn default() -> Self {
        Self {
            next_event_id: 1,
            calendars: vec![Calendar {
                id: DEFAULT_CALENDAR_ID.to_string(),
                title: "Waterkit".to_string(),
                color: None,
                is_read_only: false,
            }],
            events: Vec::new(),
        }
    }
}

pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    blocking::unblock(|| {
        let path = store_path()?;
        Ok(load_store(&path)?.calendars)
    })
    .await
}

pub async fn fetch_events(start: &str, end: &str) -> Result<Vec<Event>, CalendarError> {
    let start = start.to_string();
    let end = end.to_string();
    blocking::unblock(move || {
        if start > end {
            return Err(CalendarError::PlatformError(
                "start date must be less than or equal to end date".to_string(),
            ));
        }
        let path = store_path()?;
        let store = load_store(&path)?;
        Ok(store
            .events
            .into_iter()
            .filter(|event| event_overlaps(event, &start, &end))
            .collect())
    })
    .await
}

pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    blocking::unblock(move || {
        if data.start_date > data.end_date {
            return Err(CalendarError::PlatformError(
                "event start date must be less than or equal to end date".to_string(),
            ));
        }

        let path = store_path()?;
        let mut store = load_store(&path)?;
        let calendar_id = data
            .calendar_id
            .unwrap_or_else(|| DEFAULT_CALENDAR_ID.to_string());
        let calendar = store
            .calendars
            .iter()
            .find(|calendar| calendar.id == calendar_id.as_str())
            .ok_or_else(|| {
                CalendarError::PlatformError(format!("calendar not found: {calendar_id}"))
            })?;
        if calendar.is_read_only {
            return Err(CalendarError::ReadOnly);
        }

        let event = Event {
            id: format!("desktop-event-{}", store.next_event_id),
            title: data.title,
            notes: data.notes,
            location: data.location,
            start_date: data.start_date,
            end_date: data.end_date,
            is_all_day: data.is_all_day,
            calendar_id,
        };
        store.next_event_id = store
            .next_event_id
            .checked_add(1)
            .ok_or_else(|| CalendarError::PlatformError("desktop event id overflow".to_string()))?;
        store.events.push(event.clone());
        write_store(&path, &store)?;
        Ok(event)
    })
    .await
}

pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    let id = id.to_string();
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        let Some(position) = store
            .events
            .iter()
            .position(|event| event.id == id.as_str())
        else {
            return Err(CalendarError::NotFound(id));
        };
        store.events.remove(position);
        write_store(&path, &store)
    })
    .await
}

fn event_overlaps(event: &Event, start: &str, end: &str) -> bool {
    event.end_date.as_str() >= start && event.start_date.as_str() <= end
}

fn store_path() -> Result<PathBuf, CalendarError> {
    let mut base = dirs::data_local_dir().ok_or_else(|| {
        CalendarError::PlatformError("unable to resolve local data directory".to_string())
    })?;
    base.push("waterkit");
    base.push("calendar");
    base.push(STORE_FILE_NAME);
    Ok(base)
}

fn load_store(path: &Path) -> Result<CalendarStore, CalendarError> {
    if !path.exists() {
        return Ok(CalendarStore::default());
    }
    let bytes = std::fs::read(path).map_err(|error| {
        CalendarError::PlatformError(format!("read calendar store {}: {error}", path.display()))
    })?;
    if bytes.is_empty() {
        return Ok(CalendarStore::default());
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        CalendarError::PlatformError(format!("parse calendar store {}: {error}", path.display()))
    })
}

fn write_store(path: &Path, store: &CalendarStore) -> Result<(), CalendarError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CalendarError::PlatformError(format!(
                "create calendar store directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let bytes = serde_json::to_vec_pretty(store).map_err(|error| {
        CalendarError::PlatformError(format!(
            "serialize calendar store {}: {error}",
            path.display()
        ))
    })?;
    std::fs::write(path, bytes).map_err(|error| {
        CalendarError::PlatformError(format!("write calendar store {}: {error}", path.display()))
    })
}
