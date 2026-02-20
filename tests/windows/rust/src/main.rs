#![allow(missing_docs)]

#[cfg(target_os = "windows")]
use waterkit_permission::Permission;

#[cfg(target_os = "windows")]
fn main() {
    std::mem::drop(waterkit_permission::check(Permission::Location));
    std::mem::drop(waterkit_biometric::is_available());
    let _ = waterkit_haptic::Haptic::is_available();
    std::mem::drop(waterkit_bluetooth::adapter_state());
    let _ = waterkit_nfc::is_available();
    let _ = waterkit_share::ShareSheet::text("waterkit");
    let _ = waterkit_speech::SpeechRecognizer::is_available();
    let _ = waterkit_deeplink::DeepLink::parse("https://example.com");
    let _ = waterkit_sensor::Accelerometer::is_available();
    let _ = waterkit_system::get_connectivity_info();
    let _ = waterkit_system::get_thermal_state();
    let _ = waterkit_system::get_system_load();
    std::mem::drop(waterkit_passkey::is_available());
}

#[cfg(not(target_os = "windows"))]
const fn main() {}
