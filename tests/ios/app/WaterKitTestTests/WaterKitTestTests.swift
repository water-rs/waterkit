import XCTest

/// The harness suite as one XCTest. Running it through `xcodebuild test` is
/// what makes testmanagerd install the host app for development, which
/// `SKTestSession` requires; a `simctl` or `devicectl` install does not
/// count.
///
/// `__swift_bridge__$run_tests_json` comes from the bridging header and
/// resolves through `BUNDLE_LOADER` into the host app binary, so the suite
/// runs against the same library and process state the app itself uses.
final class WaterKitTestTests: XCTestCase {
    /// Runs the whole Rust suite and writes its structured report.
    ///
    /// XCTest invokes this method inside a `RunTestsFromRunLoop` source on
    /// the main thread, and the cases under test hop to the main queue
    /// (Core Location, camera, any UIKit touch). Calling `run_tests_json`
    /// directly here would park the main thread inside `block_on`, so those
    /// hops could never run and the suite would deadlock. An `async` test
    /// method suspends instead of occupying its thread: while this method
    /// awaits, the main run loop keeps draining and the main-queue hops run.
    func testWaterKitSuite() async throws {
        let report = await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                continuation.resume(
                    returning: RustString(ptr: __swift_bridge__$run_tests_json()).toString()
                )
            }
        }
        try ReportWriter.persist(report)
    }
}
