# Waterkit System Settings

`waterkit-regional` provides a cross-platform runtime snapshot of system settings context:

- locale tag (`en-US`, `zh-Hant-HK`, ...)
- preferred language list
- region
- timezone (`America/Los_Angeles`, ...)

It also provides callback registration to observe context updates.

## API

- `current_settings()`
- `register_listener(...)`
- `refresh()`
- `start_auto_refresh(...)`
- `set_settings(...)`
- `set_locale_tag(...)`

## Notes

- This crate is callback-oriented and does **not** depend on `nami`.
- For platforms without native change notifications wired in, use `start_auto_refresh`.
