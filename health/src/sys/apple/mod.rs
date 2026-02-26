use crate::{HealthDataType, HealthError, HealthSample};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn health_is_available() -> bool;
        fn health_request_auth(
            read_types: &str,
            write_types: &str,
            callback: Box<dyn FnOnce(String) -> ()>,
        );
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

pub async fn request_authorization(
    read_types: &[HealthDataType],
    write_types: &[HealthDataType],
) -> Result<(), HealthError> {
    let read = types_to_csv(read_types);
    let write = types_to_csv(write_types);
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::health_request_auth(
        &read,
        &write,
        Box::new(move |error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(HealthError::PermissionDenied));
            }
        }),
    );
    rx.await
        .map_err(|_| HealthError::PlatformError("callback dropped".into()))?
}

pub async fn query_samples(
    data_type: HealthDataType,
    start_date: &str,
    end_date: &str,
) -> Result<Vec<HealthSample>, HealthError> {
    let type_str = type_to_str(data_type);
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::health_query(
        type_str,
        start_date,
        end_date,
        Box::new(move |json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(json));
            } else {
                let _ = tx.send(Err(HealthError::PlatformError(error)));
            }
        }),
    );
    let json = rx
        .await
        .map_err(|_| HealthError::PlatformError("callback dropped".into()))??;
    Ok(parse_samples(data_type, &json))
}

pub async fn write_sample(sample: HealthSample) -> Result<(), HealthError> {
    let type_str = type_to_str(sample.data_type());
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::health_write(
        type_str,
        sample.value(),
        sample.unit(),
        sample.start_date(),
        sample.end_date(),
        Box::new(move |error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(HealthError::PlatformError(error)));
            }
        }),
    );
    rx.await
        .map_err(|_| HealthError::PlatformError("callback dropped".into()))?
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

fn types_to_csv(types: &[HealthDataType]) -> String {
    types
        .iter()
        .map(|t| type_to_str(*t))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_samples(data_type: HealthDataType, json: &str) -> Vec<HealthSample> {
    // Format: "value\tunit\tstart\tend\tsource\n..."
    json.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '\t').collect();
            if parts.len() >= 4 {
                let mut sample = HealthSample::new(
                    data_type,
                    parts[0].parse().unwrap_or(0.0),
                    parts[1],
                    parts[2],
                    parts[3],
                );
                if let Some(src) = parts.get(4).filter(|s| !s.is_empty()) {
                    sample = sample.with_source(*src);
                }
                Some(sample)
            } else {
                None
            }
        })
        .collect()
}
