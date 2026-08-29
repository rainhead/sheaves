import Foundation
import Testing
@testable import SheavesCore

@Suite("CalendarDate")
struct CalendarDateTests {
    @Test("round-trips ISO 8601 text")
    func roundTrip() throws {
        let date = try #require(CalendarDate(iso: "2026-08-29"))
        #expect(date.year == 2026)
        #expect(date.month == 8)
        #expect(date.day == 29)
        #expect(date.description == "2026-08-29")
    }

    @Test("pads single digits")
    func padding() {
        #expect(CalendarDate(year: 2026, month: 1, day: 5).description == "2026-01-05")
    }

    @Test("rejects nonsense")
    func rejectsNonsense() {
        #expect(CalendarDate(iso: "not-a-date") == nil)
        #expect(CalendarDate(iso: "2026-13-01") == nil)
        #expect(CalendarDate(iso: "2026-08") == nil)
    }

    @Test("orders chronologically")
    func ordering() {
        let earlier = CalendarDate(year: 2026, month: 8, day: 29)
        let later = CalendarDate(year: 2026, month: 9, day: 1)
        #expect(earlier < later)
        #expect(!(later < earlier))
    }

    @Test("adds days across a month boundary")
    func addingDays() {
        let end = CalendarDate(year: 2026, month: 8, day: 31)
        #expect(end.adding(days: 1) == CalendarDate(year: 2026, month: 9, day: 1))
        #expect(end.adding(days: -31) == CalendarDate(year: 2026, month: 7, day: 31))
    }

    @Test("adds days across a leap day")
    func leapDay() {
        let date = CalendarDate(year: 2028, month: 2, day: 28)
        #expect(date.adding(days: 1) == CalendarDate(year: 2028, month: 2, day: 29))
    }

    /// The whole point of the type: a day must not drift when the machine is east or
    /// west of UTC, which is exactly what storing `spent_date` as a `Date` would cause.
    @Test("holds its day in any time zone", arguments: ["Pacific/Kiritimati", "UTC", "Pacific/Midway"])
    func stableAcrossTimeZones(zoneName: String) throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try #require(TimeZone(identifier: zoneName))
        let day = CalendarDate(year: 2026, month: 8, day: 29)
        #expect(CalendarDate(day.startOfDay(in: calendar), in: calendar) == day)
    }

    @Test("finds the start of the week for each Harvest week-start setting")
    func startOfWeek() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try #require(TimeZone(identifier: "UTC"))
        // 2026-08-29 is a Saturday.
        let saturday = CalendarDate(year: 2026, month: 8, day: 29)
        #expect(saturday.startOfWeek(weekStart: .monday, in: calendar) == CalendarDate(year: 2026, month: 8, day: 24))
        #expect(saturday.startOfWeek(weekStart: .sunday, in: calendar) == CalendarDate(year: 2026, month: 8, day: 23))
        #expect(saturday.startOfWeek(weekStart: .saturday, in: calendar) == saturday)
    }
}

@Suite("Hours formatting")
struct HoursFormattingTests {
    @Test("formats as hours and minutes")
    func hoursMinutes() {
        #expect(1.4.formattedHours(.hoursMinutes) == "1:24")
        #expect(0.0.formattedHours(.hoursMinutes) == "0:00")
        #expect(10.5.formattedHours(.hoursMinutes) == "10:30")
    }

    /// Rounding to minutes before splitting keeps 0:60 from ever appearing.
    @Test("never renders sixty minutes")
    func neverSixtyMinutes() {
        #expect(0.99999.formattedHours(.hoursMinutes) == "1:00")
        #expect(1.99999.formattedHours(.hoursMinutes) == "2:00")
    }

    @Test("formats as decimal hours")
    func decimal() {
        #expect(1.4.formattedHours(.decimal) == "1.40")
        #expect(0.25.formattedHours(.decimal) == "0.25")
    }

    @Test("clamps negative durations")
    func negatives() {
        #expect((-1.0).formattedHours(.hoursMinutes) == "0:00")
    }

    @Test("shows seconds for a running timer")
    func withSeconds() {
        #expect(1.4.formattedHoursWithSeconds() == "1:24:00")
        #expect((7.0 / 3600).formattedHoursWithSeconds() == "0:00:07")
    }
}
