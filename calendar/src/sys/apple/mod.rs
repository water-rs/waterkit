use crate::{Calendar, CalendarError, Event, EventData};
use waterkit_core::Timestamp;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn calendar_list(callback: Box<dyn FnOnce(String, String) -> ()>);
        fn calendar_fetch_events(
            start: &str,
            end: &str,
            callback: Box<dyn FnOnce(String, String) -> ()>,
        );
        fn calendar_create_event(json: &str, callback: Box<dyn FnOnce(String, String) -> ()>);
        fn calendar_delete_event(id: &str, callback: Box<dyn FnOnce(String) -> ()>);
    }
}

pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::calendar_list(Box::new(move |json: String, error: String| {
        if error.is_empty() {
            let _ = tx.send(Ok(json));
        } else {
            let _ = tx.send(Err(CalendarError::Platform(error)));
        }
    }));
    let json = rx
        .await
        .map_err(|_| CalendarError::Platform("callback dropped".into()))??;
    Ok(json
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            if parts.len() >= 4 {
                Some(Calendar {
                    id: parts[0].to_string(),
                    title: parts[1].to_string(),
                    color: if parts[2].is_empty() {
                        None
                    } else {
                        Some(parts[2].to_string())
                    },
                    is_read_only: parts[3] == "1",
                })
            } else {
                None
            }
        })
        .collect())
}

pub async fn fetch_events(start: Timestamp, end: Timestamp) -> Result<Vec<Event>, CalendarError> {
    let start_str = start.to_string();
    let end_str = end.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::calendar_fetch_events(
        &start_str,
        &end_str,
        Box::new(move |json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(json));
            } else {
                let _ = tx.send(Err(CalendarError::Platform(error)));
            }
        }),
    );
    let json = rx
        .await
        .map_err(|_| CalendarError::Platform("callback dropped".into()))??;
    Ok(parse_events(&json))
}

pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    let json = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        data.title,
        data.notes.as_deref().unwrap_or(""),
        data.location.as_deref().unwrap_or(""),
        data.start,
        data.end,
        if data.is_all_day { "1" } else { "0" },
        data.calendar_id.as_deref().unwrap_or(""),
    );
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::calendar_create_event(
        &json,
        Box::new(move |result: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(result));
            } else {
                let _ = tx.send(Err(CalendarError::Platform(error)));
            }
        }),
    );
    let result = rx
        .await
        .map_err(|_| CalendarError::Platform("callback dropped".into()))??;
    parse_events(&result)
        .into_iter()
        .next()
        .ok_or_else(|| CalendarError::Platform("failed to create event".into()))
}

pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::calendar_delete_event(
        id,
        Box::new(move |error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(CalendarError::Platform(error)));
            }
        }),
    );
    rx.await
        .map_err(|_| CalendarError::Platform("callback dropped".into()))?
}

fn parse_events(json: &str) -> Vec<Event> {
    json.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(8, '\t').collect();
            if parts.len() < 7 {
                return None;
            }
            let start = parts[4].parse::<Timestamp>().ok()?;
            let end = parts[5].parse::<Timestamp>().ok()?;
            Some(Event {
                id: parts[0].to_string(),
                title: parts[1].to_string(),
                notes: if parts[2].is_empty() {
                    None
                } else {
                    Some(parts[2].to_string())
                },
                location: if parts[3].is_empty() {
                    None
                } else {
                    Some(parts[3].to_string())
                },
                start,
                end,
                is_all_day: parts[6] == "1",
                calendar_id: parts.get(7).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}
