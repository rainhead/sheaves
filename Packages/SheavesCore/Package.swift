// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SheavesCore",
    platforms: [.macOS(.v15), .iOS(.v18)],
    products: [
        .library(name: "SheavesCore", targets: ["SheavesCore"])
    ],
    targets: [
        // The Rust core (crates/sheaves-core) and the Swift UniFFI generates to call
        // it, in Sources/SheavesCore/Generated. Both are built by
        // Scripts/build-xcframework.sh and are not checked in.
        .binaryTarget(
            name: "sheaves_coreFFI",
            path: "Frameworks/sheaves_coreFFI.xcframework"
        ),
        .target(
            name: "SheavesCore",
            dependencies: ["sheaves_coreFFI"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
        .testTarget(
            name: "SheavesCoreTests",
            dependencies: ["SheavesCore"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
    ]
)
