//! macOS structured test binary for `waterkit-location`.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use waterkit_location::Location;
use waterkit_permission::{Permission, PermissionStatus};
use waterkit_test_report::{
    write_report_block, write_report_block_to_stdout, TestCase, TestReport,
};

#[tokio::main]
async fn main() -> ExitCode {
    let report = run_location_tests().await;
    write_report_outputs(&report).expect("failed to write structured test report");

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn run_location_tests() -> TestReport {
    let mut report = TestReport::new("macos", "waterkit-location");
    let status = waterkit_permission::check(Permission::Location).await;

    match status {
        PermissionStatus::Granted => {
            report.push(TestCase::passed("location.permission"));
        }
        PermissionStatus::NotDetermined
        | PermissionStatus::Denied
        | PermissionStatus::Restricted => {
            report.push(TestCase::skipped(
                "location.permission",
                format!("location permission is {status:?}"),
            ));
            report.push(TestCase::skipped(
                "location.get",
                "location permission is not granted",
            ));
            return report;
        }
        unexpected => {
            report.push(TestCase::failed(
                "location.permission",
                format!("unsupported permission status {unexpected:?}"),
            ));
            return report;
        }
    }

    match Location::get().await {
        Ok(location) => record_location(&mut report, &location),
        Err(error) => report.push(TestCase::failed(
            "location.get",
            format!("permission is granted but location read failed: {error}"),
        )),
    }

    report
}

fn record_location(report: &mut TestReport, location: &Location) {
    let latitude = location.latitude().get();
    let longitude = location.longitude().get();

    if !latitude.is_finite() || !longitude.is_finite() {
        report.push(TestCase::failed(
            "location.get",
            format!("location contained non-finite coordinates lat={latitude} lon={longitude}"),
        ));
        return;
    }

    if let Some(accuracy) = location.horizontal_accuracy() {
        if !accuracy.is_finite() || accuracy < 0.0 {
            report.push(TestCase::failed(
                "location.get",
                format!("horizontal accuracy must be finite and non-negative, got {accuracy}"),
            ));
            return;
        }
    }

    if let Some(altitude) = location.altitude() {
        if !altitude.is_finite() {
            report.push(TestCase::failed(
                "location.get",
                format!("altitude must be finite, got {altitude}"),
            ));
            return;
        }
    }

    report.push(TestCase::passed_with_message(
        "location.get",
        format!("lat={latitude:.6} lon={longitude:.6}"),
    ));
}

fn write_report_outputs(report: &TestReport) -> io::Result<()> {
    write_report_block_to_stdout(report)?;

    if let Some(path) = macos_bundle_log_path() {
        let file = File::create(path)?;
        write_report_block(file, report)?;
    }

    Ok(())
}

fn macos_bundle_log_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let macos_dir = executable.parent()?;

    if macos_dir.file_name()? != OsStr::new("MacOS") {
        return None;
    }

    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()? != OsStr::new("Contents") {
        return None;
    }

    let app_dir = contents_dir.parent()?;
    let build_dir = app_dir.parent()?;
    Some(build_dir.join("location-test.log"))
}
