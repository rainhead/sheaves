import AppKit
import SheavesCore
import Testing
@testable import Sheaves

/// The menu bar glyph is tinted while a timer runs, and that tint is fragile in a
/// way nothing on screen announces.
///
/// A status bar button draws a *template* image in the menu bar's own text colour,
/// ignoring `contentTintColor` while it does — so a tint asked for the obvious way
/// is simply dropped, and the item looks exactly as it did before. Template-ness is
/// the difference between a tint that survives and one that does not, so it is what
/// these assert.
@Suite("Status item icon")
@MainActor
struct StatusItemIconTests {
    private func entry(running: Bool) -> TrackedEntry {
        TrackedEntry(
            id: .server(1), project: Reference(id: 1, name: "Online Store"),
            task: Reference(id: 2, name: "Programming"), client: nil, notes: nil,
            spentDate: .today(), bankedHours: 0.5, isRunning: running,
            timerStartedAt: running ? Date() : nil, isLocked: false, isBillable: true,
            isPending: false, updatedAt: Date()
        )
    }

    @Test("a running timer's glyph carries its own colour")
    func runningIsTinted() throws {
        let icon = try #require(StatusItemController.icon(for: .running(entry(running: true))))
        #expect(icon.isTemplate == false)
    }

    /// The idle glyph is Sheaves' own, from the asset catalog, not a system symbol:
    /// a missing asset would leave the menu bar item with no image at all.
    @Test("idle shows the Sheaves glyph, sized for the menu bar")
    func idleIsTheSheavesGlyph() throws {
        let icon = try #require(StatusItemController.icon(for: .idle))
        #expect(icon.name() == "Sheaves")
        #expect(icon.size == NSSize(width: 16, height: 16))
    }

    @Test("every other state follows the menu bar")
    func othersAreTemplates() throws {
        for activity in [TimeTracker.Activity.recent(entry(running: false)), .idle] {
            let icon = try #require(StatusItemController.icon(for: activity))
            #expect(icon.isTemplate, "\(activity) should be drawn in the menu bar's own colour")
        }
    }
}
