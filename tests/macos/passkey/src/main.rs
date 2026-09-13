//! macOS structured test for `waterkit-passkey`.

use std::process::ExitCode;

use waterkit_passkey as passkey;
use waterkit_test_report::{write_report_block_to_stdout, TestCase, TestReport};

#[tokio::main]
async fn main() -> ExitCode {
    let mut report = TestReport::new("macos", "waterkit-passkey");

    match passkey::is_available().await {
        Ok(availability) if availability.is_platform_supported => {
            report.push(TestCase::passed_with_message(
                "passkey.availability",
                format!(
                    "supported=true user_verification={} discoverable={}",
                    availability.supports_user_verification,
                    availability.supports_discoverable_credentials
                ),
            ))
        }
        Ok(_) => report.push(TestCase::failed(
            "passkey.availability",
            "passkey reports unsupported on macOS 13+",
        )),
        Err(error) => report.push(TestCase::failed(
            "passkey.availability",
            format!("passkey availability failed: {error}"),
        )),
    }

    write_report_block_to_stdout(&report).expect("failed to write structured test report");

    if report.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
