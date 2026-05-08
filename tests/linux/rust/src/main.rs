#[cfg(target_os = "linux")]
use waterkit_permission::Permission;

#[cfg(target_os = "linux")]
fn main() {
    std::mem::drop(waterkit_permission::check(Permission::Location));
    std::mem::drop(waterkit_biometric::capabilities());
    let _ = waterkit_haptic::Haptic::capabilities();
    std::mem::drop(waterkit_bluetooth::adapter_state());
    let _ = waterkit_nfc::is_available();
    let _ = waterkit_share::ShareSheet::text("waterkit");
    let _ = waterkit_speech::SpeechRecognizer::is_available();
    let _ = waterkit_deeplink::DeepLink::parse("https://example.com");
    let _ = waterkit_sensor::Accelerometer::is_available();
    let _ = waterkit_system::connectivity();
    let _ = waterkit_system::thermal_state();
    let _ = waterkit_system::load();
    std::mem::drop(waterkit_passkey::is_available());
}

#[cfg(not(target_os = "linux"))]
const fn main() {}
