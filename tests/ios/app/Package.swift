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
            path: "WaterKitTest"
        )
    ]
)
