use enumset::EnumSet;
use waterkit_core::Capabilities;

use crate::{Payload, Symbology, VisionError, sys};

/// The one-shot system code scanner.
///
/// Presents the platform's own scanning UI and resolves to the decoded
/// payload and its [`Symbology`], like the workspace's dialog pickers.
/// The caller never sees a camera frame:
///
/// - **Android:** the Google code scanner of Google Play services
///   (`GmsBarcodeScanning`), restricted to the requested symbologies with
///   auto-zoom on. It renders Play services' own screen and needs no
///   camera permission from the app; the app only links the thin
///   `play-services-code-scanner` client this crate declares for the
///   packager, and its scanning module is downloaded to the device by
///   Play services on first use.
/// - **iOS:** `VisionKit`'s `DataScannerViewController`, presented from the
///   app's key window scene, restricted to the requested symbologies.
///   `VisionKit` has no UPC-A symbology: a requested [`Symbology::UpcA`]
///   registers EAN-13, and a result is reported as [`Symbology::UpcA`]
///   when UPC-A was requested and the EAN-13 payload has the leading 0
///   that makes it a UPC-A.
///
/// There is no system scanner on macOS, Windows, Linux or Android
/// devices without Play services: [`CodeScanner::capabilities`] reports
/// the scanner unavailable there and [`CodeScanner::scan`] fails with
/// [`VisionError::Unsupported`]. A [`CodeScanner`] is never silently
/// served another way; `WaterUI` owns the fallback scanning view.
#[derive(Debug, Clone)]
pub struct CodeScanner {
    symbologies: EnumSet<Symbology>,
}

/// A code returned by [`CodeScanner::scan`].
///
/// The decoded payload and its [`Symbology`]. There is no geometry — the
/// scanner presents its own UI, so no caller-visible frame exists for
/// bounds to be normalized to.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScannedCode {
    pub(crate) symbology: Symbology,
    pub(crate) payload: Payload,
}

impl ScannedCode {
    /// The symbology the payload was decoded as.
    #[must_use]
    pub const fn symbology(&self) -> Symbology {
        self.symbology
    }

    /// The decoded payload.
    #[must_use]
    pub const fn payload(&self) -> &Payload {
        &self.payload
    }
}

impl CodeScanner {
    /// Creates a scanner restricted to `symbologies`.
    ///
    /// # Panics
    ///
    /// Panics when `symbologies` is empty: a scan restricted to nothing
    /// can never return.
    pub fn new(symbologies: impl Into<EnumSet<Symbology>>) -> Self {
        let symbologies = symbologies.into();
        assert!(
            !symbologies.is_empty(),
            "CodeScanner requires at least one symbology"
        );
        Self { symbologies }
    }

    /// The symbologies this scanner accepts.
    #[must_use]
    pub const fn symbologies(&self) -> EnumSet<Symbology> {
        self.symbologies
    }

    /// Probes whether this device can present the system code scanner.
    ///
    /// Reports the scanner available on Android only when Google Play
    /// services is usable on the device, and on iOS only when
    /// `DataScannerViewController` is supported; the simulator and all
    /// other platforms report unavailable. The returned
    /// [`ScannerCapabilities::symbologies`] is the set this platform's
    /// scanner can express: every [`Symbology`] on iOS, the Google code
    /// scanner's format list on Android, and the empty set where no
    /// system scanner exists. An unavailable scanner means
    /// [`CodeScanner::scan`] fails: the fallback scanning UI lives in
    /// `WaterUI`, not here.
    ///
    /// # Panics
    ///
    /// On Android, panics if the application `Context` has not been
    /// published to `ndk_context` yet or the JNI probe fails.
    #[must_use]
    #[cfg_attr(
        not(any(target_os = "ios", target_os = "android")),
        expect(
            clippy::missing_const_for_fn,
            reason = "the iOS and Android availability probes are runtime calls; on other platforms the probe is a constant and clippy suggests const"
        )
    )]
    pub fn capabilities() -> ScannerCapabilities {
        ScannerCapabilities {
            available: sys::scanner_available(),
            symbologies: sys::scanner_symbologies(),
        }
    }

    /// Presents the system scanner and resolves to the scanned code, or
    /// `Ok(None)` when the user cancels, like the dialog pickers.
    ///
    /// # Errors
    ///
    /// Returns [`VisionError::Unsupported`] before presenting anything
    /// when the requested symbologies include ones this platform's
    /// scanner cannot express — the error names them — and when this
    /// device has no system code scanner at all, the check
    /// [`CodeScanner::capabilities`] performs. Returns
    /// [`VisionError::Platform`] when a supported scanner fails while
    /// presenting or decoding.
    pub async fn scan(self) -> Result<Option<ScannedCode>, VisionError> {
        let inexpressible = self.symbologies - sys::scanner_symbologies();
        if !inexpressible.is_empty() {
            let names = inexpressible
                .iter()
                .map(|symbology| format!("{symbology:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(VisionError::Unsupported(format!(
                "this platform's code scanner cannot express: {names}"
            )));
        }
        sys::scan(self.symbologies).await
    }
}

/// Capability probe for the system code scanner, returned by
/// [`CodeScanner::capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScannerCapabilities {
    /// Whether the device can present the system code scanner.
    pub available: bool,
    /// The symbologies this platform's scanner can express.
    pub symbologies: EnumSet<Symbology>,
}

impl Capabilities for ScannerCapabilities {
    fn available(&self) -> bool {
        self.available
    }
}
