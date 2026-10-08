//! macOS structured test for `waterkit-vision`.

use std::process::ExitCode;

use waterkit_test_report::{write_report_block_to_stdout, TestCase, TestReport};
use waterkit_vision::{CodeScanner, Symbology, VisionError};

#[tokio::main]
async fn main() -> ExitCode {
    let mut report = TestReport::new("macos", "waterkit-vision");

    let capabilities = CodeScanner::capabilities();
    if capabilities.available || !capabilities.symbologies.is_empty() {
        report.push(TestCase::failed(
            "scanner.capabilities",
            "macOS has no system code scanner yet capabilities were reported",
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

    // Every symbology is inexpressible on macOS; the failure must name them
    // before any presentation is attempted.
    match CodeScanner::new(waterkit_vision::EnumSet::all())
        .scan()
        .await
    {
        Err(VisionError::Unsupported(message)) if message.contains("Qr") => {
            report.push(TestCase::passed_with_message(
                "scanner.scan_unsupported_symbologies",
                format!("unsupported: {message}"),
            ));
        }
        other => report.push(TestCase::failed(
            "scanner.scan_unsupported_symbologies",
            format!("scan() of the full vocabulary returned {other:?}"),
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
