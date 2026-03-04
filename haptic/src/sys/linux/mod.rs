//! Linux haptic implementation using feedbackd over D-Bus.

use crate::{HapticError, HapticPattern, HapticStep, Intensity};
use futures::executor::block_on;
use std::collections::HashMap;
use std::thread;
use zbus::Connection;
use zbus::zvariant::OwnedValue;

const FEEDBACK_BUS_NAME: &str = "org.sigxcpu.Feedback";
const FEEDBACK_OBJECT_PATH: &str = "/org/sigxcpu/Feedback";
const FEEDBACK_INTERFACE: &str = "org.sigxcpu.Feedback";

const DBUS_BUS_NAME: &str = "org.freedesktop.DBus";
const DBUS_OBJECT_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";

const APP_ID: &str = "dev.waterui.haptic";

const IMPACT_LOW_EVENTS: &[&str] = &["button-pressed", "button-press", "message"];
const IMPACT_MEDIUM_EVENTS: &[&str] = &["button-pressed", "message", "button-press"];
const IMPACT_HIGH_EVENTS: &[&str] = &["bell-terminal", "button-pressed", "message"];
const SUCCESS_EVENTS: &[&str] = &["complete", "notification-success", "button-pressed"];
const WARNING_EVENTS: &[&str] = &["dialog-warning", "notification-warning", "button-pressed"];
const ERROR_EVENTS: &[&str] = &["dialog-error", "notification-error", "button-pressed"];

fn spawn_haptic_task(
    task: impl FnOnce() -> Result<(), HapticError> + Send + 'static,
) -> Result<(), HapticError> {
    thread::Builder::new()
        .name("waterkit-haptic-linux".into())
        .spawn(move || {
            let _ = task();
        })
        .map(|_| ())
        .map_err(|error| HapticError::InitFailed(format!("failed to spawn haptic worker: {error}")))
}

async fn feedback_service_available_async() -> bool {
    let connection = match Connection::session().await {
        Ok(connection) => connection,
        Err(_) => return false,
    };

    let dbus_proxy = match zbus::Proxy::new(
        &connection,
        DBUS_BUS_NAME,
        DBUS_OBJECT_PATH,
        DBUS_INTERFACE,
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(_) => return false,
    };

    dbus_proxy
        .call::<_, _, bool>("NameHasOwner", &(FEEDBACK_BUS_NAME,))
        .await
        .unwrap_or(false)
}

async fn trigger_feedback_event_async(event: &str) -> Result<(), HapticError> {
    let connection = Connection::session()
        .await
        .map_err(|error| HapticError::InitFailed(format!("failed to open session bus: {error}")))?;

    let feedback_proxy = zbus::Proxy::new(
        &connection,
        FEEDBACK_BUS_NAME,
        FEEDBACK_OBJECT_PATH,
        FEEDBACK_INTERFACE,
    )
    .await
    .map_err(|error| {
        HapticError::InitFailed(format!("failed to create feedback proxy: {error}"))
    })?;

    let hints: HashMap<&str, OwnedValue> = HashMap::new();
    if feedback_proxy
        .call_method("TriggerFeedback", &(APP_ID, event, hints, -1_i32))
        .await
        .is_ok()
    {
        return Ok(());
    }

    feedback_proxy
        .call_method("TriggerFeedback", &(APP_ID, event))
        .await
        .map(|_| ())
        .map_err(|error| {
            HapticError::Unknown(format!("failed to trigger feedback `{event}`: {error}"))
        })
}

fn trigger_feedback_event_candidates(candidates: &[&str]) -> Result<(), HapticError> {
    let mut last_error: Option<HapticError> = None;

    for event in candidates {
        match block_on(trigger_feedback_event_async(event)) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        HapticError::InitFailed("no valid Linux haptic event candidates configured".into())
    }))
}

fn impact_candidates(intensity: Intensity) -> &'static [&'static str] {
    if intensity.value() < 0.4 {
        IMPACT_LOW_EVENTS
    } else if intensity.value() < 0.8 {
        IMPACT_MEDIUM_EVENTS
    } else {
        IMPACT_HIGH_EVENTS
    }
}

pub fn is_available() -> bool {
    block_on(feedback_service_available_async())
}

pub fn impact(intensity: Intensity) -> Result<(), HapticError> {
    let candidates = impact_candidates(intensity);
    spawn_haptic_task(move || trigger_feedback_event_candidates(candidates))
}

pub fn selection() -> Result<(), HapticError> {
    spawn_haptic_task(|| trigger_feedback_event_candidates(IMPACT_LOW_EVENTS))
}

pub fn notification_success() -> Result<(), HapticError> {
    spawn_haptic_task(|| trigger_feedback_event_candidates(SUCCESS_EVENTS))
}

pub fn notification_warning() -> Result<(), HapticError> {
    spawn_haptic_task(|| trigger_feedback_event_candidates(WARNING_EVENTS))
}

pub fn notification_error() -> Result<(), HapticError> {
    spawn_haptic_task(|| trigger_feedback_event_candidates(ERROR_EVENTS))
}

pub fn play_pattern(pattern: &HapticPattern) -> Result<(), HapticError> {
    if pattern.steps().is_empty() {
        return Err(HapticError::InvalidPattern(
            "pattern must contain at least one step".into(),
        ));
    }

    let steps = pattern.steps().to_vec();
    spawn_haptic_task(move || {
        for step in steps {
            match step {
                HapticStep::Vibrate {
                    duration,
                    intensity,
                } => {
                    trigger_feedback_event_candidates(impact_candidates(intensity))?;
                    thread::sleep(duration);
                }
                HapticStep::Pause(duration) => thread::sleep(duration),
            }
        }
        Ok(())
    })
}
