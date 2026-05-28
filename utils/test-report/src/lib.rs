//! Structured WaterKit integration test reports.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

/// Marker emitted before a structured report in mixed human-readable output.
pub const REPORT_BEGIN: &str = "WATERKIT_TEST_REPORT_BEGIN";

/// Marker emitted after a structured report in mixed human-readable output.
pub const REPORT_END: &str = "WATERKIT_TEST_REPORT_END";

/// A single test case outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    /// The test case executed and satisfied its assertions.
    Passed,
    /// The test case executed and violated an assertion.
    Failed,
    /// The test case could not execute because the platform, hardware, or
    /// permission state does not expose the capability under test.
    Skipped,
}

/// One structured integration test result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCase {
    /// Stable test case name.
    pub name: String,
    /// Test case status.
    pub status: TestStatus,
    /// Optional diagnostic message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl TestCase {
    /// Creates a passing test case.
    #[must_use]
    pub fn passed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Passed,
            message: None,
        }
    }

    /// Creates a passing test case with a diagnostic message.
    #[must_use]
    pub fn passed_with_message(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Passed,
            message: Some(message.into()),
        }
    }

    /// Creates a failing test case.
    #[must_use]
    pub fn failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Failed,
            message: Some(message.into()),
        }
    }

    /// Creates a skipped test case.
    #[must_use]
    pub fn skipped(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Skipped,
            message: Some(message.into()),
        }
    }
}

/// Structured report produced by a WaterKit integration harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Platform under test, such as `macos`, `ios`, or `android`.
    pub platform: String,
    /// Crate or feature package under test.
    pub crate_name: String,
    /// Test case outcomes.
    pub cases: Vec<TestCase>,
}

impl TestReport {
    /// Current report schema version.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Creates an empty report.
    #[must_use]
    pub fn new(platform: impl Into<String>, crate_name: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            platform: platform.into(),
            crate_name: crate_name.into(),
            cases: Vec::new(),
        }
    }

    /// Adds one case to the report.
    pub fn push(&mut self, case: TestCase) {
        self.cases.push(case);
    }

    /// Returns whether every executed case passed and at least one case was
    /// recorded.
    #[must_use]
    pub fn is_success(&self) -> bool {
        !self.cases.is_empty()
            && self
                .cases
                .iter()
                .all(|case| case.status != TestStatus::Failed)
    }

    /// Returns whether the report contains at least one failed case.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.cases
            .iter()
            .any(|case| case.status == TestStatus::Failed)
    }

    /// Number of passing cases.
    #[must_use]
    pub fn passed_count(&self) -> usize {
        self.count(TestStatus::Passed)
    }

    /// Number of skipped cases.
    #[must_use]
    pub fn skipped_count(&self) -> usize {
        self.count(TestStatus::Skipped)
    }

    /// Number of failing cases.
    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.count(TestStatus::Failed)
    }

    /// Formats all failed cases as a compact diagnostic string.
    #[must_use]
    pub fn failure_summary(&self) -> String {
        self.cases
            .iter()
            .filter(|case| case.status == TestStatus::Failed)
            .map(|case| match &case.message {
                Some(message) => format!("{}: {}", case.name, message),
                None => case.name.clone(),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn count(&self, status: TestStatus) -> usize {
        self.cases
            .iter()
            .filter(|case| case.status == status)
            .count()
    }
}

/// Serializes a report as pretty JSON.
///
/// # Errors
///
/// Returns a serialization error if the report cannot be encoded.
pub fn to_json_pretty(report: &TestReport) -> serde_json::Result<String> {
    serde_json::to_string_pretty(report)
}

/// Parses a report from JSON.
///
/// # Errors
///
/// Returns a deserialization error if the JSON does not match the report
/// schema.
pub fn from_json(json: &str) -> serde_json::Result<TestReport> {
    serde_json::from_str(json)
}

/// Writes a report wrapped in stable begin/end markers.
///
/// # Errors
///
/// Returns an I/O error if the destination cannot be written or the report
/// cannot be serialized.
pub fn write_report_block(mut writer: impl Write, report: &TestReport) -> io::Result<()> {
    let json = to_json_pretty(report).map_err(io::Error::other)?;
    writeln!(writer, "{REPORT_BEGIN}")?;
    writeln!(writer, "{json}")?;
    writeln!(writer, "{REPORT_END}")?;
    writer.flush()
}

/// Writes a marked report block to stdout.
///
/// # Errors
///
/// Returns an I/O error if stdout cannot be written or the report cannot be
/// serialized.
pub fn write_report_block_to_stdout(report: &TestReport) -> io::Result<()> {
    let stdout = io::stdout();
    write_report_block(stdout.lock(), report)
}

/// Extracts one marked report block from mixed process output.
///
/// # Errors
///
/// Returns a deserialization error if a marked block is present but does not
/// contain a valid report.
pub fn parse_report_block(output: &str) -> serde_json::Result<Option<TestReport>> {
    let Some(begin) = output.find(REPORT_BEGIN) else {
        return Ok(None);
    };
    let json_start = begin + REPORT_BEGIN.len();
    let Some(relative_end) = output[json_start..].find(REPORT_END) else {
        return Ok(None);
    };
    let json_end = json_start + relative_end;
    let json = output[json_start..json_end].trim();
    from_json(json).map(Some)
}

#[cfg(test)]
mod tests {
    use super::{
        REPORT_BEGIN, REPORT_END, TestCase, TestReport, TestStatus, parse_report_block,
        to_json_pretty,
    };

    #[test]
    fn summary_counts_case_statuses() {
        let mut report = TestReport::new("macos", "waterkit-sensor");
        report.push(TestCase::passed("accelerometer"));
        report.push(TestCase::skipped("gyroscope", "hardware unavailable"));
        report.push(TestCase::failed("barometer", "read failed"));

        assert_eq!(report.passed_count(), 1);
        assert_eq!(report.skipped_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert!(report.has_failures());
        assert!(!report.is_success());
        assert_eq!(report.cases[2].status, TestStatus::Failed);
    }

    #[test]
    fn parser_extracts_marked_report_from_process_output() {
        let mut report = TestReport::new("android", "waterkit-location");
        report.push(TestCase::passed("location.permission"));
        let json = to_json_pretty(&report).expect("report should serialize");
        let output = format!("noise\n{REPORT_BEGIN}\n{json}\n{REPORT_END}\nmore noise");

        let parsed = parse_report_block(&output)
            .expect("marked report should parse")
            .expect("marked report should be present");

        assert_eq!(parsed, report);
    }
}
