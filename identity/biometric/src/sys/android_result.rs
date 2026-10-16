//! Conversion of Android `BiometricPrompt` outcomes into [`BiometricError`].
//!
//! `BiometricHelper.kt` forwards the framework's `BIOMETRIC_ERROR_*` code with
//! every outcome so cancellations are distinguishable from genuine
//! authentication failures. The mapping is compiled on host targets for tests.

use crate::BiometricError;

/// `android.hardware.biometrics.BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE`.
const BIOMETRIC_ERROR_HW_UNAVAILABLE: i32 = 1;
/// `android.hardware.biometrics.BiometricPrompt.BIOMETRIC_ERROR_CANCELED`.
const BIOMETRIC_ERROR_CANCELED: i32 = 5;
/// `android.hardware.biometrics.BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED`.
const BIOMETRIC_ERROR_USER_CANCELED: i32 = 10;
/// `android.hardware.biometrics.BiometricPrompt.BIOMETRIC_ERROR_NO_BIOMETRICS`.
const BIOMETRIC_ERROR_NO_BIOMETRICS: i32 = 11;
/// `android.hardware.biometrics.BiometricPrompt.BIOMETRIC_ERROR_HW_NOT_PRESENT`.
const BIOMETRIC_ERROR_HW_NOT_PRESENT: i32 = 12;

/// Converts a `BiometricHelper.onResult` delivery into the public result.
///
/// `error_code` is the `BIOMETRIC_ERROR_*` constant reported by
/// `BiometricPrompt.onAuthenticationError` (negative values are reserved for
/// errors raised outside the prompt, such as an unsupported API level).
pub fn authenticate_result(
    success: bool,
    error_code: i32,
    message: Option<String>,
) -> Result<(), BiometricError> {
    if success {
        return Ok(());
    }
    Err(match error_code {
        BIOMETRIC_ERROR_HW_UNAVAILABLE
        | BIOMETRIC_ERROR_NO_BIOMETRICS
        | BIOMETRIC_ERROR_HW_NOT_PRESENT => BiometricError::NotAvailable,
        BIOMETRIC_ERROR_CANCELED | BIOMETRIC_ERROR_USER_CANCELED => BiometricError::Cancelled,
        _ => BiometricError::Failed(
            message.unwrap_or_else(|| String::from("unknown biometric error")),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BIOMETRIC_ERROR_CANCELED, BIOMETRIC_ERROR_HW_NOT_PRESENT, BIOMETRIC_ERROR_HW_UNAVAILABLE,
        BIOMETRIC_ERROR_NO_BIOMETRICS, BIOMETRIC_ERROR_USER_CANCELED, BiometricError,
        authenticate_result,
    };

    #[test]
    fn successful_authentication_maps_to_ok() {
        assert!(authenticate_result(true, 0, None).is_ok());
    }

    #[test]
    fn unavailable_codes_map_to_not_available() {
        for code in [
            BIOMETRIC_ERROR_HW_UNAVAILABLE,
            BIOMETRIC_ERROR_NO_BIOMETRICS,
            BIOMETRIC_ERROR_HW_NOT_PRESENT,
        ] {
            let result = authenticate_result(false, code, Some("unavailable".into()));
            assert!(
                matches!(result, Err(BiometricError::NotAvailable)),
                "error code {code} should map to NotAvailable, got {result:?}"
            );
        }
    }

    #[test]
    fn cancellation_codes_map_to_cancelled() {
        for code in [BIOMETRIC_ERROR_CANCELED, BIOMETRIC_ERROR_USER_CANCELED] {
            let result = authenticate_result(false, code, Some("cancelled".into()));
            assert!(
                matches!(result, Err(BiometricError::Cancelled)),
                "error code {code} should map to Cancelled, got {result:?}"
            );
        }
    }

    #[test]
    fn platform_error_maps_to_failed_with_message() {
        // BIOMETRIC_ERROR_LOCKOUT = 7
        let result = authenticate_result(false, 7, Some("too many attempts".into()));
        assert!(
            matches!(result, Err(BiometricError::Failed(ref message)) if message == "too many attempts"),
            "expected Failed(\"too many attempts\"), got {result:?}"
        );
    }

    #[test]
    fn out_of_prompt_error_maps_to_failed() {
        let result = authenticate_result(false, -1, Some("unsupported".into()));
        assert!(matches!(result, Err(BiometricError::Failed(_))));
    }
}
