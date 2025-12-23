use crate::{BiometricError, BiometricType};
use windows::Foundation::IAsyncOperation;
use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability, UserConsentVerificationResult};

pub async fn is_available() -> bool {
    let availability = match UserConsentVerifier::CheckAvailabilityAsync() {
        Ok(op) => match op.await {
            Ok(avail) => avail,
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    availability == UserConsentVerifierAvailability::Available
}

pub async fn get_biometric_type() -> Option<BiometricType> {
    if is_available().await {
        // Windows Hello encompasses Face, Fingerprint, PIN.
        // UserConsentVerifier doesn't easily distinguish precisely between Face/Fingerprint without more complex APIs.
        // Usually it's treated as "Windows Hello" / "Biometrics".
        // We'll return Fingerprint as a generic placeholder or Unknown if we want to be strict.
        // But for now, let's say Unknown because we don't know if it's Face or Finger.
        // Or we could try to guess, but it's not exposed in this API.
        Some(BiometricType::Unknown) 
    } else {
        None
    }
}

pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    if !is_available().await {
        return Err(BiometricError::NotAvailable);
    }
    
    // Convert reason to HSTRING which is handled automatically by windows-rs for &str usually?
    // Actually RequestVerificationAsync takes HSTRING.
    let result = UserConsentVerifier::RequestVerificationAsync(&windows::core::HSTRING::from(reason))
        .map_err(|e| BiometricError::PlatformError(e.to_string()))?
        .await
        .map_err(|e| BiometricError::PlatformError(e.to_string()))?;
        
    match result {
        UserConsentVerificationResult::Verified => Ok(()),
        UserConsentVerificationResult::Canceled => Err(BiometricError::Cancelled),
        UserConsentVerificationResult::DeviceBusy => Err(BiometricError::Failed("Device busy".into())),
        UserConsentVerificationResult::RetriesExhausted => Err(BiometricError::Failed("Retries exhausted".into())),
        UserConsentVerificationResult::DisabledByPolicy => Err(BiometricError::NotAvailable), // Or failed
        _ => Err(BiometricError::Failed("Verification failed".into())),
    }
}
