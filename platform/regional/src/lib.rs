//! Runtime system settings context (locale, preferred languages, region,
//! timezone).
//!
//! `waterkit-regional` exposes a [`RegionalContext`] handle. Each
//! `RegionalContext` owns its own state — there is **no** global static.
//! Multiple contexts in the same process are isolated and useful for tests
//! that need a fixed locale.
//!
//! Reactive consumers should hold the [`Subscribed<SystemSettingsContext>`]
//! returned by [`RegionalContext::current`] and react to its changes via
//! `nami::Signal::watch` or `subscribed.stream()`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use waterkit_core::{Subscribed, SubscribedSink, subscribed};

/// A normalized snapshot of system settings (locale, preferred languages,
/// region, timezone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSettingsContext {
    locale_tag: String,
    preferred_languages: Vec<String>,
    region: Option<String>,
    timezone: String,
}

impl SystemSettingsContext {
    /// Constructs a normalized system settings context.
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

    /// Returns a clone with a different locale tag.
    #[must_use]
    pub fn with_locale_tag(&self, locale_tag: impl Into<String>) -> Self {
        Self::new(
            locale_tag,
            self.preferred_languages.clone(),
            self.timezone.clone(),
        )
    }
}

/// Owns one snapshot of system settings plus an override.
///
/// `RegionalContext` lives on a thread that has a nami-compatible
/// `LocalExecutor` polling (typically the UI thread) — the held
/// `Subscribed<T>` requires it. Cross-thread producers (auto-refresh
/// worker, OS callbacks) push updates through cloned [`SubscribedSink`]
/// handles, which are `Send`.
#[derive(Debug)]
pub struct RegionalContext {
    subscribed: Subscribed<SystemSettingsContext>,
    sink: SubscribedSink<SystemSettingsContext>,
    override_state: Arc<Mutex<Option<SystemSettingsContext>>>,
}

impl RegionalContext {
    /// Detects the current system settings and returns a fresh context.
    ///
    /// # Panics
    ///
    /// Panics if no `LocalExecutor` is set up on the calling thread —
    /// `RegionalContext` requires one to drive the `Subscribed` mailbox.
    #[must_use]
    pub fn new() -> Self {
        let initial = detect_system_context();
        let (subscribed, sink) = subscribed(initial);
        Self {
            subscribed,
            sink,
            override_state: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns the reactive [`Subscribed`] view. Watchers / stream
    /// consumers see updates whenever [`refresh`](Self::refresh),
    /// [`set_settings`](Self::set_settings),
    /// [`clear_override`](Self::clear_override), or
    /// [`set_locale_tag`](Self::set_locale_tag) yield a new value.
    #[must_use]
    pub const fn current(&self) -> &Subscribed<SystemSettingsContext> {
        &self.subscribed
    }

    /// Synchronous snapshot of the current settings.
    #[must_use]
    pub fn snapshot(&self) -> SystemSettingsContext {
        self.subscribed.get()
    }

    /// Re-detects from system APIs (or the active override) and pushes
    /// the next value to subscribers when it differs from the previous
    /// snapshot.
    pub fn refresh(&self) -> SystemSettingsContext {
        let next = override_snapshot(&self.override_state).unwrap_or_else(detect_system_context);
        if self.subscribed.get() != next {
            self.sink.set(next.clone());
        }
        next
    }

    /// Installs an explicit override and notifies subscribers.
    ///
    /// # Panics
    ///
    /// Panics if the internal override mutex is poisoned.
    pub fn set_settings(&self, context: SystemSettingsContext) {
        {
            let mut guard = self
                .override_state
                .lock()
                .expect("regional override mutex poisoned");
            *guard = Some(context.clone());
        }
        if self.subscribed.get() != context {
            self.sink.set(context);
        }
    }

    /// Clears any explicit override and re-reads from system APIs.
    ///
    /// # Panics
    ///
    /// Panics if the internal override mutex is poisoned.
    #[must_use]
    pub fn clear_override(&self) -> SystemSettingsContext {
        {
            let mut guard = self
                .override_state
                .lock()
                .expect("regional override mutex poisoned");
            *guard = None;
        }
        self.refresh()
    }

    /// Replaces just the locale tag while preserving timezone and the
    /// preferred-language ordering.
    pub fn set_locale_tag(&self, locale_tag: impl Into<String>) -> SystemSettingsContext {
        let current = self.snapshot();
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

        let next =
            SystemSettingsContext::new(locale_tag, preferred, current.timezone().to_string());
        self.set_settings(next.clone());
        next
    }

    /// Spawns a background thread that periodically detects system
    /// settings and pushes them through the sink. Drop the returned
    /// [`AutoRefreshGuard`] to stop the thread.
    ///
    /// `interval` of zero is normalized to two seconds.
    #[must_use]
    pub fn auto_refresh(&self, interval: Duration) -> AutoRefreshGuard {
        let interval = if interval.is_zero() {
            Duration::from_secs(2)
        } else {
            interval
        };
        let (stop, stop_rx) = mpsc::channel();
        let sink = self.sink.clone();
        let override_state = Arc::clone(&self.override_state);
        let join = thread::spawn(move || {
            loop {
                match stop_rx.recv_timeout(interval) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let next = override_snapshot(&override_state)
                            .unwrap_or_else(detect_system_context);
                        sink.set(next);
                    }
                }
            }
        });
        AutoRefreshGuard {
            stop: Some(stop),
            join: Some(join),
        }
    }
}

fn override_snapshot(
    state: &Arc<Mutex<Option<SystemSettingsContext>>>,
) -> Option<SystemSettingsContext> {
    state
        .lock()
        .expect("regional override mutex poisoned")
        .clone()
}

impl Default for RegionalContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Drop guard for [`RegionalContext::auto_refresh`].
#[derive(Debug)]
pub struct AutoRefreshGuard {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl Drop for AutoRefreshGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
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
