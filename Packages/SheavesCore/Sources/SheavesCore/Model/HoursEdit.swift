import Foundation

/// What a closed duration field should send, if anything.
///
/// Where the field lives is a view's business; this decision is not. It is the same
/// rule on any platform, and it is subtle enough to be worth stating once, in one
/// place, with tests.
public enum HoursEdit {
    /// The hours to send, or nil to send nothing.
    ///
    /// `asOpened` is the text the field was showing when it opened, and a field still
    /// showing it has asked for nothing. That matters most while a timer runs:
    /// `TimeTracker.updateHours` restarts a live entry's measurement from the moment
    /// the change lands, so echoing the opening value back would round the total to
    /// the displayed minute and throw away however long the field sat open — for a
    /// user who only clicked the duration and then clicked elsewhere. A stopped entry
    /// must not be marked pending over a number nobody changed either.
    ///
    /// An edited running entry always sends. Its total is moving, so there is nothing
    /// to compare it against: banked hours exclude the segment still being measured.
    public static func commit(
        draft: String,
        asOpened: String,
        banked: Double,
        isRunning: Bool,
        locale: Locale = .autoupdatingCurrent
    ) -> Double? {
        guard draft != asOpened else { return nil }
        guard let hours = Double.hours(parsing: draft, locale: locale) else { return nil }
        guard isRunning || hours != banked else { return nil }
        return hours
    }
}
