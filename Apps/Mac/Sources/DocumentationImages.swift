#if DEBUG
import Foundation
import SheavesCore
import SwiftUI

/// Renders the images the README and pull requests use, from the real views.
///
/// Nothing is ever put on screen, which is the only way to photograph this app
/// without a person at the keyboard: the panel dismisses the moment attention moves
/// anywhere else, so a screen capture needs a machine nobody is touching. Rendering
/// also fixes the state, so an image can show the cases worth showing — a budget, an
/// overrun, a project with neither — instead of whatever the app happened to be doing.
///
/// The view is hosted in a window that is never ordered front, rather than handed to
/// `ImageRenderer`. `ImageRenderer` renders in a single pass, and `SizedScrollView`
/// only learns its height from `onGeometryChange` on a later one — so the entry list
/// and the project list both come out zero-height and the picture is an empty frame.
/// A hosting view laid out for real settles that the way the running app does.
///
/// This lives in the app target rather than in the tests, and it has to. A view reads
/// the tracker with `@Environment(TimeTracker.self)`, which matches on type identity;
/// the test bundle links its own copy of `SheavesCore`, so a tracker built there is a
/// *different* `TimeTracker` and every app view rendered with it traps looking for
/// one. Building the tracker here keeps both on the same copy.
@MainActor
enum DocumentationImages {
    /// Writes every documentation image into `directory` and returns what it wrote.
    static func render(into directory: URL) async throws -> [URL] {
        let tracker = await makeTracker()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return [
            try await write(
                DayView().environment(tracker),
                width: 380,
                to: directory.appending(path: "menu-bar.png")
            ),
            // The duration open for typing, which a picture of the whole panel
            // cannot show: the edit state lives in the panel's own selection.
            try await write(
                EntryRow(
                    entry: tracker.entries.first { $0.isRunning } ?? tracker.entries[0],
                    format: HoursFormat(company: tracker.company),
                    isEditingNotes: .constant(false),
                    isEditingHours: .constant(true),
                    isConfirmingResume: .constant(false)
                )
                .environment(tracker)
                .padding(8),
                width: 380,
                to: directory.appending(path: "edit-hours.png")
            ),
        ]
    }

    /// `TimeTracker` state is `private(set)`, so the only way to populate one is to
    /// let it sync — against the scripted account below rather than Harvest.
    private static func makeTracker() async -> TimeTracker {
        let scratch = URL.temporaryDirectory.appending(path: "sheaves-docs-\(UUID().uuidString)")
        let tracker = TimeTracker(
            client: HarvestClient(credentials: .init(accountID: "0", token: "none"), transport: DemoAccount()),
            keychain: KeychainStore(service: "com.rainhead.Sheaves.docs-\(UUID().uuidString)"),
            snapshots: SnapshotStore(fileURL: scratch.appending(path: "snapshot.json")),
            queue: MutationQueue(fileURL: scratch.appending(path: "queue.json"))
        )
        await tracker.sync()
        return tracker
    }

    /// Retina, because these are read at full size in a README.
    private static let scale = 2

    private static func write(_ view: some View, width: CGFloat, to url: URL) async throws -> URL {
        let hosting = NSHostingView(
            rootView: view
                .frame(width: width)
                .background(Color(nsColor: .windowBackgroundColor))
        )
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: hosting.fittingSize.height)

        // Offscreen and never ordered front. A window is what gives the hosting view a
        // real layout pass and the main screen's backing scale, so the image comes out
        // at retina resolution without one appearing.
        let window = NSWindow(
            contentRect: hosting.frame,
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        // Pinned, not inherited. A hosting view takes the system appearance, so the
        // same command run after dark produced a dark panel and would have flipped
        // the README's images without anyone deciding to.
        window.appearance = NSAppearance(named: .aqua)
        hosting.appearance = NSAppearance(named: .aqua)
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()

        // A turn of the run loop for the geometry readers to report and for the views
        // that size themselves from that to take their real height.
        try await Task.sleep(for: .milliseconds(200))
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: hosting.fittingSize.height)
        window.setContentSize(hosting.frame.size)
        hosting.layoutSubtreeIfNeeded()

        // Drawn into a bitmap of explicitly doubled pixel dimensions rather than
        // through `cacheDisplay`, which takes its scale from the window's screen —
        // and a window that is never shown has none, so that route gives 1x.
        let bounds = hosting.bounds
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int(bounds.width) * scale,
            pixelsHigh: Int(bounds.height) * scale,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ) else { throw RenderError.failed(url.lastPathComponent) }
        // Points, not pixels: the difference is what makes it a 2x image.
        rep.size = bounds.size

        guard let context = NSGraphicsContext(bitmapImageRep: rep) else {
            throw RenderError.failed(url.lastPathComponent)
        }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        hosting.displayIgnoringOpacity(bounds, in: context)
        NSGraphicsContext.restoreGraphicsState()

        guard let png = rep.representation(using: .png, properties: [:]) else {
            throw RenderError.failed(url.lastPathComponent)
        }

        try png.write(to: url)
        return url
    }

    enum RenderError: Error {
        case failed(String)
    }
}

#endif
