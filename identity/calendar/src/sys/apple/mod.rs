//! Apple platform implementation backed by `EventKit` (`EKEventStore`).
//!
//! Each entry point opens its own `EKEventStore` — `EventKit` objects are
//! thread-safe and are created, used, and released inside the one async
//! call, so nothing crosses threads and no callback registry is needed.
//! Access is checked with `authorizationStatusForEntityType:` and requested
//! with `requestFullAccessToEventsWithCompletion:` when not yet determined.

use futures::channel::oneshot;
use objc2::rc::Retained;

use objc2_core_graphics::CGColor;
use objc2_event_kit::{
    EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStore, EKSpan,
};
use objc2_foundation::{NSBundle, NSDate, NSError, NSString};
use waterkit_core::Timestamp;

use crate::{Calendar, CalendarError, Event, EventData};

fn ns_error(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

/// Convert a `Timestamp` into an `NSDate`.
#[expect(
    clippy::cast_precision_loss,
    reason = "NSDate stores intervals as f64 seconds; sub-second precision survives"
)]
fn ns_date(timestamp: &Timestamp) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(
        f64::from(timestamp.subsec_nanosecond()).mul_add(1e-9, timestamp.as_second() as f64),
    )
}

/// Convert an `NSDate` into a `Timestamp`.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "NSDate stores intervals as f64 seconds; truncation is exact below one second"
)]
fn timestamp(date: &NSDate) -> Timestamp {
    let seconds = date.timeIntervalSince1970();
    let whole = seconds.floor() as i64;
    let nanos = ((seconds - whole as f64) * 1e9) as i32;
    Timestamp::new(whole, nanos).expect("EventKit dates are within Timestamp range")
}

/// Format a calendar's `CGColor` as `#RRGGBB`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "channel is clamped to 0.0..=1.0 before rounding to u8"
)]
fn calendar_color(calendar: &EKCalendar) -> Option<String> {
    // SAFETY: `CGColor` is a read-only accessor on a live calendar.
    let color = unsafe { calendar.CGColor() }?;
    let count = CGColor::number_of_components(Some(&color));
    let ptr = CGColor::components(Some(&color));
    if ptr.is_null() || count == 0 {
        return None;
    }
    // SAFETY: `components` returns `count` contiguous CGFloat values.
    let components = unsafe { core::slice::from_raw_parts(ptr, count) };
    let (r, g, b) = match components {
        // Grayscale + alpha.
        [gray, _alpha] => (*gray, *gray, *gray),
        [r, g, b, ..] => (*r, *g, *b),
        _ => return None,
    };
    let channel = |v: f64| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
    Some(format!(
        "#{:02X}{:02X}{:02X}",
        channel(r),
        channel(g),
        channel(b)
    ))
}

/// Build the crate's `Event` from a live `EKEvent`.
fn map_event(event: &EKEvent) -> Event {
    // SAFETY: all of these are read-only accessors on a live store event.
    let (identifier, title, notes, location, start, end, all_day, calendar) = unsafe {
        (
            event.eventIdentifier(),
            event.title(),
            event.notes(),
            event.location(),
            event.startDate(),
            event.endDate(),
            event.isAllDay(),
            event.calendar(),
        )
    };
    Event {
        id: identifier
            .expect("events read from the store always have an identifier")
            .to_string(),
        title: title.to_string(),
        notes: notes.map(|notes| notes.to_string()),
        location: location.map(|location| location.to_string()),
        start: timestamp(&start),
        end: timestamp(&end),
        is_all_day: all_day,
        calendar_id: calendar.map_or_else(String::new, |calendar| {
            // SAFETY: `calendarIdentifier` is a read-only accessor.
            unsafe { calendar.calendarIdentifier() }.to_string()
        }),
    }
}

/// An `EKEventStore` with event access guaranteed (or an error).
///
/// `for_write` selects the minimum access level: write-only grants can
/// create events but cannot enumerate calendars or read events.
async fn event_store(for_write: bool) -> Result<Retained<EKEventStore>, CalendarError> {
    // SAFETY: class-level authorization query.
    let mut status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    if status == EKAuthorizationStatus::NotDetermined {
        request_access().await?;
        // SAFETY: re-read after the user answered the prompt.
        status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    }
    match status {
        // `Authorized` is the `FullAccess` alias — one arm covers both.
        s if s == EKAuthorizationStatus::FullAccess => (),
        s if s == EKAuthorizationStatus::WriteOnly && for_write => (),
        _ => return Err(CalendarError::PermissionDenied),
    }
    // SAFETY: `new` on a live class. The store is created after the last
    // `.await` because `Retained` is not `Send`.
    Ok(unsafe { EKEventStore::new() })
}

/// The `Info.plist` usage key this OS version's access request requires.
///
/// Fails fast: without the key `EventKit` answers the request with an
/// opaque `XPC error communicating with calaccessd` instead of prompting.
fn require_usage_description() {
    let key = if objc2::available!(ios = 17.0, macos = 14.0) {
        "NSCalendarsFullAccessUsageDescription"
    } else {
        "NSCalendarsUsageDescription"
    };
    // SAFETY: `mainBundle` and the Info.plist lookup are read-only.
    let present = NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str(key))
        .is_some();
    assert!(
        present,
        "main bundle Info.plist is missing `{key}`; EventKit cannot request          calendar access without it — add `{key}` to the app's Info.plist"
    );
}

/// Requests event access on a throwaway store and waits for the answer.
/// The completion block retains a clone of the store, so `EventKit` cannot
/// deallocate it (and possibly cancel the request) before answering.
async fn request_access() -> Result<(), CalendarError> {
    require_usage_description();
    let (tx, rx) = oneshot::channel::<Result<(), CalendarError>>();
    let tx = std::sync::Mutex::new(Some(tx));
    {
        // Everything non-`Send` (`store`, `block`) is created and dropped
        // inside this scope; only `rx` crosses the `.await`. The copied
        // block retains its own clone of the store, so `EventKit` cannot
        // deallocate it (and possibly cancel the request) before answering.
        // SAFETY: `new` on a live class; this store only issues the request.
        let store = unsafe { EKEventStore::new() };
        let block = {
            let store = Retained::clone(&store);
            block2::RcBlock::new(move |granted: objc2::runtime::Bool, error: *mut NSError| {
                // Keeping `store` alive until the completion runs is the
                // point of this capture.
                let _ = &store;
                let result = if error.is_null() {
                    if granted.as_bool() {
                        Ok(())
                    } else {
                        Err(CalendarError::PermissionDenied)
                    }
                } else {
                    // SAFETY: `error` is a non-null pointer to a live `NSError`.
                    Err(CalendarError::Platform(
                        unsafe { &*error }.localizedDescription().to_string(),
                    ))
                };
                let tx = tx.lock().expect("sender mutex").take();
                if let Some(tx) = tx {
                    let _ = tx.send(result);
                }
            })
        };
        // `requestFullAccessToEventsWithCompletion:` exists only from iOS 17 /
        // macOS 14; the workspace deployment floor is iOS 14 / macOS 12.3, so
        // older systems take the pre-17 request API — an OS-version branch,
        // not a fallback.
        if objc2::available!(ios = 17.0, macos = 14.0) {
            // SAFETY: `RcBlock::as_ptr` yields the block pointer the completion
            // parameter expects; EventKit copies the block for the request.
            unsafe {
                store.requestFullAccessToEventsWithCompletion(block2::RcBlock::as_ptr(&block));
            }
        } else {
            #[expect(
                deprecated,
                reason = "requestAccessToEntityType:completion: is the only request API on iOS < 17 / macOS < 14"
            )]
            unsafe {
                store.requestAccessToEntityType_completion(
                    EKEntityType::Event,
                    block2::RcBlock::as_ptr(&block),
                );
            }
        }
    }
    rx.await
        .map_err(|_| CalendarError::Platform("access completion dropped".into()))?
}

/// Lists all event calendars on the device.
pub async fn list_calendars() -> Result<Vec<Calendar>, CalendarError> {
    let store = event_store(false).await?;
    // SAFETY: `calendarsForEntityType` on a live authorized store.
    let calendars = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
    Ok(calendars
        .iter()
        .map(|calendar| Calendar {
            // SAFETY: read-only accessors on a live calendar.
            id: unsafe { calendar.calendarIdentifier() }.to_string(),
            title: unsafe { calendar.title() }.to_string(),
            color: calendar_color(&calendar),
            is_read_only: !unsafe { calendar.allowsContentModifications() },
        })
        .collect())
}

/// Fetches events within a date range.
pub async fn fetch_events(start: Timestamp, end: Timestamp) -> Result<Vec<Event>, CalendarError> {
    let store = event_store(false).await?;
    let start = ns_date(&start);
    let end = ns_date(&end);
    // SAFETY: predicate + query on a live authorized store.
    let predicate =
        unsafe { store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None) };
    let events = unsafe { store.eventsMatchingPredicate(&predicate) };
    Ok(events.iter().map(|event| map_event(&event)).collect())
}

/// Creates a new event and returns it with its store-assigned identifier.
pub async fn create_event(data: EventData) -> Result<Event, CalendarError> {
    let store = event_store(true).await?;
    let calendar = if let Some(id) = data.calendar_id.as_deref() {
        // SAFETY: lookup on a live authorized store.
        unsafe { store.calendarWithIdentifier(&NSString::from_str(id)) }
            .ok_or_else(|| CalendarError::NotFound(id.to_string()))?
    } else {
        // SAFETY: read-only accessor on a live authorized store.
        unsafe { store.defaultCalendarForNewEvents() }.ok_or(CalendarError::NotAvailable)?
    };
    // SAFETY: `allowsContentModifications` is a read-only accessor.
    if !unsafe { calendar.allowsContentModifications() } {
        return Err(CalendarError::ReadOnly);
    }
    // SAFETY: `eventWithEventStore` creates an unsaved event bound to this
    // store; all setters run on the live event object.
    let event = unsafe { EKEvent::eventWithEventStore(&store) };
    let title = NSString::from_str(&data.title);
    let notes = data.notes.as_deref().map(NSString::from_str);
    let location = data.location.as_deref().map(NSString::from_str);
    let start = ns_date(&data.start);
    let end = ns_date(&data.end);
    unsafe {
        event.setTitle(Some(&title));
        event.setNotes(notes.as_deref());
        event.setLocation(location.as_deref());
        event.setStartDate(Some(&start));
        event.setEndDate(Some(&end));
        event.setAllDay(data.is_all_day);
        event.setCalendar(Some(&calendar));
        store
            .saveEvent_span_error(&event, EKSpan::ThisEvent)
            .map_err(|error| CalendarError::Platform(ns_error(&error)))?;
    }
    Ok(map_event(&event))
}

/// Deletes an event by its store identifier.
pub async fn delete_event(id: &str) -> Result<(), CalendarError> {
    let store = event_store(false).await?;
    let identifier = NSString::from_str(id);
    // SAFETY: lookup on a live authorized store.
    let event = unsafe { store.eventWithIdentifier(&identifier) }
        .ok_or_else(|| CalendarError::NotFound(id.to_string()))?;
    // SAFETY: `removeEvent` on a live store event.
    unsafe {
        store
            .removeEvent_span_error(&event, EKSpan::ThisEvent)
            .map_err(|error| CalendarError::Platform(ns_error(&error)))?;
    }
    Ok(())
}
