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

/// The StoreKit Test environment registration, in its own app launch.
///
/// storekitd binds an app's payment environment when the app's StoreKit
/// client first registers at launch, and does not re-evaluate it when an
/// `SKTestSession` is created later in the same process. On a fresh
/// simulator the persisted Octane environment does not exist at the
/// suite's launch, so the suite's `Product.purchase()` calls route to the
/// real sandbox and hang on an interactive Apple-ID sign-in. Running this
/// test first gives `store_test_begin` a dedicated launch whose session
/// persists the configuration; the suite's later launch then binds to the
/// test environment.
final class StoreKitEnvironmentTests: XCTestCase {
    func testRegisterStoreKitTestEnvironment() {
        let error = store_test_begin().toString()
        XCTAssertEqual(error, "", "SKTestSession could not be created: \(error)")
    }
}
