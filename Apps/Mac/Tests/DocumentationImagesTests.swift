import Foundation
import Testing
@testable import Sheaves

/// Triggers the renderer in the app target and says where it put the files.
///
/// Off unless told to run, so an ordinary test run touches no files. The
/// `TEST_RUNNER_` prefix is required and is stripped on the way through:
/// xcodebuild forwards only variables carrying it into the test process. Without it
/// this suite is skipped — and a skipped suite still reports TEST SUCCEEDED, so
/// check the printed paths rather than the exit code.
///
///     TEST_RUNNER_SHEAVES_RENDER_IMAGES=1 xcodebuild -project Sheaves.xcodeproj \
///       -scheme Sheaves -destination 'platform=macOS' test \
///       -only-testing:SheavesTests/DocumentationImagesTests
///
/// The files land in the app's sandbox container, because the test host is the app
/// and it may not write to the repository. `Scripts/render-docs-images.sh` runs this
/// and copies them out.
@Suite(
    "DocumentationImagesTests",
    .enabled(if: ProcessInfo.processInfo.environment["SHEAVES_RENDER_IMAGES"] != nil)
)
@MainActor
struct DocumentationImagesTests {
    @Test("renders the documentation images")
    func renders() async throws {
        let directory = URL.temporaryDirectory.appending(path: "sheaves-images", directoryHint: .isDirectory)
        let written = try await DocumentationImages.render(into: directory)

        #expect(!written.isEmpty)
        for url in written {
            let size = try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int ?? 0
            // A rendered file of a few bytes is a blank canvas, which is how this
            // fails when a view does not get what it needs from the environment.
            #expect(size > 10_000, "\(url.lastPathComponent) is \(size) bytes — probably blank")
            print("RENDERED \(url.path)")
        }
    }
}
