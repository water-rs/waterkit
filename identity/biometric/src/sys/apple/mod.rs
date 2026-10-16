//! Apple platform (iOS/macOS) biometric implementation backed by
//! `LocalAuthentication`.
//!
//! `LAContext` calls run wherever the caller happens to be: the framework's
//! evaluation reply executes on a private queue inside the framework, so the
//! reply block only holds `Send` state (the oneshot sender resolving the
//! future and the context, which must outlive the evaluation or the
//! framework cancels it).

use std::sync::Mutex;

use block2::RcBlock;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LABiometryType, LAContext, LAPolicy};

use crate::{BiometricError, BiometricType};

/// Check if biometrics are available on Apple platforms.
#[expect(
    clippy::unused_async,
    reason = "the sys interface is async on every platform"
)]
pub async fn is_available() -> bool {
    // SAFETY: `new` is `[[LAContext alloc] init]`; the context is a plain
    // query object usable on any thread.
    let context = unsafe { LAContext::new() };
    // SAFETY: synchronous policy check on a live context; the returned
    // error's content is not inspected, only whether evaluation is possible.
    unsafe { context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics) }
        .is_ok()
}

/// Get the biometric type on Apple platforms.
#[expect(
    clippy::unused_async,
    reason = "the sys interface is async on every platform"
)]
pub async fn get_biometric_type() -> Option<BiometricType> {
    // SAFETY: `new` is `[[LAContext alloc] init]`; `biometryType` is a
    // read-only property queried on a live context.
    let context = unsafe { LAContext::new() };
    // SAFETY: see `is_available`; the `Err` case means no biometric type.
    unsafe {
        if context
            .canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
            .is_err()
        {
            return None;
        }
        match context.biometryType() {
            LABiometryType::TouchID => Some(BiometricType::Fingerprint),
            LABiometryType::FaceID => Some(BiometricType::Face),
            LABiometryType::OpticID => Some(BiometricType::Iris),
            _ => None,
        }
    }
}

/// Perform biometric authentication on Apple platforms.
///
/// # Errors
/// Returns `BiometricError::NotAvailable` if biometrics are not ready,
/// `BiometricError::Failed` with the framework's localized description when
/// evaluation fails, or `BiometricError::Platform` if the callback is lost.
pub async fn authenticate(reason: &str) -> Result<(), BiometricError> {
    if !is_available().await {
        return Err(BiometricError::NotAvailable);
    }

    let (sender, receiver) = oneshot::channel();
    {
        // The reply is an `Fn` block, so the one-shot sender it resolves is
        // taken through a mutex; `evaluatePolicy` invokes it exactly once.
        let sender = Mutex::new(Some(sender));
        // SAFETY: `new` is `[[LAContext alloc] init]`. The reply block holds
        // a retain of the context so it stays alive for the whole
        // evaluation; the framework cancels a running evaluation when its
        // context deallocates.
        let context = unsafe { LAContext::new() };
        let reply = {
            let context = context.clone();
            RcBlock::new(move |success: Bool, error: *mut NSError| {
                // The capture keeps the context alive until the reply runs.
                let _context = &context;
                let Some(sender) = sender.lock().expect("reply sender lock poisoned").take() else {
                    return;
                };
                // SAFETY: LocalAuthentication reports `error` only when
                // `success` is false; a non-null pointer references a live
                // NSError valid for the duration of the reply, which
                // `retain` turns into an owned one.
                let message = unsafe { Retained::retain(error) }.map_or_else(
                    || "Unknown error".to_owned(),
                    |error| error.localizedDescription().to_string(),
                );
                let _ = sender.send(if success.is_true() {
                    Ok(())
                } else {
                    Err(BiometricError::Failed(message))
                });
            })
        };

        // SAFETY: `evaluatePolicy:localizedReason:reply:` runs its reply on
        // a private queue inside the framework. The block's captures are
        // all `Send` (the mutex'd oneshot sender and `Retained<LAContext>`),
        // so running it off this thread is sound; LocalAuthentication
        // itself presents the prompt, so no main-thread hop is needed.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
                &NSString::from_str(reason),
                &reply,
            );
        }
        // The framework copied the block for the pending evaluation; the
        // locals drop here so nothing `!Send` crosses the await below.
    }

    receiver
        .await
        .map_err(|_| BiometricError::Platform("authentication callback dropped".into()))?
}
