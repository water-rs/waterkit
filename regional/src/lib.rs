//! Runtime system settings context (locale, preferred languages, region, timezone).
//!
//! This crate exposes a callback-registration API that does not depend on `nami`.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// A full system settings context captured from the current system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSettingsContext {
    locale_tag: String,
    preferred_languages: Vec<String>,
    region: Option<String>,
    timezone: String,
}

impl SystemSettingsContext {
    /// Creates a normalized system settings context.
    #[must_use]
    pub fn new(
        locale_tag: impl Into<String>,
        preferred_languages: Vec<String>,
        timezone: impl Into<String>,
    ) -> Self {
        let locale_tag = normalize_locale_tag(&locale_tag.into());

        let mut normalized_languages: Vec<String> = preferred_languages
            .into_iter()
            .map(|lang| normalize_locale_tag(&lang))
            .filter(|lang| !lang.is_empty())
            .collect();

        if normalized_languages.is_empty() {
            normalized_languages.push(locale_tag.clone());
        }

        if !normalized_languages.iter().any(|lang| lang == &locale_tag) {
            normalized_languages.insert(0, locale_tag.clone());
        }

        let timezone = timezone.into();

        Self {
            region: extract_region(&locale_tag),
            locale_tag,
            preferred_languages: normalized_languages,
            timezone: if timezone.is_empty() {
                "UTC".to_string()
            } else {
                timezone
            },
        }
    }

    /// Returns the canonical locale tag.
    #[must_use]
    pub fn locale_tag(&self) -> &str {
        &self.locale_tag
    }

    /// Returns preferred languages, in descending priority.
    #[must_use]
    pub fn preferred_languages(&self) -> &[String] {
        &self.preferred_languages
    }

    /// Returns the inferred region subtag, if present.
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }

    /// Returns the IANA timezone identifier.
    #[must_use]
    pub fn timezone(&self) -> &str {
        &self.timezone
    }

    /// Returns a copy with a new locale tag.
    #[must_use]
    pub fn with_locale_tag(&self, locale_tag: impl Into<String>) -> Self {
        Self::new(
            locale_tag,
            self.preferred_languages.clone(),
            self.timezone.clone(),
        )
    }
}

type Listener = Arc<dyn Fn(SystemSettingsContext) + Send + Sync + 'static>;

struct Runtime {
    state: Mutex<State>,
    auto_refresh_started: AtomicBool,
}

struct State {
    current: SystemSettingsContext,
    override_context: Option<SystemSettingsContext>,
    listeners: HashMap<u64, Listener>,
    next_listener_id: u64,
}

impl Runtime {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                current: detect_system_context(),
                override_context: None,
                listeners: HashMap::new(),
                next_listener_id: 1,
            }),
            auto_refresh_started: AtomicBool::new(false),
        }
    }

    fn remove_listener(&self, id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.listeners.remove(&id);
        }
    }
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(Runtime::new)
}

/// Subscription handle returned by [`register_listener`].
///
/// Dropping the handle automatically unregisters the listener.
#[derive(Debug)]
pub struct ListenerHandle {
    id: Option<u64>,
}

impl ListenerHandle {
    /// Unregisters the listener immediately.
    pub fn unregister(mut self) {
        self.unregister_inner();
    }

    fn unregister_inner(&mut self) {
        if let Some(id) = self.id.take() {
            runtime().remove_listener(id);
        }
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        self.unregister_inner();
    }
}

/// Returns the current system settings snapshot.
#[must_use]
pub fn current_settings() -> SystemSettingsContext {
    runtime()
        .state
        .lock()
        .map(|state| state.current.clone())
        .unwrap_or_else(|_| detect_system_context())
}

/// Registers a listener for system settings updates.
///
/// The callback is invoked immediately with the current context.
pub fn register_listener<F>(callback: F) -> ListenerHandle
where
    F: Fn(SystemSettingsContext) + Send + Sync + 'static,
{
    let callback: Listener = Arc::new(callback);

    let (id, current) = {
        let mut state = runtime()
            .state
            .lock()
            .expect("waterkit-regional runtime mutex poisoned");
        let id = state.next_listener_id;
        state.next_listener_id = state
            .next_listener_id
            .checked_add(1)
            .unwrap_or(state.next_listener_id);
        let current = state.current.clone();
        state.listeners.insert(id, callback.clone());
        (id, current)
    };

    callback(current);

    ListenerHandle { id: Some(id) }
}

/// Refreshes system settings context and notifies listeners when changed.
#[must_use]
pub fn refresh() -> SystemSettingsContext {
    let next = {
        let state = runtime()
            .state
            .lock()
            .expect("waterkit-regional runtime mutex poisoned");
        state
            .override_context
            .clone()
            .unwrap_or_else(detect_system_context)
    };

    publish_if_changed(next)
}

/// Sets an explicit runtime context override and notifies listeners.
#[must_use]
pub fn set_settings(context: SystemSettingsContext) -> SystemSettingsContext {
    let listeners = {
        let mut state = runtime()
            .state
            .lock()
            .expect("waterkit-regional runtime mutex poisoned");
        state.override_context = Some(context.clone());
        if state.current == context {
            return context;
        }
        state.current = context.clone();
        state.listeners.values().cloned().collect::<Vec<_>>()
    };

    for listener in listeners {
        listener(context.clone());
    }

    context
}

/// Clears explicit override and re-reads context from system APIs.
#[must_use]
pub fn clear_override() -> SystemSettingsContext {
    {
        let mut state = runtime()
            .state
            .lock()
            .expect("waterkit-regional runtime mutex poisoned");
        state.override_context = None;
    }
    refresh()
}

/// Sets only locale tag while preserving current timezone and language ordering.
#[must_use]
pub fn set_locale_tag(locale_tag: impl Into<String>) -> SystemSettingsContext {
    let current = current_settings();
    let locale_tag = normalize_locale_tag(&locale_tag.into());

    let mut preferred = current.preferred_languages().to_vec();
    if let Some(pos) = preferred.iter().position(|lang| lang == &locale_tag) {
        if pos != 0 {
            preferred.remove(pos);
            preferred.insert(0, locale_tag.clone());
        }
    } else {
        preferred.insert(0, locale_tag.clone());
    }

    set_settings(SystemSettingsContext::new(
        locale_tag,
        preferred,
        current.timezone().to_string(),
    ))
}

/// Starts a background polling loop to refresh system settings periodically.
///
/// This function is idempotent and starts at most one polling thread.
pub fn start_auto_refresh(interval: Duration) {
    if runtime().auto_refresh_started.swap(true, Ordering::SeqCst) {
        return;
    }

    let interval = if interval.is_zero() {
        Duration::from_secs(2)
    } else {
        interval
    };

    thread::spawn(move || {
        loop {
            thread::sleep(interval);
            let _ = refresh();
        }
    });
}

/// Starts auto refresh with a 2-second interval.
pub fn start_auto_refresh_default() {
    start_auto_refresh(Duration::from_secs(2));
}

fn publish_if_changed(next: SystemSettingsContext) -> SystemSettingsContext {
    let listeners = {
        let mut state = runtime()
            .state
            .lock()
            .expect("waterkit-regional runtime mutex poisoned");

        if state.current == next {
            return next;
        }

        state.current = next.clone();
        state.listeners.values().cloned().collect::<Vec<_>>()
    };

    for listener in listeners {
        listener(next.clone());
    }

    next
}

fn detect_system_context() -> SystemSettingsContext {
    let mut preferred_languages: Vec<String> = sys_locale::get_locales()
        .map(|locale| normalize_locale_tag(&locale))
        .filter(|locale| !locale.is_empty())
        .collect();

    if preferred_languages.is_empty()
        && let Some(locale) = sys_locale::get_locale()
    {
        let locale = normalize_locale_tag(&locale);
        if !locale.is_empty() {
            preferred_languages.push(locale);
        }
    }

    if preferred_languages.is_empty() {
        preferred_languages.push("en-US".to_string());
    }

    let locale_tag = preferred_languages[0].clone();

    let timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string());

    SystemSettingsContext::new(locale_tag, preferred_languages, timezone)
}

fn normalize_locale_tag(raw: &str) -> String {
    let cleaned = raw.trim().replace('_', "-");
    if cleaned.is_empty() {
        return "en-US".to_string();
    }

    let parts: Vec<&str> = cleaned.split('-').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return "en-US".to_string();
    }

    let mut normalized = Vec::with_capacity(parts.len());

    for (index, part) in parts.into_iter().enumerate() {
        let canonical = if index == 0 {
            part.to_ascii_lowercase()
        } else if part.len() == 4 && part.chars().all(|c| c.is_ascii_alphabetic()) {
            let mut chars = part.chars();
            let first = chars
                .next()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or_default();
            let rest = chars.as_str().to_ascii_lowercase();
            format!("{first}{rest}")
        } else if (part.len() == 2 && part.chars().all(|c| c.is_ascii_alphabetic()))
            || (part.len() == 3 && part.chars().all(|c| c.is_ascii_digit()))
        {
            part.to_ascii_uppercase()
        } else {
            part.to_string()
        };

        normalized.push(canonical);
    }

    normalized.join("-")
}

fn extract_region(locale_tag: &str) -> Option<String> {
    for subtag in locale_tag.split('-').skip(1) {
        let is_region_alpha = subtag.len() == 2 && subtag.chars().all(|c| c.is_ascii_alphabetic());
        let is_region_numeric = subtag.len() == 3 && subtag.chars().all(|c| c.is_ascii_digit());

        if is_region_alpha || is_region_numeric {
            return Some(subtag.to_ascii_uppercase());
        }

        if subtag.len() == 1 {
            // BCP47 extension singleton marks the end of language-script-region section.
            break;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{extract_region, normalize_locale_tag};

    #[test]
    fn normalize_locale_tag_handles_separator_and_casing() {
        assert_eq!(normalize_locale_tag("en_us"), "en-US");
        assert_eq!(normalize_locale_tag("ZH-hant-hk"), "zh-Hant-HK");
        assert_eq!(normalize_locale_tag("  fr-FR  "), "fr-FR");
    }

    #[test]
    fn extract_region_reads_alpha_and_numeric_regions() {
        assert_eq!(extract_region("en-US"), Some("US".to_string()));
        assert_eq!(extract_region("es-419"), Some("419".to_string()));
        assert_eq!(extract_region("zh-Hant"), None);
        assert_eq!(extract_region("en-US-u-hc-h23"), Some("US".to_string()));
    }
}
