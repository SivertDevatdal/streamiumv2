// swift-tools-version:5.9
import PackageDescription

// StreamiumKit wraps the Rust core (delivered as StreamiumCoreFFI.xcframework
// plus the generated StreamiumCore.swift) with the Apple playback engines and
// the library/persistence layer used by the apps.
//
// Run apple/scripts/build-xcframework.sh before building: it produces both
// git-ignored inputs (Frameworks/StreamiumCoreFFI.xcframework and
// Sources/StreamiumCore/StreamiumCore.swift).

let package = Package(
    name: "StreamiumKit",
    platforms: [.iOS(.v17), .macOS(.v14), .tvOS(.v17)],
    products: [
        .library(name: "StreamiumKit", targets: ["StreamiumKit"]),
        .library(name: "StreamiumCore", targets: ["StreamiumCore"]),
    ],
    targets: [
        .binaryTarget(
            name: "StreamiumCoreFFI",
            path: "Frameworks/StreamiumCoreFFI.xcframework"
        ),
        .target(
            name: "StreamiumCore",
            dependencies: ["StreamiumCoreFFI"],
            path: "Sources/StreamiumCore"
        ),
        .target(
            name: "StreamiumKit",
            dependencies: ["StreamiumCore"],
            path: "Sources/StreamiumKit"
        ),
        .testTarget(
            name: "StreamiumKitTests",
            dependencies: ["StreamiumKit"],
            path: "Tests/StreamiumKitTests"
        ),
    ]
)
