// swift-tools-version: 5.9
import PackageDescription

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
                .unsafeFlags(["-import-objc-header", "/Users/lexoliu/Coding/kit/tests/ios/app/WaterKitTest/Generated/Bridging-Header.h"])
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L/Users/lexoliu/Coding/kit/target/aarch64-apple-ios-sim/debug",
                    "-lwaterkit_test_ios",
                    "-framework", "CoreFoundation",
                    "-framework", "Security" // For biometric
                ])
            ]
        )
    ]
)
