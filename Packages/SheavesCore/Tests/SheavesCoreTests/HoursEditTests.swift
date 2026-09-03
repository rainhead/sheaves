import Foundation
import Testing
@testable import SheavesCore

@Suite("HoursEdit")
struct HoursEditTests {
    /// The regression this exists for. A running entry's field opens showing the
    /// total rounded to the minute; committing that back sets the total *and*
    /// restarts the clock, so a user who clicked the duration and then clicked the
    /// notes would silently lose the seconds since — and however long they thought
    /// about it.
    @Test("sends nothing when a running entry's field was never touched")
    func untouchedRunning() {
        #expect(HoursEdit.commit(draft: "1:30", asOpened: "1:30", banked: 1.2, isRunning: true) == nil)
    }

    @Test("sends nothing when a stopped entry's field was never touched")
    func untouchedStopped() {
        #expect(HoursEdit.commit(draft: "1:30", asOpened: "1:30", banked: 1.5, isRunning: false) == nil)
    }

    @Test("sends what was typed onto a running entry")
    func editedRunning() {
        #expect(HoursEdit.commit(draft: "2", asOpened: "1:30", banked: 1.2, isRunning: true) == 2)
    }

    /// Banked hours exclude the segment still being measured, so they are not the
    /// question. Typing the banked figure onto a running entry is a real request.
    @Test("sends a running entry's banked figure when that is what was typed")
    func editedRunningToBanked() {
        #expect(HoursEdit.commit(draft: "1:12", asOpened: "1:30", banked: 1.2, isRunning: true) == 1.2)
    }

    @Test("sends what was typed onto a stopped entry")
    func editedStopped() {
        #expect(HoursEdit.commit(draft: "2:15", asOpened: "1:30", banked: 1.5, isRunning: false) == 2.25)
    }

    /// Retyping a stopped entry's own total in another notation is a change to the
    /// text and no change at all to Harvest.
    @Test("sends nothing when a stopped entry was retyped to the same total")
    func retypedStopped() {
        #expect(HoursEdit.commit(draft: "1.5", asOpened: "1:30", banked: 1.5, isRunning: false) == nil)
    }

    @Test("sends nothing for text that is not a duration")
    func unparseable() {
        #expect(HoursEdit.commit(draft: "soon", asOpened: "1:30", banked: 1.5, isRunning: false) == nil)
    }

    @Test("reads the separator the locale writes")
    func localeDecimal() {
        #expect(
            HoursEdit.commit(
                draft: "1,75",
                asOpened: "1:30",
                banked: 1.5,
                isRunning: false,
                locale: Locale(identifier: "fr_FR")
            ) == 1.75
        )
    }
}
