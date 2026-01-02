//! Android platform implementation.
//!
//! Screen capture on Android requires `MediaProjection` API with Activity/permission flow,
//! which is not yet implemented.

use crate::frame::ScreenFrame;
use crate::stream::StreamConfig;
use crate::{Error, ScreenInfo};
use std::sync::{Arc, OnceLock};
use wgpu::{Device, Queue};

static CONTEXT: OnceLock<jni::objects::GlobalRef> = OnceLock::new();

/// Initialize the screen module with Android context.
pub fn init(env: &mut jni::JNIEnv, context: &jni::objects::JObject) -> Result<(), Error> {
    let global = env
        .new_global_ref(context)
        .map_err(|e| Error::Platform(e.to_string()))?;
    CONTEXT
        .set(global)
        .map_err(|_| Error::Platform("Already initialized".into()))?;
    Ok(())
}

/// Enumerate screens (returns single dummy screen).
#[allow(clippy::unnecessary_wraps)]
pub fn screens() -> Result<Vec<ScreenInfo>, Error> {
    Ok(vec![ScreenInfo {
        id: 0,
        name: "Main Screen".into(),
        width: 0,
        height: 0,
        scale_factor: 1.0,
        is_primary: true,
    }])
}

/// Get screen brightness.
#[allow(clippy::unused_async)]
pub async fn get_brightness() -> Result<f32, Error> {
    let Some(context) = CONTEXT.get() else {
        return Err(Error::Platform("Not initialized".into()));
    };

    // TODO: Use Settings.System.getInt for SCREEN_BRIGHTNESS
    let _ = context;
    Ok(1.0)
}

/// Set screen brightness.
#[allow(clippy::unused_async)]
pub async fn set_brightness(_val: f32) -> Result<(), Error> {
    let Some(_context) = CONTEXT.get() else {
        return Err(Error::Platform("Not initialized".into()));
    };

    // TODO: Use Settings.System.putInt for SCREEN_BRIGHTNESS
    Ok(())
}

/// Screen stream (not supported on Android yet).
pub struct ScreenStreamInner;

impl ScreenStreamInner {
    pub fn new(
        _display: &ScreenInfo,
        _device: Arc<Device>,
        _queue: Arc<Queue>,
        _config: &StreamConfig,
    ) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }

    #[allow(clippy::unused_async)]
    pub async fn next_frame(&self) -> Option<ScreenFrame> {
        self.try_next_frame()
    }

    #[allow(clippy::unused_self)]
    pub const fn try_next_frame(&self) -> Option<ScreenFrame> {
        None
    }

    #[allow(clippy::unused_self)]
    pub const fn dimensions(&self) -> (u32, u32) {
        (0, 0)
    }
}
