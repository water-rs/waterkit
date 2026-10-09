import Foundation

/// Persists the harness's structured report inside the app container, where
/// the `waterkit-test` runner collects it:
/// `Documents/waterkit-test-reports/waterkit-test-report.json`.
///
/// The path is the same for every run: the runner reads the report only
/// after its test has finished, and a finished test always rewrites the file,
/// so a report that comes back belongs to the run that produced it. Reports
/// of earlier runs are removed first so they do not pile up in the
/// container.
enum ReportWriter {
    static let fileName = "waterkit-test-report.json"

    static func persist(_ report: String) throws {
        let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let reports = documents.appendingPathComponent("waterkit-test-reports", isDirectory: true)
        if FileManager.default.fileExists(atPath: reports.path) {
            try FileManager.default.removeItem(at: reports)
        }
        try FileManager.default.createDirectory(at: reports, withIntermediateDirectories: true)
        try report.write(
            to: reports.appendingPathComponent(fileName),
            atomically: true,
            encoding: .utf8
        )
    }
}
