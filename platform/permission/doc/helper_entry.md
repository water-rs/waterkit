Runs this launch as a permission helper instance when the crate started
it as one, returning the exit code the process must end with.

Call it first thing in `main` and return its code when it is `Some`:

```no_run
fn main() -> std::process::ExitCode {
    if let Some(code) = waterkit_permission::run_helper_entry_if_requested() {
        return code;
    }
    // ... the app itself ...
    std::process::ExitCode::SUCCESS
}
```

The crate launches the app's own executable to read and request the
macOS privacy grants (see the crate docs), and a library cannot add an
entry point of its own. An ordinary launch returns `None` without side
effects, and so does every launch on other platforms.

# Panics

Panics when the launch carries the helper marker but no valid helper
command; only this crate starts helper instances, so that is a bug.
