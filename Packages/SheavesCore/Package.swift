// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "SheavesCore",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "SheavesCore", targets: ["SheavesCore"])
    ],
    targets: [
        .target(
            name: "SheavesCore",
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
        .testTarget(
            name: "SheavesCoreTests",
            dependencies: ["SheavesCore"],
            swiftSettings: [.swiftLanguageMode(.v6)]
        ),
    ]
)
