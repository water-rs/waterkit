//! Windows compile-time smoke test for the `WaterKit` platform surface.

#[cfg(target_os = "windows")]
use waterkit_permission::Permission;

#[cfg(target_os = "windows")]
fn main() {
    std::mem::drop(waterkit_permission::check(Permission::Location));
    std::mem::drop(waterkit_biometric::capabilities());
    let _ = waterkit_haptic::Haptic::capabilities();
    std::mem::drop(waterkit_bluetooth::adapter_state());
    let _ = waterkit_nfc::is_available();
    let _ = waterkit_share::ShareSheet::text("waterkit");
    let _ = waterkit_speech::SpeechRecognizer::capabilities();
    let _ = waterkit_deeplink::DeepLink::parse("https://example.com");
    let _ = waterkit_sensor::Accelerometer::capabilities().available;
    let _ = futures::executor::block_on(waterkit_store::capabilities());
    let _ = futures::executor::block_on(waterkit_store::Store::connect(
        waterkit_store::Catalog::new(),
    ));
    let _ = futures::executor::block_on(waterkit_system::connectivity());
    let _ = waterkit_system::thermal_state();
    let _ = futures::executor::block_on(waterkit_system::load());
    std::mem::drop(waterkit_passkey::is_available());
}

#[cfg(not(target_os = "windows"))]
const fn main() {}

/// The `store` case: the harness binary is never packaged for the
/// Microsoft Store, so the capability probe must report purchases
/// unavailable and `Store::connect` must fail with `Unavailable` — the
/// same contract the Android case asserts on the Play-less emulator.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    #[test]
    fn store_reports_unavailable_unpackaged() {
        let capabilities = futures::executor::block_on(waterkit_store::capabilities()).unwrap();
        assert!(!capabilities.purchases);
        let result = futures::executor::block_on(waterkit_store::Store::connect(
            waterkit_store::Catalog::new(),
        ));
        assert!(matches!(
            result,
            Err(waterkit_store::StoreError::Unavailable)
        ));
    }
}
