# waterkit-calendar

Cross-platform calendar and events access for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- List device calendars
- Fetch, create, and delete calendar events
- Recurrence rules (daily, weekly, monthly, yearly)
- ISO 8601 date/time handling

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (EventKit via Swift bridge) |
| macOS    | Native (EventKit via Swift bridge) |
| Android  | Native (CalendarProvider via JNI/Kotlin) |
| Windows  | Desktop store (persistent local data) |
| Linux    | Desktop store (persistent local data) |

## Usage

```rust
use waterkit_calendar::{list_calendars, fetch_events, create_event, EventData};

async fn example() -> Result<(), waterkit_calendar::CalendarError> {
    // List all calendars
    let calendars = list_calendars().await?;

    // Fetch events in a date range
    let events = fetch_events("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z").await?;

    // Create a new event
    let event = create_event(EventData {
        title: "Team Meeting".into(),
        start_date: "2025-06-15T10:00:00Z".into(),
        end_date: "2025-06-15T11:00:00Z".into(),
        is_all_day: false,
        notes: None,
        location: None,
        calendar_id: None,
    }).await?;

    Ok(())
}
```

## License

MIT OR Apache-2.0
