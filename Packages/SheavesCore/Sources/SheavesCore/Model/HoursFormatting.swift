import Foundation

/// How the account prefers to see durations. Harvest reports this as `time_format`.
public enum HoursFormat: String, Sendable, Codable {
    case hoursMinutes = "hours_minutes"
    case decimal

    public init(company: HarvestCompany?) {
        self = HoursFormat(rawValue: company?.timeFormat ?? "") ?? .hoursMinutes
    }
}

public extension Double {
    /// Reads a duration the way people type them: `1` is an hour, `1:30` and `0:45`
    /// are hours and minutes, `1.5` is decimal — through the locale, so `1,5` where
    /// the separator is a comma. Nil for anything else, including negatives.
    static func hours(parsing text: String, locale: Locale = .autoupdatingCurrent) -> Double? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        if let colon = trimmed.firstIndex(of: ":") {
            let wholePart = String(trimmed[..<colon])
            let minutePart = String(trimmed[trimmed.index(after: colon)...])
            // ":45" means three quarters of an hour; "1:5" means 1:05, as on a clock.
            // The parts go through the locale so the digits the formatter writes —
            // Arabic-Indic included — read back in.
            let whole = wholePart.isEmpty ? 0 : (try? Int(wholePart, format: .number.locale(locale)))
            guard let whole, whole >= 0,
                  let minutes = try? Int(minutePart, format: .number.locale(locale)),
                  (0..<60).contains(minutes)
            else { return nil }
            return Double(whole) + Double(minutes) / 60
        }
        guard let value = try? Double(trimmed, format: .number.locale(locale)), value >= 0
        else { return nil }
        return value
    }

    /// Formats a duration in hours the way the account displays it: `1:24` or `1.40`.
    ///
    /// Both go through locale-aware format styles. The colon form looks
    /// locale-independent but is not — Arabic renders it `١:٢٤` — and the decimal
    /// form needs the locale's decimal separator, `1,40` in French and German.
    func formattedHours(
        _ format: HoursFormat = .hoursMinutes,
        locale: Locale = .autoupdatingCurrent
    ) -> String {
        let hours = max(0, self)
        switch format {
        case .decimal:
            return hours.formatted(.number.precision(.fractionLength(2)).locale(locale))
        case .hoursMinutes:
            // Round to the nearest minute first, so 0.99999 h reads 1:00, not 0:60.
            let totalMinutes = Int((hours * 60).rounded())
            return Duration.seconds(totalMinutes * 60)
                .formatted(.time(pattern: .hourMinute).locale(locale))
        }
    }
}
