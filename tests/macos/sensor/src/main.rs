//! macOS structured test for `waterkit-sensor`.

use std::process::ExitCode;

use waterkit_sensor::{
    Accelerometer, AmbientLight, Barometer, Gyroscope, Magnetometer, ScalarData, SensorData,
    SensorError,
};
use waterkit_test_report::{write_report_block_to_stdout, TestCase, TestReport};

#[tokio::main]
async fn main() -> ExitCode {
    let mut report = TestReport::new("macos", "waterkit-sensor");

    if Accelerometer::capabilities().available {
        record_three_axis(
            &mut report,
            "accelerometer.read",
            Accelerometer::read().await,
        );
    } else {
        report.push(TestCase::skipped(
            "accelerometer.read",
            "accelerometer is unavailable on this device",
        ));
    }

    if Gyroscope::capabilities().available {
        record_three_axis(&mut report, "gyroscope.read", Gyroscope::read().await);
    } else {
        report.push(TestCase::skipped(
            "gyroscope.read",
            "gyroscope is unavailable on this device",
        ));
    }

    if Magnetometer::capabilities().available {
        record_three_axis(&mut report, "magnetometer.read", Magnetometer::read().await);
    } else {
        report.push(TestCase::skipped(
            "magnetometer.read",
            "magnetometer is unavailable on this device",
        ));
    }

    if Barometer::capabilities().available {
        record_scalar(&mut report, "barometer.read", Barometer::read().await);
    } else {
        report.push(TestCase::skipped(
            "barometer.read",
            "barometer is unavailable on this device",
        ));
    }

    if AmbientLight::capabilities().available {
        record_scalar(
            &mut report,
            "ambient_light.read",
            AmbientLight::read().await,
        );
    } else {
        report.push(TestCase::skipped(
            "ambient_light.read",
            "ambient light sensor is unavailable on this device",
        ));
    }

    finish(report)
}

fn record_three_axis(
    report: &mut TestReport,
    name: &'static str,
    result: Result<SensorData, SensorError>,
) {
    match result {
        Ok(data) if data.x().is_finite() && data.y().is_finite() && data.z().is_finite() => {
            report.push(TestCase::passed_with_message(
                name,
                format!("x={:.3} y={:.3} z={:.3}", data.x(), data.y(), data.z()),
            ));
        }
        Ok(data) => {
            report.push(TestCase::failed(
                name,
                format!(
                    "sensor returned non-finite sample x={} y={} z={}",
                    data.x(),
                    data.y(),
                    data.z()
                ),
            ));
        }
        Err(error) => {
            report.push(TestCase::failed(
                name,
                format!("sensor reported available but read failed: {error}"),
            ));
        }
    }
}

fn record_scalar(
    report: &mut TestReport,
    name: &'static str,
    result: Result<ScalarData, SensorError>,
) {
    match result {
        Ok(data) if data.value().is_finite() => {
            report.push(TestCase::passed_with_message(
                name,
                format!("value={:.3}", data.value()),
            ));
        }
        Ok(data) => {
            report.push(TestCase::failed(
                name,
                format!("sensor returned non-finite sample value={}", data.value()),
            ));
        }
        Err(error) => {
            report.push(TestCase::failed(
                name,
                format!("sensor reported available but read failed: {error}"),
            ));
        }
    }
}

fn finish(report: TestReport) -> ExitCode {
    write_report_block_to_stdout(&report).expect("failed to write structured test report");

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
