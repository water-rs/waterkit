use crate::{HealthDataType, HealthError, HealthSample};
use waterkit_core::Timestamp;

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn health_is_available() -> bool;
        fn health_query(
            data_type: &str,
            start: &str,
            end: &str,
            callback: Box<dyn FnOnce(String, String) -> ()>,
        );
        fn health_write(
            data_type: &str,
            value: f64,
            unit: &str,
            start: &str,
            end: &str,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
    }
}

pub fn is_available() -> bool {
    ffi::health_is_available()
}

pub async fn query_samples(
    data_type: HealthDataType,
    start: Timestamp,
    end: Timestamp,
) -> Result<Vec<HealthSample>, HealthError> {
    let type_str = type_to_str(data_type);
    let start_str = start.to_string();
    let end_str = end.to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::health_query(
        type_str,
        &start_str,
        &end_str,
        Box::new(move |json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(json));
            } else {
                let _ = tx.send(Err(HealthError::Platform(error)));
            }
        }),
    );
    let json = rx
        .await
        .map_err(|_| HealthError::Platform("callback dropped".into()))??;
    Ok(parse_samples(data_type, &json))
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let type_str = type_to_str(sample.data_type());
    let start_str = sample.start().to_string();
    let end_str = sample.end().to_string();
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::health_write(
        type_str,
        sample.value(),
        sample.unit(),
        &start_str,
        &end_str,
        Box::new(move |error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(HealthError::Platform(error)));
            }
        }),
    );
    rx.await
        .map_err(|_| HealthError::Platform("callback dropped".into()))?
}

const fn type_to_str(t: HealthDataType) -> &'static str {
    match t {
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

fn parse_samples(data_type: HealthDataType, json: &str) -> Vec<HealthSample> {
    // Format: "value\tunit\tstart\tend\tsource\n..."
    json.lines()
        .filter(|l| !l.is_empty())
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
            if let Some(src) = parts.get(4).filter(|s| !s.is_empty()) {
                sample = sample.source(*src);
            }
            Some(sample)
        })
        .collect()
}
