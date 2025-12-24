mod platform;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Platform error: {0}")]
    Platform(String),
    #[error("Unsupported platform or feature")]
    Unsupported,
    #[error("Monitor not found")]
    MonitorNotFound,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ScreenInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub is_primary: bool,
}

/// Capture the screen content.
/// 
/// On desktop, this captures the specified display.
/// On mobile, this usually captures the current window/view content (snapshot).
/// 
/// Returns PNG encoded bytes.
pub fn capture_screen(display_index: usize) -> Result<Vec<u8>, Error> {
    platform::capture_screen(display_index)
}

/// Get the current brightness level (0.0 - 1.0).
/// 
/// On desktop, this might return the brightness of the primary monitor or the first controllable one.
pub async fn get_brightness() -> Result<f32, Error> {
    platform::get_brightness().await
}

/// Set the brightness level (0.0 - 1.0).
pub async fn set_brightness(val: f32) -> Result<(), Error> {
    platform::set_brightness(val).await
}

/// List available screens.
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    platform::screens()
}

#[cfg(target_os = "android")]
pub fn init(env: &mut jni::JNIEnv, context: &jni::objects::JObject) -> Result<(), Error> {
    platform::init(env, context)
}
