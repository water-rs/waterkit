use std::path::{Path, PathBuf};

use crate::{Calendar, CalendarError, Event, EventData};
use waterkit_core::Timestamp;
use waterkit_fs::WaterFs;

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

pub async fn fetch_events(start: Timestamp, end: Timestamp) -> Result<Vec<Event>, CalendarError> {
    blocking::unblock(move || {
        if start > end {
            return Err(CalendarError::Platform(
                "start instant must be less than or equal to end".to_string(),
            ));
        }
        let path = store_path()?;
        let store = load_store(&path)?;
        Ok(store
            .events
            .into_iter()
            .filter(|event| event.end >= start && event.start <= end)
            .collect())
    })
    .await
}

pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    blocking::unblock(move || {
        if data.start > data.end {
            return Err(CalendarError::Platform(
                "event start must be less than or equal to end".to_string(),
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
            .ok_or_else(|| CalendarError::Platform(format!("calendar not found: {calendar_id}")))?;
        if calendar.is_read_only {
            return Err(CalendarError::ReadOnly);
        }

        let event = Event {
            id: format!("desktop-event-{}", store.next_event_id),
            title: data.title,
            notes: data.notes,
            location: data.location,
            start: data.start,
            end: data.end,
            is_all_day: data.is_all_day,
            calendar_id,
        };
        store.next_event_id = store
            .next_event_id
            .checked_add(1)
            .ok_or_else(|| CalendarError::Platform("desktop event id overflow".to_string()))?;
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

fn store_path() -> Result<PathBuf, CalendarError> {
    WaterFs::data_local_path(Path::new("waterkit").join("calendar").join(STORE_FILE_NAME))
        .map_err(|error| CalendarError::Platform(format!("resolve calendar store path: {error}")))
}

fn load_store(path: &Path) -> Result<CalendarStore, CalendarError> {
    WaterFs::load_json_store(path).map_err(|error| {
        CalendarError::Platform(format!("load calendar store {}: {error}", path.display()))
    })
}

fn write_store(path: &Path, store: &CalendarStore) -> Result<(), CalendarError> {
    WaterFs::write_json_store(path, store).map_err(|error| {
        CalendarError::Platform(format!("write calendar store {}: {error}", path.display()))
    })
}
