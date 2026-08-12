//! Android camera implementation using Camera2 + `MediaRecorder` via JNI/Kotlin bridge.

use crate::{
    CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo, DynamicRangeProfile,
    ExposureMode, FlashMode, FocusMode, Frame, Photo, PixelFormat, RawPhoto, RawPhotoFormat,
    RawVideoFormat, Resolution, StabilizationMode,
};
use jni::objects::{
    Global, JByteArray, JFloatArray, JIntArray, JObject, JObjectArray, JString, JValue,
};
use jni::strings::JNIStr;
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use waterkit_build::{AndroidError, DexHelper, dex_helper, jvm_and_context};

/// `waterkit.camera.CameraHelper`, embedded as a DEX by this crate's build
/// script and loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.camera.CameraHelper");

impl From<AndroidError> for CameraError {
    fn from(error: AndroidError) -> Self {
        Self::PlatformError(error.to_string())
    }
}

const DYNAMIC_RANGE_SDR: i32 = 0;
const DYNAMIC_RANGE_HDR10: i32 = 1;
const DYNAMIC_RANGE_HLG10: i32 = 2;
const DYNAMIC_RANGE_DOLBY_VISION: i32 = 3;

const FLASH_OFF: i32 = 0;
const FLASH_ON: i32 = 1;
const FLASH_AUTO: i32 = 2;
const FLASH_TORCH: i32 = 3;

const STABILIZATION_OFF: i32 = 0;
const STABILIZATION_STANDARD: i32 = 1;
const STABILIZATION_CINEMATIC: i32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingMode {
    Standard,
    Raw,
}

#[derive(Debug)]
struct RawFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: Duration,
}

#[derive(Debug)]
struct AndroidBridge {
    vm: JavaVM,
    helper: Global<JObject<'static>>,
}

impl AndroidBridge {
    fn new() -> Result<Self, CameraError> {
        let (vm, context) = jvm_and_context()?;

        let helper = vm
            .attach_current_thread(
                |env| -> Result<Result<Global<JObject<'static>>, CameraError>, jni::errors::Error> {
                    Ok(Self::create_helper(env, context.as_obj()))
                },
            )
            .map_err(|error| {
                CameraError::PlatformError(format!("attach_current_thread: {error}"))
            })??;

        Ok(Self { vm, helper })
    }

    /// Loads `CameraHelper` from the embedded DEX and constructs one instance
    /// bound to the application context.
    /// Loads `CameraHelper` and constructs one instance bound to the
    /// application context.
    fn create_helper(
        env: &mut Env<'_>,
        context: &JObject<'_>,
    ) -> Result<Global<JObject<'static>>, CameraError> {
        let helper_class = HELPER.class(env, context)?;

        let helper_obj = env
            .new_object(
                helper_class,
                jni_sig!("(Landroid/content/Context;)V"),
                &[JValue::Object(context)],
            )
            .map_err(|error| {
                CameraError::PlatformError(format!("new CameraHelper instance: {error}"))
            })?;

        env.new_global_ref(helper_obj)
            .map_err(|error| CameraError::PlatformError(format!("new_global_ref(helper): {error}")))
    }

    fn with_env<T, F>(&self, f: F) -> Result<T, CameraError>
    where
        F: FnOnce(&mut Env<'_>) -> Result<T, CameraError>,
    {
        self.vm
            .attach_current_thread(
                |env| -> Result<Result<T, CameraError>, jni::errors::Error> { Ok(f(env)) },
            )
            .map_err(|error| {
                CameraError::PlatformError(format!("attach_current_thread: {error}"))
            })?
    }

    fn read_java_string(
        env: &Env<'_>,
        object: &JObject<'_>,
        context: &str,
    ) -> Result<String, CameraError> {
        env.as_cast::<JString>(object)
            .and_then(|text| text.try_to_string(env))
            .map_err(|error| CameraError::PlatformError(format!("get_string({context}): {error}")))
    }

    fn frame_size_internal(&self, env: &mut Env<'_>) -> Result<Resolution, CameraError> {
        let dims_obj = env
            .call_method(
                self.helper.as_obj(),
                jni_str!("getFrameSize"),
                jni_sig!("()[I"),
                &[],
            )
            .and_then(jni::objects::JValueOwned::l)
            .map_err(|error| CameraError::PlatformError(format!("getFrameSize: {error}")))?;

        if dims_obj.is_null() {
            return Err(CameraError::PlatformError(
                "getFrameSize returned null".into(),
            ));
        }

        let dims_array = env.cast_local::<JIntArray>(dims_obj).map_err(|error| {
            CameraError::PlatformError(format!("getFrameSize is not an int array: {error}"))
        })?;
        let mut dims = [0_i32; 2];
        dims_array.get_region(env, 0, &mut dims).map_err(|error| {
            CameraError::PlatformError(format!("get_region(frame size): {error}"))
        })?;

        let width = u32::try_from(dims[0])
            .map_err(|_| CameraError::PlatformError(format!("invalid frame width: {}", dims[0])))?;
        let height = u32::try_from(dims[1]).map_err(|_| {
            CameraError::PlatformError(format!("invalid frame height: {}", dims[1]))
        })?;

        if width == 0 || height == 0 {
            return Err(CameraError::PlatformError(
                "frame dimensions must be non-zero".into(),
            ));
        }

        Ok(Resolution { width, height })
    }

    fn call_bool_with_camera(&self, method: &JNIStr, camera_id: &str) -> Result<bool, CameraError> {
        self.with_env(|env| {
            let camera_id_java = env.new_string(camera_id).map_err(|error| {
                CameraError::PlatformError(format!("new_string(camera_id): {error}"))
            })?;

            env.call_method(
                self.helper.as_obj(),
                method,
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&camera_id_java)],
            )
            .and_then(jni::objects::JValueOwned::z)
            .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn call_int_with_camera(&self, method: &JNIStr, camera_id: &str) -> Result<i32, CameraError> {
        self.with_env(|env| {
            let camera_id_java = env.new_string(camera_id).map_err(|error| {
                CameraError::PlatformError(format!("new_string(camera_id): {error}"))
            })?;

            env.call_method(
                self.helper.as_obj(),
                method,
                jni_sig!("(Ljava/lang/String;)I"),
                &[JValue::Object(&camera_id_java)],
            )
            .and_then(jni::objects::JValueOwned::i)
            .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn call_int_array_with_camera(
        &self,
        method: &JNIStr,
        camera_id: &str,
    ) -> Result<Vec<i32>, CameraError> {
        self.with_env(|env| {
            let camera_id_java = env.new_string(camera_id).map_err(|error| {
                CameraError::PlatformError(format!("new_string(camera_id): {error}"))
            })?;

            let arr_obj = env
                .call_method(
                    self.helper.as_obj(),
                    method,
                    jni_sig!("(Ljava/lang/String;)[I"),
                    &[JValue::Object(&camera_id_java)],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))?;

            if arr_obj.is_null() {
                return Ok(Vec::new());
            }

            let arr = env.cast_local::<JIntArray>(arr_obj).map_err(|error| {
                CameraError::PlatformError(format!("{method} is not an int array: {error}"))
            })?;
            let len = arr
                .len(env)
                .map_err(|error| CameraError::PlatformError(format!("{method} length: {error}")))?;
            let mut out = vec![0_i32; len];
            arr.get_region(env, 0, &mut out)
                .map_err(|error| CameraError::PlatformError(format!("{method} read: {error}")))?;
            Ok(out)
        })
    }

    fn call_float_array_with_camera(
        &self,
        method: &JNIStr,
        camera_id: &str,
    ) -> Result<Vec<f32>, CameraError> {
        self.with_env(|env| {
            let camera_id_java = env.new_string(camera_id).map_err(|error| {
                CameraError::PlatformError(format!("new_string(camera_id): {error}"))
            })?;

            let arr_obj = env
                .call_method(
                    self.helper.as_obj(),
                    method,
                    jni_sig!("(Ljava/lang/String;)[F"),
                    &[JValue::Object(&camera_id_java)],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))?;

            if arr_obj.is_null() {
                return Ok(Vec::new());
            }

            let arr = env.cast_local::<JFloatArray>(arr_obj).map_err(|error| {
                CameraError::PlatformError(format!("{method} is not a float array: {error}"))
            })?;
            let len = arr
                .len(env)
                .map_err(|error| CameraError::PlatformError(format!("{method} length: {error}")))?;
            let mut out = vec![0_f32; len];
            arr.get_region(env, 0, &mut out)
                .map_err(|error| CameraError::PlatformError(format!("{method} read: {error}")))?;
            Ok(out)
        })
    }

    fn list_cameras(&self) -> Result<Vec<CameraInfo>, CameraError> {
        self.with_env(|env| {
            let rows_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("listCameras"),
                    jni_sig!("()[[Ljava/lang/String;"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::EnumerationFailed(format!("listCameras JNI call: {error}"))
                })?;

            if rows_obj.is_null() {
                return Ok(Vec::new());
            }

            let rows = env.cast_local::<JObjectArray>(rows_obj).map_err(|error| {
                CameraError::EnumerationFailed(format!("listCameras is not an array: {error}"))
            })?;
            let row_count = rows.len(env).map_err(|error| {
                CameraError::EnumerationFailed(format!("listCameras length: {error}"))
            })?;

            let mut cameras = Vec::with_capacity(row_count);
            for index in 0..row_count {
                let camera_row_obj = rows.get_element(env, index).map_err(|error| {
                    CameraError::EnumerationFailed(format!("listCameras row {index}: {error}"))
                })?;

                if camera_row_obj.is_null() {
                    return Err(CameraError::EnumerationFailed(format!(
                        "listCameras row {index} is null"
                    )));
                }

                let camera_row =
                    env.cast_local::<JObjectArray>(camera_row_obj)
                        .map_err(|error| {
                            CameraError::EnumerationFailed(format!(
                                "listCameras row {index} is not an array: {error}"
                            ))
                        })?;
                let column_count = camera_row.len(env).map_err(|error| {
                    CameraError::EnumerationFailed(format!(
                        "listCameras row {index} length: {error}"
                    ))
                })?;
                if column_count < 3 {
                    return Err(CameraError::EnumerationFailed(format!(
                        "listCameras row {index} has {column_count} columns, expected at least 3"
                    )));
                }

                let id_obj = camera_row.get_element(env, 0).map_err(|error| {
                    CameraError::EnumerationFailed(format!("camera id row {index}: {error}"))
                })?;
                let name_obj = camera_row.get_element(env, 1).map_err(|error| {
                    CameraError::EnumerationFailed(format!("camera name row {index}: {error}"))
                })?;
                let is_front_obj = camera_row.get_element(env, 2).map_err(|error| {
                    CameraError::EnumerationFailed(format!("camera facing row {index}: {error}"))
                })?;

                if id_obj.is_null() || name_obj.is_null() || is_front_obj.is_null() {
                    return Err(CameraError::EnumerationFailed(format!(
                        "listCameras row {index} contains null field"
                    )));
                }

                let id = Self::read_java_string(env, &id_obj, "camera id")?;
                let name = Self::read_java_string(env, &name_obj, "camera name")?;
                let is_front_raw = Self::read_java_string(env, &is_front_obj, "camera facing")?;

                let is_front_facing = match is_front_raw.as_str() {
                    "true" | "True" | "TRUE" => true,
                    "false" | "False" | "FALSE" => false,
                    _ => {
                        return Err(CameraError::EnumerationFailed(format!(
                            "invalid is_front_facing value `{is_front_raw}` for camera {id}"
                        )));
                    }
                };

                cameras.push(CameraInfo {
                    id,
                    name,
                    description: None,
                    is_front_facing,
                });
            }

            Ok(cameras)
        })
    }

    fn get_supported_resolutions(&self, camera_id: &str) -> Result<Vec<Resolution>, CameraError> {
        let flat =
            self.call_int_array_with_camera(jni_str!("getSupportedResolutions"), camera_id)?;
        if flat.len() % 2 != 0 {
            return Err(CameraError::PlatformError(format!(
                "getSupportedResolutions returned odd array length: {}",
                flat.len()
            )));
        }

        let mut out = Vec::with_capacity(flat.len() / 2);
        for chunk in flat.as_chunks::<2>().0 {
            let width = u32::try_from(chunk[0]).map_err(|_| {
                CameraError::PlatformError(format!(
                    "resolution width must be positive, got {}",
                    chunk[0]
                ))
            })?;
            let height = u32::try_from(chunk[1]).map_err(|_| {
                CameraError::PlatformError(format!(
                    "resolution height must be positive, got {}",
                    chunk[1]
                ))
            })?;
            if width == 0 || height == 0 {
                return Err(CameraError::PlatformError(format!(
                    "resolution dimensions must be non-zero, got {width}x{height}"
                )));
            }
            out.push(Resolution { width, height });
        }

        Ok(out)
    }

    fn get_supported_frame_rates(&self, camera_id: &str) -> Result<Vec<u32>, CameraError> {
        let rates =
            self.call_int_array_with_camera(jni_str!("getSupportedFrameRates"), camera_id)?;
        let mut out = Vec::with_capacity(rates.len());
        for fps in rates {
            let value = u32::try_from(fps).map_err(|_| {
                CameraError::PlatformError(format!("frame rate must be positive, got {fps}"))
            })?;
            if value == 0 {
                return Err(CameraError::PlatformError(
                    "frame rate must be non-zero".into(),
                ));
            }
            out.push(value);
        }
        Ok(out)
    }

    fn get_zoom_range(&self, camera_id: &str) -> Result<Option<(f32, f32)>, CameraError> {
        let range = self.call_float_array_with_camera(jni_str!("getZoomRange"), camera_id)?;
        if range.is_empty() {
            return Ok(None);
        }
        if range.len() < 2 {
            return Err(CameraError::PlatformError(format!(
                "getZoomRange returned {} elements, expected at least 2",
                range.len()
            )));
        }

        let min = range[0];
        let max = range[1];
        if !(min.is_finite() && max.is_finite()) || min <= 0.0 || max < min {
            return Err(CameraError::PlatformError(format!(
                "invalid zoom range [{min}, {max}]"
            )));
        }
        Ok(Some((min, max)))
    }

    fn query_capabilities(
        &self,
        camera_id: &str,
        requested_resolution: Resolution,
        requested_frame_rate: u32,
    ) -> Result<CameraCapabilities, CameraError> {
        let mut resolutions = self.get_supported_resolutions(camera_id)?;
        if resolutions.is_empty() {
            resolutions.push(requested_resolution);
        }
        if !resolutions.contains(&requested_resolution) {
            resolutions.push(requested_resolution);
        }

        let mut frame_rates = self.get_supported_frame_rates(camera_id)?;
        if frame_rates.is_empty() {
            frame_rates.push(requested_frame_rate.max(1));
        }
        frame_rates.sort_unstable();
        frame_rates.dedup();

        let supports_hdr = self.call_bool_with_camera(jni_str!("supportsHdr"), camera_id)?;
        let supports_dolby_vision =
            self.call_bool_with_camera(jni_str!("supportsDolbyVision"), camera_id)?;
        let supports_standard_stabilization =
            self.call_bool_with_camera(jni_str!("supportsStandardStabilization"), camera_id)?;
        let supports_cinematic_stabilization =
            self.call_bool_with_camera(jni_str!("supportsCinematicStabilization"), camera_id)?;
        let supports_exposure_compensation =
            self.call_bool_with_camera(jni_str!("supportsExposureCompensation"), camera_id)?;
        let supports_manual_focus =
            self.call_bool_with_camera(jni_str!("supportsManualFocus"), camera_id)?;
        let supports_manual_white_balance =
            self.call_bool_with_camera(jni_str!("supportsManualWhiteBalance"), camera_id)?;
        let has_flash = self.call_bool_with_camera(jni_str!("hasFlash"), camera_id)?;
        let has_torch = self.call_bool_with_camera(jni_str!("hasTorch"), camera_id)?;
        let supports_raw_photo =
            self.call_bool_with_camera(jni_str!("supportsRawPhoto"), camera_id)?;
        let supports_raw_video =
            self.call_bool_with_camera(jni_str!("supportsRawVideo"), camera_id)?;
        let supports_concurrent_multi_camera =
            self.call_bool_with_camera(jni_str!("supportsConcurrentMultiCamera"), camera_id)?;
        let max_concurrent_raw =
            self.call_int_with_camera(jni_str!("maxConcurrentCameras"), camera_id)?;
        let max_concurrent_u8 = u8::try_from(max_concurrent_raw).map_err(|_| {
            CameraError::PlatformError(format!(
                "maxConcurrentCameras must fit in u8, got {max_concurrent_raw}"
            ))
        })?;
        let max_concurrent = NonZeroU8::new(max_concurrent_u8).ok_or_else(|| {
            CameraError::PlatformError(format!(
                "maxConcurrentCameras must be >= 1, got {max_concurrent_raw}"
            ))
        })?;

        let mut dynamic_ranges = vec![DynamicRangeProfile::Sdr];
        if supports_hdr {
            dynamic_ranges.push(DynamicRangeProfile::Hdr10);
            dynamic_ranges.push(DynamicRangeProfile::Hlg10);
        }
        if supports_dolby_vision {
            dynamic_ranges.push(DynamicRangeProfile::DolbyVision);
        }

        let mut stabilization_modes = vec![StabilizationMode::Off];
        if supports_standard_stabilization {
            stabilization_modes.push(StabilizationMode::Standard);
        }
        if supports_cinematic_stabilization {
            stabilization_modes.push(StabilizationMode::Cinematic);
        }

        Ok(CameraCapabilities {
            resolutions,
            frame_rates,
            iso_range: None,
            exposure_duration_range: None,
            supports_exposure_compensation,
            supports_manual_focus,
            supports_manual_white_balance,
            zoom_range: self.get_zoom_range(camera_id)?,
            dynamic_ranges,
            supports_dolby_vision,
            stabilization_modes,
            has_flash,
            has_torch,
            supports_concurrent_multi_camera,
            max_concurrent_cameras: max_concurrent,
            uses_system_photo_pipeline: true,
            uses_system_video_pipeline: true,
            supports_raw_photo,
            raw_photo_formats: if supports_raw_photo {
                vec![RawPhotoFormat::Dng]
            } else {
                Vec::new()
            },
            supports_raw_video,
            raw_video_formats: if supports_raw_video {
                vec![RawVideoFormat::Rgba8Frames]
            } else {
                Vec::new()
            },
        })
    }

    fn open_camera(
        &self,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
    ) -> Result<(), CameraError> {
        self.with_env(|env| {
            let camera_id_java = env.new_string(camera_id).map_err(|error| {
                CameraError::OpenFailed(format!("new_string(camera id): {error}"))
            })?;

            let width = i32::try_from(resolution.width)
                .map_err(|_| CameraError::OpenFailed("camera width exceeds i32".into()))?;
            let height = i32::try_from(resolution.height)
                .map_err(|_| CameraError::OpenFailed("camera height exceeds i32".into()))?;
            let fps = i32::try_from(frame_rate.max(1))
                .map_err(|_| CameraError::OpenFailed("camera frame rate exceeds i32".into()))?;

            let opened = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("openCamera"),
                    jni_sig!("(Ljava/lang/String;III)Z"),
                    &[
                        JValue::Object(&camera_id_java),
                        JValue::Int(width),
                        JValue::Int(height),
                        JValue::Int(fps),
                    ],
                )
                .and_then(jni::objects::JValueOwned::z)
                .map_err(|error| {
                    CameraError::OpenFailed(format!("openCamera JNI call: {error}"))
                })?;

            if opened {
                Ok(())
            } else {
                Err(CameraError::OpenFailed(format!(
                    "openCamera returned false for `{camera_id}`"
                )))
            }
        })
    }

    fn start_capture(&self) -> Result<(), CameraError> {
        self.with_env(|env| {
            let started = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("startCapture"),
                    jni_sig!("()Z"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::z)
                .map_err(|error| {
                    CameraError::StartFailed(format!("startCapture JNI call: {error}"))
                })?;

            if started {
                Ok(())
            } else {
                Err(CameraError::StartFailed(
                    "startCapture returned false".into(),
                ))
            }
        })
    }

    fn stop_capture(&self) -> Result<(), CameraError> {
        self.with_env(|env| {
            env.call_method(
                self.helper.as_obj(),
                jni_str!("stopCapture"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| CameraError::PlatformError(format!("stopCapture: {error}")))?;
            Ok(())
        })
    }

    fn close_camera(&self) -> Result<(), CameraError> {
        self.with_env(|env| {
            env.call_method(
                self.helper.as_obj(),
                jni_str!("closeCamera"),
                jni_sig!("()V"),
                &[],
            )
            .map_err(|error| CameraError::PlatformError(format!("closeCamera: {error}")))?;
            Ok(())
        })
    }

    fn frame_size(&self) -> Result<Resolution, CameraError> {
        self.with_env(|env| self.frame_size_internal(env))
    }

    fn wait_for_frame(
        &self,
        start_instant: Instant,
        timeout_ms: i32,
        expected_size: Resolution,
    ) -> Result<Option<RawFrame>, CameraError> {
        self.with_env(|env| {
            let frame_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("waitForNextFrame"),
                    jni_sig!("(I)[B"),
                    &[JValue::Int(timeout_ms.max(0))],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("waitForNextFrame JNI call: {error}"))
                })?;

            if frame_obj.is_null() {
                return Ok(None);
            }

            let frame_array = env.cast_local::<JByteArray>(frame_obj).map_err(|error| {
                CameraError::CaptureFailed(format!("frame is not a byte array: {error}"))
            })?;
            let data = env.convert_byte_array(&frame_array).map_err(|error| {
                CameraError::CaptureFailed(format!("convert_byte_array(frame): {error}"))
            })?;

            let hinted_expected = usize::try_from(expected_size.width)
                .ok()
                .and_then(|width| {
                    usize::try_from(expected_size.height)
                        .ok()
                        .and_then(move |height| width.checked_mul(height))
                })
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    CameraError::CaptureFailed(format!(
                        "frame size overflow: {}x{}",
                        expected_size.width, expected_size.height
                    ))
                })?;

            let size = if data.len() == hinted_expected {
                expected_size
            } else {
                self.frame_size_internal(env)?
            };

            let expected = usize::try_from(size.width)
                .ok()
                .and_then(|width| {
                    usize::try_from(size.height)
                        .ok()
                        .and_then(move |height| width.checked_mul(height))
                })
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| {
                    CameraError::CaptureFailed(format!(
                        "frame size overflow: {}x{}",
                        size.width, size.height
                    ))
                })?;

            if data.len() != expected {
                return Err(CameraError::CaptureFailed(format!(
                    "invalid frame byte length {}, expected {} ({}x{}x4)",
                    data.len(),
                    expected,
                    size.width,
                    size.height
                )));
            }

            Ok(Some(RawFrame {
                data,
                width: size.width,
                height: size.height,
                timestamp: start_instant.elapsed(),
            }))
        })
    }

    fn call_bool_no_args(&self, method: &JNIStr) -> Result<bool, CameraError> {
        self.with_env(|env| {
            env.call_method(self.helper.as_obj(), method, jni_sig!("()Z"), &[])
                .and_then(jni::objects::JValueOwned::z)
                .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn call_bool_with_float(&self, method: &JNIStr, value: f32) -> Result<bool, CameraError> {
        self.with_env(|env| {
            env.call_method(
                self.helper.as_obj(),
                method,
                jni_sig!("(F)Z"),
                &[JValue::Float(value)],
            )
            .and_then(jni::objects::JValueOwned::z)
            .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn call_bool_with_int(&self, method: &JNIStr, value: i32) -> Result<bool, CameraError> {
        self.with_env(|env| {
            env.call_method(
                self.helper.as_obj(),
                method,
                jni_sig!("(I)Z"),
                &[JValue::Int(value)],
            )
            .and_then(jni::objects::JValueOwned::z)
            .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn call_bool_with_string(&self, method: &JNIStr, value: &str) -> Result<bool, CameraError> {
        self.with_env(|env| {
            let jvalue = env.new_string(value).map_err(|error| {
                CameraError::PlatformError(format!("new_string(path): {error}"))
            })?;
            env.call_method(
                self.helper.as_obj(),
                method,
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&jvalue)],
            )
            .and_then(jni::objects::JValueOwned::z)
            .map_err(|error| CameraError::PlatformError(format!("{method}: {error}")))
        })
    }

    fn set_zoom(&self, value: f32) -> Result<(), CameraError> {
        if self.call_bool_with_float(jni_str!("setZoom"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("zoom".into()))
        }
    }

    fn set_flash_mode(&self, value: i32) -> Result<(), CameraError> {
        if self.call_bool_with_int(jni_str!("setFlashMode"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("flash".into()))
        }
    }

    fn set_stabilization_mode(&self, value: i32) -> Result<(), CameraError> {
        if self.call_bool_with_int(jni_str!("setStabilizationMode"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("stabilization".into()))
        }
    }

    fn set_dynamic_range(&self, value: i32) -> Result<(), CameraError> {
        if self.call_bool_with_int(jni_str!("setDynamicRange"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("dynamic_range".into()))
        }
    }

    fn set_exposure_compensation(&self, value: f32) -> Result<(), CameraError> {
        if self.call_bool_with_float(jni_str!("setExposureCompensation"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported(
                "exposure_compensation".into(),
            ))
        }
    }

    fn set_focus_mode(&self, value: i32) -> Result<(), CameraError> {
        if self.call_bool_with_int(jni_str!("setFocusMode"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("focus_mode".into()))
        }
    }

    fn set_focus_distance_normalized(&self, value: f32) -> Result<(), CameraError> {
        if self.call_bool_with_float(jni_str!("setFocusDistanceNormalized"), value)? {
            Ok(())
        } else {
            Err(CameraError::ControlUnsupported("focus_distance".into()))
        }
    }

    fn capture_photo_data(&self) -> Result<Vec<u8>, CameraError> {
        if !self.call_bool_no_args(jni_str!("capturePhoto"))? {
            return Err(CameraError::CaptureFailed(
                "CameraHelper.capturePhoto returned false".into(),
            ));
        }

        self.with_env(|env| {
            let data_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("consumePhotoData"),
                    jni_sig!("()[B"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("consumePhotoData JNI call: {error}"))
                })?;

            if data_obj.is_null() {
                return Err(CameraError::CaptureFailed(
                    "consumePhotoData returned null".into(),
                ));
            }

            let data = env.cast_local::<JByteArray>(data_obj).map_err(|error| {
                CameraError::CaptureFailed(format!("photo data is not a byte array: {error}"))
            })?;
            env.convert_byte_array(&data).map_err(|error| {
                CameraError::CaptureFailed(format!("convert_byte_array(photo): {error}"))
            })
        })
    }

    fn capture_raw_photo_data(&self) -> Result<Vec<u8>, CameraError> {
        if !self.call_bool_no_args(jni_str!("captureRawPhoto"))? {
            return Err(CameraError::CaptureFailed(
                "CameraHelper.captureRawPhoto returned false".into(),
            ));
        }

        self.with_env(|env| {
            let data_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("consumeRawPhotoData"),
                    jni_sig!("()[B"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("consumeRawPhotoData JNI call: {error}"))
                })?;

            if data_obj.is_null() {
                return Err(CameraError::CaptureFailed(
                    "consumeRawPhotoData returned null".into(),
                ));
            }

            let data = env.cast_local::<JByteArray>(data_obj).map_err(|error| {
                CameraError::CaptureFailed(format!("photo data is not a byte array: {error}"))
            })?;
            env.convert_byte_array(&data).map_err(|error| {
                CameraError::CaptureFailed(format!("convert_byte_array(raw photo): {error}"))
            })
        })
    }

    fn start_recording(&self, path: &str) -> Result<(), CameraError> {
        if self.call_bool_with_string(jni_str!("startRecording"), path)? {
            Ok(())
        } else {
            Err(CameraError::RecordingError(
                "CameraHelper.startRecording returned false".into(),
            ))
        }
    }

    fn stop_recording(&self) -> Result<(), CameraError> {
        if self.call_bool_no_args(jni_str!("stopRecording"))? {
            Ok(())
        } else {
            Err(CameraError::RecordingError(
                "CameraHelper.stopRecording returned false".into(),
            ))
        }
    }

    fn recording_duration_ms(&self) -> Result<u64, CameraError> {
        self.with_env(|env| {
            let value = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("getRecordingDurationMs"),
                    jni_sig!("()J"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::j)
                .map_err(|error| {
                    CameraError::RecordingError(format!("getRecordingDurationMs: {error}"))
                })?;

            u64::try_from(value).map_err(|_| {
                CameraError::RecordingError(format!(
                    "getRecordingDurationMs returned negative value: {value}"
                ))
            })
        })
    }

    fn start_raw_recording(&self, path: &str) -> Result<(), CameraError> {
        if self.call_bool_with_string(jni_str!("startRawRecording"), path)? {
            Ok(())
        } else {
            Err(CameraError::RecordingError(
                "CameraHelper.startRawRecording returned false".into(),
            ))
        }
    }

    fn stop_raw_recording(&self) -> Result<(), CameraError> {
        if self.call_bool_no_args(jni_str!("stopRawRecording"))? {
            Ok(())
        } else {
            Err(CameraError::RecordingError(
                "CameraHelper.stopRawRecording returned false".into(),
            ))
        }
    }

    fn raw_recording_duration_ms(&self) -> Result<u64, CameraError> {
        self.with_env(|env| {
            let value = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("getRawRecordingDurationMs"),
                    jni_sig!("()J"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::j)
                .map_err(|error| {
                    CameraError::RecordingError(format!("getRawRecordingDurationMs: {error}"))
                })?;

            u64::try_from(value).map_err(|_| {
                CameraError::RecordingError(format!(
                    "getRawRecordingDurationMs returned negative value: {value}"
                ))
            })
        })
    }
}

/// Camera inner implementation for Android.
pub struct CameraInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    resolution: Resolution,
    frame_receiver: async_channel::Receiver<RawFrame>,
    running: Arc<AtomicBool>,
    bridge: Arc<AndroidBridge>,
    recording_mode: Option<RecordingMode>,
}

impl std::fmt::Debug for CameraInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraInner")
            .field("resolution", &self.resolution)
            .finish_non_exhaustive()
    }
}

fn decode_encoded_photo(encoded: &[u8]) -> Result<(Vec<u8>, u32, u32), CameraError> {
    let decoded = image::load_from_memory(encoded)
        .map_err(|error| CameraError::CaptureFailed(format!("decode photo bytes: {error}")))?;
    let rgba = decoded.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Ok((rgba.into_raw(), width, height))
}

impl CameraInner {
    /// List available cameras.
    pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
        AndroidBridge::new()?.list_cameras()
    }

    /// Open a camera by ID.
    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "the cross-platform camera open API is async even when the Android JNI setup completes synchronously"
    )]
    pub async fn open(
        camera_id: &str,
        config: CameraConfig,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Result<Self, CameraError> {
        let bridge = Arc::new(AndroidBridge::new()?);

        let cameras = bridge.list_cameras()?;
        if cameras.iter().all(|camera| camera.id != camera_id) {
            return Err(CameraError::NotFound(camera_id.to_owned()));
        }

        let capabilities =
            bridge.query_capabilities(camera_id, config.resolution, config.frame_rate.max(1))?;
        capabilities.validate()?;

        bridge.open_camera(camera_id, config.resolution, config.frame_rate)?;

        if let Err(error) = bridge.start_capture() {
            let _ = bridge.close_camera();
            return Err(error);
        }

        let resolution = match bridge.frame_size() {
            Ok(size) => size,
            Err(error) => {
                let _ = bridge.stop_capture();
                let _ = bridge.close_camera();
                return Err(error);
            }
        };

        let (sender, receiver) = async_channel::bounded(1);
        let running = Arc::new(AtomicBool::new(true));
        let running_for_thread = Arc::clone(&running);
        let bridge_for_thread = Arc::clone(&bridge);
        let start_instant = Instant::now();
        let frame_wait_ms = i32::try_from((1_000_u64 / u64::from(config.frame_rate.max(1))).max(5))
            .unwrap_or(i32::MAX);
        let mut frame_resolution = resolution;

        std::thread::spawn(move || {
            while running_for_thread.load(Ordering::SeqCst) {
                match bridge_for_thread.wait_for_frame(
                    start_instant,
                    frame_wait_ms,
                    frame_resolution,
                ) {
                    Ok(Some(frame)) => {
                        frame_resolution = Resolution {
                            width: frame.width,
                            height: frame.height,
                        };
                        let _ = sender.force_send(frame);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        running_for_thread.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }

            let _ = bridge_for_thread.stop_capture();
            let _ = bridge_for_thread.close_camera();
        });

        Ok(Self {
            device,
            queue,
            capabilities,
            controls: CameraControls::default(),
            resolution,
            frame_receiver: receiver,
            running,
            bridge,
            recording_mode: None,
        })
    }

    #[must_use]
    pub const fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
        if let Some(ref exposure) = controls.exposure {
            if exposure.mode != ExposureMode::Auto {
                return Err(CameraError::ControlUnsupported(
                    "manual exposure mode".into(),
                ));
            }

            if let Some(ev) = exposure.compensation {
                if !self.capabilities.supports_exposure_compensation {
                    return Err(CameraError::ControlUnsupported(
                        "exposure_compensation".into(),
                    ));
                }
                self.bridge.set_exposure_compensation(ev)?;
            }

            if exposure.iso.is_some() || exposure.duration.is_some() {
                return Err(CameraError::ControlUnsupported(
                    "manual ISO/exposure duration".into(),
                ));
            }
            self.controls.exposure = Some(exposure.clone());
        }

        if let Some(ref focus) = controls.focus {
            let mode = match focus.mode {
                FocusMode::ContinuousAuto => 0,
                FocusMode::Auto => 1,
                FocusMode::Manual => 2,
                FocusMode::Locked => 3,
            };
            self.bridge.set_focus_mode(mode)?;
            if let Some(distance) = focus.distance {
                if !(0.0..=1.0).contains(&distance) {
                    return Err(CameraError::ValueOutOfRange(format!(
                        "focus distance {distance} not in range [0.0, 1.0]"
                    )));
                }
                self.bridge.set_focus_distance_normalized(distance)?;
            }
            if focus.point_of_interest.is_some() {
                return Err(CameraError::ControlUnsupported(
                    "focus point of interest".into(),
                ));
            }
            self.controls.focus = Some(focus.clone());
        }

        if controls.white_balance.is_some() {
            return Err(CameraError::ControlUnsupported(
                "manual white balance".into(),
            ));
        }

        if let Some(zoom) = controls.zoom {
            let Some((min, max)) = self.capabilities.zoom_range else {
                return Err(CameraError::ControlUnsupported("zoom".into()));
            };
            let zoom_value = zoom.get();
            if zoom_value < min || zoom_value > max {
                return Err(CameraError::ValueOutOfRange(format!(
                    "zoom {zoom} not in range [{min}, {max}]"
                )));
            }
            self.bridge.set_zoom(zoom_value)?;
            self.controls.zoom = Some(zoom);
        }

        if let Some(flash) = controls.flash {
            let mode = match flash {
                FlashMode::Off => FLASH_OFF,
                FlashMode::On => FLASH_ON,
                FlashMode::Auto => FLASH_AUTO,
                FlashMode::Torch => FLASH_TORCH,
            };
            self.bridge.set_flash_mode(mode)?;
            self.controls.flash = Some(flash);
        }

        if let Some(profile) = controls.dynamic_range {
            if !self.capabilities.dynamic_ranges.contains(&profile) {
                return Err(CameraError::ControlUnsupported(format!(
                    "dynamic range {profile:?}"
                )));
            }
            let mode = match profile {
                DynamicRangeProfile::Sdr => DYNAMIC_RANGE_SDR,
                DynamicRangeProfile::Hdr10 => DYNAMIC_RANGE_HDR10,
                DynamicRangeProfile::Hlg10 => DYNAMIC_RANGE_HLG10,
                DynamicRangeProfile::DolbyVision => DYNAMIC_RANGE_DOLBY_VISION,
            };
            self.bridge.set_dynamic_range(mode)?;
            self.controls.dynamic_range = Some(profile);
        }

        if let Some(stabilization) = controls.stabilization {
            if !self
                .capabilities
                .stabilization_modes
                .contains(&stabilization)
            {
                return Err(CameraError::ControlUnsupported(format!(
                    "stabilization {stabilization:?}"
                )));
            }
            let mode = match stabilization {
                StabilizationMode::Off => STABILIZATION_OFF,
                StabilizationMode::Standard => STABILIZATION_STANDARD,
                StabilizationMode::Cinematic => STABILIZATION_CINEMATIC,
            };
            self.bridge.set_stabilization_mode(mode)?;
            self.controls.stabilization = Some(stabilization);
        }

        Ok(())
    }

    #[must_use]
    pub const fn controls(&self) -> &CameraControls {
        &self.controls
    }

    #[must_use]
    pub const fn resolution(&self) -> Resolution {
        self.resolution
    }

    pub fn frames(&self) -> impl futures::Stream<Item = Frame> + '_ {
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let receiver = self.frame_receiver.clone();

        futures::stream::unfold(
            (device, queue, receiver),
            |(device, queue, receiver)| async move {
                let raw = receiver.recv().await.ok()?;

                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("AndroidCameraFrame"),
                    size: wgpu::Extent3d {
                        width: raw.width,
                        height: raw.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });

                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &raw.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(raw.width * 4),
                        rows_per_image: Some(raw.height),
                    },
                    wgpu::Extent3d {
                        width: raw.width,
                        height: raw.height,
                        depth_or_array_layers: 1,
                    },
                );

                let frame = Frame {
                    texture,
                    width: raw.width,
                    height: raw.height,
                    format: PixelFormat::Rgba8,
                    timestamp: raw.timestamp,
                };

                Some((frame, (device, queue, receiver)))
            },
        )
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "the cross-platform camera capture API is async even when the Android JNI call completes synchronously"
    )]
    pub async fn capture_photo(&self) -> Result<Photo, CameraError> {
        let encoded = self.bridge.capture_photo_data()?;
        let (rgba, width, height) = decode_encoded_photo(&encoded)?;

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("AndroidCameraPhoto"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        Ok(Photo {
            texture,
            width,
            height,
        })
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "the cross-platform camera raw capture API is async even when the Android JNI call completes synchronously"
    )]
    pub async fn capture_raw_photo(&self) -> Result<RawPhoto, CameraError> {
        if !self.capabilities.supports_raw_photo {
            return Err(CameraError::ControlUnsupported("raw_photo".into()));
        }
        let dng = self.bridge.capture_raw_photo_data()?;
        let resolution = self.bridge.frame_size()?;
        Ok(RawPhoto {
            data: dng,
            width: resolution.width,
            height: resolution.height,
            format: RawPhotoFormat::Dng,
        })
    }

    pub fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| CameraError::RecordingError("path must be valid UTF-8".into()))?;
        self.bridge.start_recording(path_str)?;
        self.recording_mode = Some(RecordingMode::Standard);
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                self.bridge.stop_recording()?;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Raw) => Err(CameraError::RecordingError(
                "raw recording active; call stop_raw_recording".into(),
            )),
            None => Ok(()),
        }
    }

    #[must_use]
    pub fn recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                Duration::from_millis(self.bridge.recording_duration_ms().unwrap_or_else(|error| {
                    panic!("recording_duration_ms failed: {error}");
                }))
            }
            _ => Duration::ZERO,
        }
    }

    pub fn start_raw_recording(&mut self, path: &Path) -> Result<(), CameraError> {
        if !self.capabilities.supports_raw_video {
            return Err(CameraError::ControlUnsupported("raw_video".into()));
        }
        if self.recording_mode.is_some() {
            return Err(CameraError::AlreadyInUse);
        }
        let path_str = path
            .to_str()
            .ok_or_else(|| CameraError::RecordingError("path must be valid UTF-8".into()))?;
        self.bridge.start_raw_recording(path_str)?;
        self.recording_mode = Some(RecordingMode::Raw);
        Ok(())
    }

    pub fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
        match self.recording_mode {
            Some(RecordingMode::Raw) => {
                self.bridge.stop_raw_recording()?;
                self.recording_mode = None;
                Ok(())
            }
            Some(RecordingMode::Standard) => Err(CameraError::RecordingError(
                "standard recording active; call stop_recording".into(),
            )),
            None => Ok(()),
        }
    }

    #[must_use]
    pub fn raw_recording_duration(&self) -> Duration {
        match self.recording_mode {
            Some(RecordingMode::Raw) => Duration::from_millis(
                self.bridge
                    .raw_recording_duration_ms()
                    .unwrap_or_else(|error| panic!("raw_recording_duration_ms failed: {error}")),
            ),
            _ => Duration::ZERO,
        }
    }
}

impl Drop for CameraInner {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        match self.recording_mode {
            Some(RecordingMode::Standard) => {
                let _ = self.bridge.stop_recording();
            }
            Some(RecordingMode::Raw) => {
                let _ = self.bridge.stop_raw_recording();
            }
            None => {}
        }
        let _ = self.bridge.stop_capture();
        let _ = self.bridge.close_camera();
    }
}
