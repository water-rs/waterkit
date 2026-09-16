// swift-tools-version: 6.3.0
import Foundation
import PackageDescription

// Manifest-relative paths keep the test app buildable from any checkout
// location; `#filePath` is the manifest's own absolute path at evaluate time.
let packageDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let repoRoot =
    packageDir
    .deletingLastPathComponent() // tests/ios
    .deletingLastPathComponent() // tests
    .deletingLastPathComponent() // repository root
let bridgingHeader =
    packageDir
    .appendingPathComponent("WaterKitTest/Generated/Bridging-Header.h").path
let librarySearchPath =
    repoRoot
    .appendingPathComponent("target/aarch64-apple-ios-sim/debug").path

let package = Package(
    name: "WaterKitTest",
    platforms: [.iOS(.v15)],
    products: [
        .executable(name: "WaterKitTest", targets: ["WaterKitTest"])
    ],
    dependencies: [],
    targets: [
        .executableTarget(
            name: "WaterKitTest",
            dependencies: [],
            path: "WaterKitTest",
            swiftSettings: [
                .unsafeFlags(["-import-objc-header", bridgingHeader])
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L\(librarySearchPath)",
                    "-lwaterkit_test_ios",
                    "-framework", "CoreFoundation",
                    "-framework", "Security", // For biometric
                ])
            ]
        )
    ]
)
