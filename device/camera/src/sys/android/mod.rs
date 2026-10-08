//! Android camera implementation using Camera2 + `MediaRecorder` via JNI/Kotlin bridge.
//!
//! The preview `ImageReader` produces GPU-sampled `PRIVATE` buffers. Each
//! image's `AHardwareBuffer` is imported into wgpu through
//! `wgpu-external-frame` with no CPU access to its pixels, and the image goes
//! back to the reader once the GPU no longer reads it. The reader acquires
//! at most a fixed number of images at once — one queued in Kotlin, one in
//! the frame channel, and the frames a consumer still holds — so a slow or
//! hoarding consumer drops camera frames instead of exhausting the pool. A
//! frame's orientation combines the sensor orientation, the lens facing and
//! the display rotation at the time the frame arrived.
//!
//! When the camera opens with `CameraConfig::analysis`, the session adds a
//! second `ImageReader` in `YUV_420_888` at the analysis size, read by a
//! second reader thread into `AnalysisFrame`s whose images close when the
//! last clone drops.

mod analysis;
mod frames;

pub use analysis::AnalysisImage;
use analysis::{ImagePlane, RawAnalysisFrame};
use frames::{FrameLease, RawFrame};

use crate::{
    AnalysisFrame, CameraCapabilities, CameraConfig, CameraControls, CameraError, CameraInfo,
    DynamicRangeProfile, ExposureMode, FlashMode, FocusMode, Frame, Photo, RawPhoto,
    RawPhotoFormat, RawVideoFormat, Resolution, StabilizationMode,
};
use jni::objects::{
    Global, JByteArray, JByteBuffer, JFloatArray, JIntArray, JObject, JObjectArray, JString, JValue,
};
use jni::strings::JNIStr;
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::num::NonZeroU8;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::clock::StreamClock;
use crate::sys::android_session::{CameraHelper, FrameThread, OpenCamera};
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

/// What fixes a camera's frame orientation besides the display rotation.
#[derive(Debug, Clone, Copy)]
pub(super) struct SensorMounting {
    sensor_orientation: u32,
    lens_faces_back: bool,
}

#[derive(Debug)]
pub(super) struct AndroidBridge {
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

    #[expect(
        clippy::too_many_lines,
        reason = "one linear pass over the CameraCharacteristics keys; splitting it would scatter a single query across helpers that share no meaning."
    )]
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

        let dynamic_range_values =
            self.call_int_array_with_camera(jni_str!("getSupportedDynamicRanges"), camera_id)?;
        let mut dynamic_ranges = Vec::with_capacity(dynamic_range_values.len());
        for value in dynamic_range_values {
            dynamic_ranges.push(match value {
                DYNAMIC_RANGE_SDR => DynamicRangeProfile::Sdr,
                DYNAMIC_RANGE_HDR10 => DynamicRangeProfile::Hdr10,
                DYNAMIC_RANGE_HLG10 => DynamicRangeProfile::Hlg10,
                DYNAMIC_RANGE_DOLBY_VISION => DynamicRangeProfile::DolbyVision,
                value => {
                    return Err(CameraError::PlatformError(format!(
                        "getSupportedDynamicRanges returned unknown profile {value}"
                    )));
                }
            });
        }
        let supports_dolby_vision = dynamic_ranges.contains(&DynamicRangeProfile::DolbyVision);
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

        if !dynamic_ranges.contains(&DynamicRangeProfile::Sdr) {
            return Err(CameraError::PlatformError(
                "getSupportedDynamicRanges omitted the required SDR profile".into(),
            ));
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
                vec![RawVideoFormat::Nv12Frames]
            } else {
                Vec::new()
            },
        })
    }

    fn frame_size(&self) -> Result<Resolution, CameraError> {
        self.with_env(|env| self.frame_size_internal(env))
    }

    /// Closes a captured frame — a preview `CapturedFrame` or an analysis
    /// `AnalysisFrame` — returning its image to the reader so acquisition
    /// resumes.
    fn release_image(&self, frame: &Global<JObject<'static>>) {
        let released = self.with_env(|env| {
            env.call_method(frame.as_obj(), jni_str!("close"), jni_sig!("()V"), &[])
                .map(drop)
                .map_err(|error| {
                    CameraError::PlatformError(format!("captured image close: {error}"))
                })
        });
        if let Err(error) = released {
            panic!("a camera image could not be returned to its reader: {error}");
        }
    }

    fn sensor_mounting(&self, camera_id: &str) -> Result<SensorMounting, CameraError> {
        let sensor_orientation =
            self.call_int_with_camera(jni_str!("getSensorOrientation"), camera_id)?;
        Ok(SensorMounting {
            sensor_orientation: u32::try_from(sensor_orientation).map_err(|_| {
                CameraError::PlatformError(format!(
                    "SENSOR_ORIENTATION is negative: {sensor_orientation}"
                ))
            })?,
            lens_faces_back: self.call_bool_with_camera(jni_str!("lensFacesBack"), camera_id)?,
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

fn dynamic_range_profile(value: i32) -> Result<DynamicRangeProfile, CameraError> {
    match value {
        DYNAMIC_RANGE_SDR => Ok(DynamicRangeProfile::Sdr),
        DYNAMIC_RANGE_HDR10 => Ok(DynamicRangeProfile::Hdr10),
        DYNAMIC_RANGE_HLG10 => Ok(DynamicRangeProfile::Hlg10),
        DYNAMIC_RANGE_DOLBY_VISION => Ok(DynamicRangeProfile::DolbyVision),
        _ => Err(CameraError::CaptureFailed(format!(
            "unknown dynamic range profile in captured frame: {value}"
        ))),
    }
}

/// `Arc<AndroidBridge>` is the [`CameraHelper`] a capture session sequences;
/// clones let the reader threads and image leases share the helper.
impl CameraHelper for Arc<AndroidBridge> {
    type Frame = RawFrame;
    type Analysis = RawAnalysisFrame;

    fn open_camera(
        &self,
        camera_id: &str,
        resolution: Resolution,
        frame_rate: u32,
        analysis: Option<Resolution>,
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
            // Zero analysis dimensions mean no analysis stream.
            let (analysis_width, analysis_height) = match analysis {
                Some(analysis) => (
                    i32::try_from(analysis.width).map_err(|_| {
                        CameraError::OpenFailed("analysis width exceeds i32".into())
                    })?,
                    i32::try_from(analysis.height).map_err(|_| {
                        CameraError::OpenFailed("analysis height exceeds i32".into())
                    })?,
                ),
                None => (0, 0),
            };

            let opened = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("openCamera"),
                    jni_sig!("(Ljava/lang/String;IIIII)Z"),
                    &[
                        JValue::Object(&camera_id_java),
                        JValue::Int(width),
                        JValue::Int(height),
                        JValue::Int(fps),
                        JValue::Int(analysis_width),
                        JValue::Int(analysis_height),
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

    /// Takes the next preview image, if one arrives within `timeout_ms`, as a
    /// frame ready for import: a reference on its `AHardwareBuffer` and a
    /// lease that closes the frame — returning its image and freeing an
    /// in-flight slot — once the GPU no longer reads it.
    fn wait_for_frame(
        &self,
        clock: &StreamClock<Duration>,
        timeout_ms: i32,
    ) -> Result<Option<RawFrame>, CameraError> {
        self.with_env(|env| {
            let frame_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("waitForNextFrame"),
                    jni_sig!("(I)Lwaterkit/camera/CameraHelper$CapturedFrame;"),
                    &[JValue::Int(timeout_ms.max(0))],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("waitForNextFrame JNI call: {error}"))
                })?;

            if frame_obj.is_null() {
                return Ok(None);
            }

            let hardware_buffer = env
                .call_method(
                    &frame_obj,
                    jni_str!("getHardwareBuffer"),
                    jni_sig!("()Landroid/hardware/HardwareBuffer;"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getHardwareBuffer: {error}"))
                })?;
            let capture_time_ns = env
                .call_method(
                    &frame_obj,
                    jni_str!("getCaptureTimeNs"),
                    jni_sig!("()J"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::j)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getCaptureTimeNs: {error}"))
                })?;
            let capture_time_ns = u64::try_from(capture_time_ns).map_err(|_| {
                CameraError::CaptureFailed(format!(
                    "sensor timestamp is negative: {capture_time_ns}"
                ))
            })?;
            let display_rotation = env
                .call_method(
                    &frame_obj,
                    jni_str!("getDisplayRotation"),
                    jni_sig!("()I"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getDisplayRotation: {error}"))
                })?;
            let display_rotation = u32::try_from(display_rotation).map_err(|_| {
                CameraError::CaptureFailed(format!("display rotation {display_rotation}"))
            })?;
            let data_space = env
                .call_method(&frame_obj, jni_str!("getDataSpace"), jni_sig!("()I"), &[])
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| CameraError::CaptureFailed(format!("getDataSpace: {error}")))?;
            let profile = env
                .call_method(
                    &frame_obj,
                    jni_str!("getDynamicRangeProfile"),
                    jni_sig!("()I"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getDynamicRangeProfile: {error}"))
                })?;
            let profile = dynamic_range_profile(profile)?;

            let lease = FrameLease::new(
                Self::clone(self),
                env.new_global_ref(&frame_obj).map_err(|error| {
                    CameraError::CaptureFailed(format!("new_global_ref(frame): {error}"))
                })?,
            );
            // SAFETY: `env` is this thread's attached JNI environment and
            // `hardware_buffer` a live `android.hardware.HardwareBuffer`; the
            // NDK handle borrows its buffer only until the frame below takes
            // its own reference.
            let buffer = unsafe {
                ndk::hardware_buffer::HardwareBuffer::from_jni(
                    env.get_raw().cast(),
                    hardware_buffer.as_raw().cast(),
                )
            };
            // Java's ImageReader waits for the camera's write fence before it
            // hands an image out, so the buffer is ready as it arrives.
            let frame = RawFrame::new(
                &buffer,
                lease,
                display_rotation,
                data_space,
                profile,
                clock.timestamp(Duration::from_nanos(capture_time_ns)),
            );
            env.call_method(&hardware_buffer, jni_str!("close"), jni_sig!("()V"), &[])
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("HardwareBuffer.close: {error}"))
                })?;
            Ok(Some(frame))
        })
    }

    /// Takes the next analysis image, if one arrives within `timeout_ms`, as
    /// a frame holding the `android.media.Image` globally: its planes'
    /// direct-buffer addresses and strides are resolved now, while an env is
    /// attached, and dropping the frame closes the helper's `AnalysisFrame`
    /// lease, returning the image to its reader.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear JNI fetch of an image and its three planes; splitting it would share the same locals across helpers that mean nothing alone"
    )]
    fn wait_for_analysis_frame(
        &self,
        clock: &StreamClock<Duration>,
        timeout_ms: i32,
    ) -> Result<Option<RawAnalysisFrame>, CameraError> {
        self.with_env(|env| {
            let frame_obj = env
                .call_method(
                    self.helper.as_obj(),
                    jni_str!("waitForNextAnalysisFrame"),
                    jni_sig!("(I)Lwaterkit/camera/CameraHelper$AnalysisFrame;"),
                    &[JValue::Int(timeout_ms.max(0))],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!(
                        "waitForNextAnalysisFrame JNI call: {error}"
                    ))
                })?;

            if frame_obj.is_null() {
                return Ok(None);
            }

            let image_obj = env
                .call_method(
                    &frame_obj,
                    jni_str!("getImage"),
                    jni_sig!("()Landroid/media/Image;"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| CameraError::CaptureFailed(format!("getImage: {error}")))?;
            if image_obj.is_null() {
                return Err(CameraError::CaptureFailed(
                    "an analysis frame carries a null image".into(),
                ));
            }
            let display_rotation = env
                .call_method(
                    &frame_obj,
                    jni_str!("getDisplayRotation"),
                    jni_sig!("()I"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getDisplayRotation: {error}"))
                })?;
            let display_rotation = u32::try_from(display_rotation).map_err(|_| {
                CameraError::CaptureFailed(format!("display rotation {display_rotation}"))
            })?;
            let data_space = env
                .call_method(&frame_obj, jni_str!("getDataSpace"), jni_sig!("()I"), &[])
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| CameraError::CaptureFailed(format!("getDataSpace: {error}")))?;
            let capture_time_ns = env
                .call_method(
                    &frame_obj,
                    jni_str!("getCaptureTimeNs"),
                    jni_sig!("()J"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::j)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getCaptureTimeNs: {error}"))
                })?;
            let capture_time_ns = u64::try_from(capture_time_ns).map_err(|_| {
                CameraError::CaptureFailed(format!(
                    "sensor timestamp is negative: {capture_time_ns}"
                ))
            })?;
            let image = env.new_global_ref(&image_obj).map_err(|error| {
                CameraError::CaptureFailed(format!("new_global_ref(image): {error}"))
            })?;
            let frame = env.new_global_ref(&frame_obj).map_err(|error| {
                CameraError::CaptureFailed(format!("new_global_ref(frame): {error}"))
            })?;

            let width = env
                .call_method(&image_obj, jni_str!("getWidth"), jni_sig!("()I"), &[])
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| CameraError::CaptureFailed(format!("getWidth: {error}")))?;
            let width = u32::try_from(width)
                .map_err(|_| CameraError::CaptureFailed(format!("invalid image width: {width}")))?;
            let height = env
                .call_method(&image_obj, jni_str!("getHeight"), jni_sig!("()I"), &[])
                .and_then(jni::objects::JValueOwned::i)
                .map_err(|error| CameraError::CaptureFailed(format!("getHeight: {error}")))?;
            let height = u32::try_from(height).map_err(|_| {
                CameraError::CaptureFailed(format!("invalid image height: {height}"))
            })?;

            let plane_list = env
                .call_method(
                    &image_obj,
                    jni_str!("getPlanes"),
                    jni_sig!("()[Landroid/media/Image$Plane;"),
                    &[],
                )
                .and_then(jni::objects::JValueOwned::l)
                .map_err(|error| CameraError::CaptureFailed(format!("getPlanes: {error}")))?;
            if plane_list.is_null() {
                return Err(CameraError::CaptureFailed(
                    "an analysis image carries no planes".into(),
                ));
            }
            let plane_array = env
                .cast_local::<JObjectArray>(plane_list)
                .map_err(|error| {
                    CameraError::CaptureFailed(format!("getPlanes is not an array: {error}"))
                })?;
            let plane_count = plane_array.len(env).map_err(|error| {
                CameraError::CaptureFailed(format!("getPlanes length: {error}"))
            })?;
            if plane_count != 3 {
                return Err(CameraError::CaptureFailed(format!(
                    "a YUV_420_888 image has 3 planes, not {plane_count}"
                )));
            }
            let mut image_planes = [ImagePlane {
                address: 0,
                len: 0,
                row_stride: 0,
                pixel_stride: 0,
            }; 3];
            for (index, slot) in image_planes.iter_mut().enumerate() {
                let plane = plane_array.get_element(env, index).map_err(|error| {
                    CameraError::CaptureFailed(format!("getPlanes[{index}]: {error}"))
                })?;
                let buffer_obj = env
                    .call_method(
                        &plane,
                        jni_str!("getBuffer"),
                        jni_sig!("()Ljava/nio/ByteBuffer;"),
                        &[],
                    )
                    .and_then(jni::objects::JValueOwned::l)
                    .map_err(|error| {
                        CameraError::CaptureFailed(format!("plane {index} getBuffer: {error}"))
                    })?;
                let buffer = env.cast_local::<JByteBuffer>(buffer_obj).map_err(|error| {
                    CameraError::CaptureFailed(format!(
                        "plane {index} buffer is not a ByteBuffer: {error}"
                    ))
                })?;
                let address = env.get_direct_buffer_address(&buffer).map_err(|error| {
                    CameraError::CaptureFailed(format!(
                        "plane {index} is not a direct buffer: {error}"
                    ))
                })?;
                let capacity = env.get_direct_buffer_capacity(&buffer).map_err(|error| {
                    CameraError::CaptureFailed(format!("plane {index} capacity: {error}"))
                })?;
                let row_stride = env
                    .call_method(&plane, jni_str!("getRowStride"), jni_sig!("()I"), &[])
                    .and_then(jni::objects::JValueOwned::i)
                    .map_err(|error| {
                        CameraError::CaptureFailed(format!("plane {index} getRowStride: {error}"))
                    })?;
                let pixel_stride = env
                    .call_method(&plane, jni_str!("getPixelStride"), jni_sig!("()I"), &[])
                    .and_then(jni::objects::JValueOwned::i)
                    .map_err(|error| {
                        CameraError::CaptureFailed(format!("plane {index} getPixelStride: {error}"))
                    })?;
                *slot = ImagePlane {
                    address: address as usize,
                    len: capacity,
                    row_stride: usize::try_from(row_stride).map_err(|_| {
                        CameraError::CaptureFailed(format!("plane {index} row stride {row_stride}"))
                    })?,
                    pixel_stride: usize::try_from(pixel_stride).map_err(|_| {
                        CameraError::CaptureFailed(format!(
                            "plane {index} pixel stride {pixel_stride}"
                        ))
                    })?,
                };
            }

            Ok(Some(RawAnalysisFrame {
                image: AnalysisImage::new(
                    Self::clone(self),
                    frame,
                    image,
                    image_planes,
                    width,
                    height,
                ),
                display_rotation,
                data_space,
                timestamp: clock.timestamp(Duration::from_nanos(capture_time_ns)),
            }))
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
}

/// Camera inner implementation for Android.
pub struct CameraInner {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    capabilities: CameraCapabilities,
    controls: CameraControls,
    resolution: Resolution,
    mounting: SensorMounting,
    /// The analysis reader thread, `Some` only when the camera was opened
    /// with `CameraConfig::analysis`. It drops before `frames_thread` so the
    /// analysis reader stops before the preview thread tears the camera down.
    analysis_thread: Option<FrameThread<RawAnalysisFrame>>,
    /// Owns the capture session: its drop stops capture, closes the camera,
    /// and returns once the reader thread has finished the teardown.
    frames_thread: FrameThread<RawFrame>,
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
        frames::check_device(&device)?;
        let bridge = Arc::new(AndroidBridge::new()?);

        let cameras = bridge.list_cameras()?;
        if cameras.iter().all(|camera| camera.id != camera_id) {
            return Err(CameraError::NotFound(camera_id.to_owned()));
        }

        let capabilities =
            bridge.query_capabilities(camera_id, config.resolution, config.frame_rate.max(1))?;
        capabilities.validate()?;
        let mounting = bridge.sensor_mounting(camera_id)?;

        // The capture owns the open camera, so every failure path closes
        // it exactly once as the value drops.
        let capture = OpenCamera::open(
            Arc::clone(&bridge),
            camera_id,
            config.resolution,
            config.frame_rate,
            config.analysis.map(|analysis| analysis.resolution),
        )?
        .start_capture()?;

        let resolution = bridge.frame_size()?;

        let frame_wait_ms = i32::try_from((1_000_u64 / u64::from(config.frame_rate.max(1))).max(5))
            .unwrap_or(i32::MAX);

        Ok(Self {
            device: Arc::clone(&device),
            queue,
            capabilities,
            controls: CameraControls::default(),
            resolution,
            mounting,
            analysis_thread: config.analysis.map(|_| {
                FrameThread::spawn_analysis(Arc::clone(&bridge), Arc::clone(&device), frame_wait_ms)
            }),
            frames_thread: FrameThread::spawn(capture, device, frame_wait_ms),
            bridge,
            recording_mode: None,
        })
    }

    #[must_use]
    pub const fn capabilities(&self) -> &CameraCapabilities {
        &self.capabilities
    }

    #[expect(clippy::too_many_lines)]
    #[expect(
        clippy::unused_async,
        reason = "the Android backend applies controls over JNI synchronously"
    )]
    pub async fn apply_controls(&mut self, controls: &CameraControls) -> Result<(), CameraError> {
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

    pub fn frames(&self) -> impl futures::Stream<Item = Result<Frame, CameraError>> + '_ {
        let importer = wgpu_external_frame::ahardware_buffer::HardwareBufferImporter::new(
            &self.device,
            &self.queue,
        );
        let receiver = self.frames_thread.frames().clone();
        let mounting = self.mounting;

        // The state is `None` once an error has been yielded, which ends the
        // stream after it.
        futures::stream::unfold(Some((importer, receiver)), move |state| async move {
            let (mut importer, receiver) = state?;
            let frame = match receiver.recv().await.ok()? {
                Ok(raw) => raw.import(&mut importer, mounting),
                Err(error) => Err(error),
            };
            let next = frame.is_ok().then_some((importer, receiver));
            Some((frame, next))
        })
    }

    /// The analysis stream: each item becomes an [`AnalysisFrame`] sharing
    /// the acquired image, with the frame's mounting turning its display
    /// rotation into an orientation.
    pub fn analysis_frames(
        &self,
    ) -> impl futures::Stream<Item = Result<AnalysisFrame, CameraError>> + '_ {
        let mounting = self.mounting;
        crate::analysis::stream(
            self.analysis_thread
                .as_ref()
                .map(|thread| thread.frames().clone()),
            move |raw| raw.and_then(|frame| frame.into_frame(mounting)),
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

    #[expect(
        clippy::unused_async,
        reason = "the Android backend starts recording over JNI synchronously"
    )]
    pub async fn start_recording(&mut self, path: &Path) -> Result<(), CameraError> {
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

    #[expect(
        clippy::unused_async,
        reason = "the Android backend stops recording over JNI synchronously"
    )]
    pub async fn stop_recording(&mut self) -> Result<(), CameraError> {
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

    #[expect(
        clippy::unused_async,
        reason = "the Android backend starts recording over JNI synchronously"
    )]
    pub async fn start_raw_recording(&mut self, path: &Path) -> Result<(), CameraError> {
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

    #[expect(
        clippy::unused_async,
        reason = "the Android backend stops recording over JNI synchronously"
    )]
    pub async fn stop_raw_recording(&mut self) -> Result<(), CameraError> {
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

    /// Ends an active standard recording inline; the JNI stop is synchronous,
    /// so `Drop` paths just run it.
    pub fn abandon_recording(&mut self) {
        if matches!(self.recording_mode, Some(RecordingMode::Standard)) {
            let _ = self.bridge.stop_recording();
        }
        self.recording_mode = None;
    }

    /// Ends an active raw recording inline, for `Drop` paths that cannot
    /// await.
    pub fn abandon_raw_recording(&mut self) {
        if matches!(self.recording_mode, Some(RecordingMode::Raw)) {
            let _ = self.bridge.stop_raw_recording();
        }
        self.recording_mode = None;
    }
}
