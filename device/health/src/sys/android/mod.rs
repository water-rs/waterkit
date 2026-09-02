use crate::{HealthDataType, HealthError, HealthSample};
use futures::future;
use jni::objects::{JObject, JValue};
use jni::{Env, jni_sig, jni_str};
use waterkit_build::{AndroidError, DexHelper, decode_string, dex_helper, with_android_context};
use waterkit_core::Timestamp;

/// `waterkit.health.HealthHelper`, embedded as a DEX by this crate's build script and
/// loaded on first use.
static HELPER: DexHelper = dex_helper!("waterkit.health.HealthHelper");

impl From<AndroidError> for HealthError {
    fn from(error: AndroidError) -> Self {
        Self::Platform(error.to_string())
    }
}

fn is_available_with_context(
    env: &mut Env<'_>,
    context: &JObject<'_>,
) -> Result<bool, HealthError> {
    let helper_class = HELPER.class(env, context)?;
    env.call_static_method(helper_class, jni_str!("isAvailable"), jni_sig!("()Z"), &[])
        .map_err(|error| {
            HealthError::Platform(format!("HealthHelper.isAvailable failed: {error}"))
        })?
        .z()
        .map_err(|error| {
            HealthError::Platform(format!("isAvailable result decode failed: {error}"))
        })
}

pub fn is_available() -> bool {
    with_android_context(is_available_with_context).unwrap_or_else(|error| {
        panic!("waterkit-health: failed to query availability with Android context: {error}")
    })
}

const fn type_to_str(data_type: HealthDataType) -> &'static str {
    match data_type {
        HealthDataType::Steps => "steps",
        HealthDataType::HeartRate => "heartRate",
        HealthDataType::ActiveEnergy => "activeEnergy",
        HealthDataType::Distance => "distance",
        HealthDataType::Weight => "weight",
        HealthDataType::Height => "height",
        HealthDataType::BloodOxygen => "bloodOxygen",
        HealthDataType::Sleep => "sleep",
    }
}

fn parse_samples(data_type: HealthDataType, payload: &str) -> Vec<HealthSample> {
    payload
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() < 4 {
                return None;
            }
            let start = parts[2].parse::<Timestamp>().ok()?;
            let end = parts[3].parse::<Timestamp>().ok()?;
            let mut sample = HealthSample::new(
                data_type,
                parts[0].parse().unwrap_or(0.0),
                parts[1],
                start,
                end,
            );
            if let Some(src) = parts.get(4).filter(|value| !value.is_empty()) {
                sample = sample.source(*src);
            }
            Some(sample)
        })
        .collect()
}

fn map_android_health_error(error: String) -> HealthError {
    if error.to_ascii_lowercase().contains("permission") {
        HealthError::PermissionDenied
    } else {
        HealthError::Platform(error)
    }
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    let data_type_name = type_to_str(data_type);
    let start_str = start.to_string();
    let end_str = end.to_string();
    future::ready(with_android_context(|env, context| {
        let helper_class = HELPER.class(env, context)?;

        let data_type_java = env.new_string(data_type_name).map_err(|error| {
            HealthError::Platform(format!("new_string data_type failed: {error}"))
        })?;
        let start = env.new_string(&start_str).map_err(|error| {
            HealthError::Platform(format!("new_string start failed: {error}"))
        })?;
        let end = env.new_string(&end_str).map_err(|error| {
            HealthError::Platform(format!("new_string end failed: {error}"))
        })?;

        let payload = env
            .call_static_method(
                helper_class,
                jni_str!("querySamples"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[
                    JValue::Object(context),
                    JValue::Object(&data_type_java),
                    JValue::Object(&start),
                    JValue::Object(&end),
                ],
            )
            .map_err(|error| {
                HealthError::Platform(format!("HealthHelper.querySamples failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                HealthError::Platform(format!("querySamples return decode failed: {error}"))
            })?;
        if payload.is_null() {
            return Ok(Vec::new());
        }

        let payload = decode_string(env, &payload)?;
        Ok(parse_samples(data_type, &payload))
    }))
    .await
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let data_type_name = type_to_str(sample.data_type());
    let start_str = sample.start().to_string();
    let end_str = sample.end().to_string();
    future::ready(with_android_context(|env, context| {
        let helper_class = HELPER.class(env, context)?;

        let data_type = env.new_string(data_type_name).map_err(|error| {
            HealthError::Platform(format!("new_string data_type failed: {error}"))
        })?;
        let unit = env.new_string(sample.unit()).map_err(|error| {
            HealthError::Platform(format!("new_string unit failed: {error}"))
        })?;
        let start = env.new_string(&start_str).map_err(|error| {
            HealthError::Platform(format!("new_string start_date failed: {error}"))
        })?;
        let end = env.new_string(&end_str).map_err(|error| {
            HealthError::Platform(format!("new_string end_date failed: {error}"))
        })?;

        let error = env
            .call_static_method(
                helper_class,
                jni_str!("writeSample"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;DLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[
                    JValue::Object(context),
                    JValue::Object(&data_type),
                    JValue::Double(sample.value()),
                    JValue::Object(&unit),
                    JValue::Object(&start),
                    JValue::Object(&end),
                ],
            )
            .map_err(|error| {
                HealthError::Platform(format!("HealthHelper.writeSample failed: {error}"))
            })?
            .l()
            .map_err(|error| {
                HealthError::Platform(format!("writeSample return decode failed: {error}"))
            })?;

        if error.is_null() {
            Ok(())
        } else {
            Err(map_android_health_error(decode_string(env, &error)?))
        }
    }))
    .await
}
