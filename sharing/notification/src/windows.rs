//! Windows application identity for toast notifications.
//!
//! A toast is attributed to the `AppUserModelID` it is shown under. Packaged
//! apps inherit one from their MSIX package; an unpackaged process must
//! register its own through [`AppIdentity::register`] before calling
//! [`Notification::show`](crate::Notification::show), or `show` fails with
//! [`NotificationError::Platform`].
//!
//! Registration also installs a COM activator (`INotificationActivationCallback`)
//! so a click on a toast is delivered to the running process, and records this
//! executable under `HKCU\Software\Classes\CLSID` so a click while nothing is
//! running launches it with the [`ACTIVATION_ARGUMENT`] command-line argument.
//!
//! ```no_run
//! # fn main() -> Result<(), waterkit_notification::NotificationError> {
//! // `registration` owns the COM class-object registration; keep it alive
//! // for as long as toasts should reach this process.
//! let registration = waterkit_notification::windows::AppIdentity::new(
//!     "com.example.myapp",
//!     "My App",
//! )
//! .register()?;
//! # let _ = registration;
//! # Ok(())
//! # }
//! ```

// The `#[implement]` expansion emits `#[inline(always)]` conversions and
// `&T as *const _` casts; lints inside it report at the macro call site.
#![allow(clippy::inline_always, clippy::ref_as_ptr)]

use std::fmt;
use std::path::PathBuf;

use ::windows::Win32::Foundation::{
    APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER, RPC_E_CHANGED_MODE,
};
use ::windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use ::windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
    CoRevokeClassObject, CoTaskMemFree, REGCLS_MULTIPLEUSE,
};
use ::windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use ::windows::Win32::UI::Shell::{
    GetCurrentProcessExplicitAppUserModelID, SetCurrentProcessExplicitAppUserModelID, ShellExecuteW,
};
use ::windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
use uuid::Uuid;
use windows_core::{GUID, HRESULT, HSTRING, IUnknown, PCWSTR, implement, w};

use crate::NotificationError;

/// The command-line argument a toast click passes to `LocalServer32`.
///
/// Applications that register an [`AppIdentity`] can match
/// `std::env::args()` against it to detect a toast-activated start.
pub const ACTIVATION_ARGUMENT: &str = "-ToastActivated";

/// The UUID v5 namespace under which activator CLSIDs are derived from the
/// `AppUserModelID`. The value is arbitrary but fixed — it only needs to be
/// stable so an id always maps to the same CLSID.
const ACTIVATOR_NAMESPACE: Uuid = Uuid::from_u128(0x63bf2602_a3cb_45dd_9185_78d5b463087a);

/// The identity a toast is attributed to on Windows: an `AppUserModelID`, a
/// display name shown in Action Center, and an optional icon.
///
/// Construct it once at startup and keep the [`AppRegistration`] returned by
/// [`AppIdentity::register`] alive for as long as the process shows or answers
/// toasts.
#[derive(Debug, Clone)]
pub struct AppIdentity {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) icon: Option<PathBuf>,
}

/// Owns the COM class-object registration made by [`AppIdentity::register`].
///
/// Dropping the guard revokes the registration with `CoRevokeClassObject`;
/// toast clicks then launch a new process through `LocalServer32` again. The
/// registry entries stay — they are what lets a click on a toast from a
/// dead process start the executable.
pub struct AppRegistration {
    cookie: u32,
}

impl fmt::Debug for AppRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppRegistration")
            .field("cookie", &self.cookie)
            .finish()
    }
}

impl Drop for AppRegistration {
    fn drop(&mut self) {
        // SAFETY: `cookie` came from a `CoRegisterClassObject` that succeeded,
        // so it is a live registration token.
        if let Err(error) = unsafe { CoRevokeClassObject(self.cookie) } {
            tracing::warn!("notification: revoking toast activator failed: {error}");
        }
    }
}

impl AppIdentity {
    /// Creates an identity from an `AppUserModelID` and the display name shown
    /// in Action Center.
    ///
    /// The id must be unique per app — a reverse-domain name such as
    /// `com.example.myapp` is the convention.
    #[must_use]
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            icon: None,
        }
    }

    /// Sets the icon shown for the app's toasts in Action Center.
    ///
    /// The path must be absolute and point to an existing file when
    /// [`register`](Self::register) runs.
    #[must_use]
    pub fn icon(mut self, path: impl Into<PathBuf>) -> Self {
        self.icon = Some(path.into());
        self
    }

    /// The `AppUserModelID` this identity registers.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The display name shown in Action Center.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// The CLSID toast clicks activate, derived deterministically from the id.
    fn activator_clsid(&self) -> GUID {
        GUID::from_u128(Uuid::new_v5(&ACTIVATOR_NAMESPACE, self.id.as_bytes()).as_u128())
    }

    /// The optional icon as a `file:///`-scheme URI for the `IconUri` value.
    fn icon_uri(&self) -> Result<Option<String>, NotificationError> {
        let Some(icon) = &self.icon else {
            return Ok(None);
        };
        if !icon.is_absolute() {
            return Err(NotificationError::Platform(format!(
                "icon {} is not an absolute path",
                icon.display()
            )));
        }
        let absolute = icon.canonicalize().map_err(|error| {
            NotificationError::Platform(format!("icon {}: {error}", icon.display()))
        })?;
        let url = url::Url::from_file_path(&absolute).map_err(|()| {
            NotificationError::Platform(format!("icon {}: not a file URL", absolute.display()))
        })?;
        Ok(Some(url.into()))
    }

    /// Registers this identity for the current process.
    ///
    /// Writes the `AppUserModelId` and `CLSID` registry keys under `HKCU`,
    /// sets the process `AppUserModelID`, and registers a COM class object so
    /// toast clicks reach this process. Call it once at startup, before the
    /// first [`Notification::show`](crate::Notification::show).
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Platform`] when the icon path is not an
    /// absolute existing file, when a registry or COM call fails, or when the
    /// process `AppUserModelID` is already set to a different id.
    pub fn register(&self) -> Result<AppRegistration, NotificationError> {
        let icon_uri = self.icon_uri()?;
        let clsid = self.activator_clsid();
        // `GUID`'s `Debug` is the uppercase unbraced form; the registry stores
        // CLSIDs in braces.
        let clsid_string = format!("{{{clsid:?}}}");

        let aumid_key = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{}", self.id))
            .map_err(platform)?;
        aumid_key
            .set_string("DisplayName", &self.display_name)
            .map_err(platform)?;
        if let Some(icon_uri) = &icon_uri {
            aumid_key
                .set_string("IconUri", icon_uri)
                .map_err(platform)?;
        }
        aumid_key
            .set_string("CustomActivator", &clsid_string)
            .map_err(platform)?;

        // The activator's local server: a toast click that finds no running
        // instance launches this executable with `ACTIVATION_ARGUMENT`.
        let executable = std::env::current_exe().map_err(|error| {
            NotificationError::Platform(format!("current executable path: {error}"))
        })?;
        windows_registry::CURRENT_USER
            .create(format!(
                r"Software\Classes\CLSID\{clsid_string}\LocalServer32"
            ))
            .map_err(platform)?
            .set_string(
                "",
                format!("\"{}\" {ACTIVATION_ARGUMENT}", executable.display()),
            )
            .map_err(platform)?;

        if current_app_user_model_id().as_deref() != Some(self.id.as_str()) {
            // SAFETY: the id outlives the call; the API only borrows it.
            unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(self.id.as_str())) }
                .map_err(platform)?;
        }

        // A COM apartment is required before a class object can be
        // registered. `RPC_E_CHANGED_MODE` means the app already initialized a
        // different model — COM is up either way.
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
            return Err(NotificationError::Platform(format!(
                "CoInitializeEx failed: {}",
                initialized.message()
            )));
        }

        let activator = INotificationActivationCallback::from(ToastActivator);
        let object = IUnknown::from(activator);
        // SAFETY: `clsid` is the CLSID the registry entry advertises and
        // `object` implements its `INotificationActivationCallback`. The
        // registration keeps its own reference, so `object` may drop when
        // `register` returns.
        let cookie = unsafe {
            CoRegisterClassObject(
                &raw const clsid,
                &object,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE,
            )
        }
        .map_err(platform)?;
        Ok(AppRegistration { cookie })
    }
}

/// Whether the process runs with package (MSIX) identity: packaged apps get
/// their `AppUserModelID` from the package and must not register one.
pub(crate) fn has_package_identity() -> Result<bool, NotificationError> {
    let mut length = 0u32;
    // SAFETY: a null buffer with a capacity of zero only queries the length.
    let error = unsafe { GetCurrentPackageFullName(&raw mut length, None) };
    if error == ERROR_INSUFFICIENT_BUFFER {
        Ok(true)
    } else if error == APPMODEL_ERROR_NO_PACKAGE {
        Ok(false)
    } else {
        Err(NotificationError::Platform(format!(
            "GetCurrentPackageFullName failed: {}",
            error.to_hresult().message()
        )))
    }
}

/// The process `AppUserModelID` set by `SetCurrentProcessExplicitAppUserModelID`,
/// or `None` when none has been set.
pub(crate) fn current_app_user_model_id() -> Option<String> {
    // SAFETY: the returned `PWSTR` is `CoTaskMem`-allocated and freed below.
    let raw = unsafe { GetCurrentProcessExplicitAppUserModelID() }.ok()?;
    let id = unsafe { raw.to_string() }.ok();
    // SAFETY: `raw` was allocated by `GetCurrentProcessExplicitAppUserModelID`
    // on the COM task allocator.
    unsafe { CoTaskMemFree(Some(raw.as_ptr().cast())) };
    id
}

/// Maps a `windows`/`windows-registry` error — both are
/// `windows_core::Error` — onto the crate's platform escape hatch.
#[expect(
    clippy::needless_pass_by_value,
    reason = "a `map_err` adapter: callers hand the error over"
)]
fn platform(error: windows_core::Error) -> NotificationError {
    NotificationError::Platform(error.to_string())
}

/// The COM class object registered for the activator CLSID.
///
/// Foreground toast activations land in `Activate`: the invoked arguments
/// mirror what an `Action`'s `protocol` activation would have received — a
/// URL to open — while a bare body click carries nothing and simply marks
/// this process as activated instead of launching another.
#[implement(INotificationActivationCallback)]
struct ToastActivator;

impl INotificationActivationCallback_Impl for ToastActivator_Impl {
    fn Activate(
        &self,
        _app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows_core::Result<()> {
        if invoked_args.is_null() {
            return Ok(());
        }
        // SAFETY: the activation contract hands over a null-terminated string
        // valid for the duration of the call.
        let args = unsafe { invoked_args.as_wide() };
        if args.is_empty() {
            return Ok(());
        }
        open_target(&String::from_utf16_lossy(args))
    }
}

/// Opens an activated argument the way a `protocol` action's URL opens: handed
/// to the shell with the `open` verb.
fn open_target(target: &str) -> windows_core::Result<()> {
    let target = HSTRING::from(target);
    // SAFETY: all pointers are borrowed for the duration of the call. The
    // return is an `HINSTANCE`-cast error code: anything above 32 is success.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        )
    };
    let code = result.0 as usize;
    if code > 32 {
        Ok(())
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "ShellExecute error codes are at most 32"
        )]
        Err(windows_core::Error::new(
            HRESULT::from_win32(code as u32),
            format!("toast activation could not open the target (ShellExecute error {code})"),
        ))
    }
}
