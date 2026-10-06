//! macOS structured test for `waterkit-vision`.

use std::process::ExitCode;

use waterkit_test_report::{TestCase, TestReport, write_report_block_to_stdout};
use waterkit_vision::{CodeScanner, Symbology, VisionError};

#[tokio::main]
async fn main() -> ExitCode {
    let mut report = TestReport::new("macos", "waterkit-vision");

    if CodeScanner::capabilities().available {
        report.push(TestCase::failed(
            "scanner.capabilities",
            "macOS has no system code scanner yet availability reported true",
        ));
    } else {
        report.push(TestCase::passed("scanner.capabilities"));
    }

    match CodeScanner::new(Symbology::Qr).scan().await {
        Err(VisionError::Unsupported(message)) => report.push(TestCase::passed_with_message(
            "scanner.scan",
            format!("unsupported: {message}"),
        )),
        other => report.push(TestCase::failed(
            "scanner.scan",
            format!("scan() on macOS returned {other:?}"),
        )),
    }

    finish(&report)
}

fn finish(report: &TestReport) -> ExitCode {
    write_report_block_to_stdout(report).expect("failed to write structured test report");

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
