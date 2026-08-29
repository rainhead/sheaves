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
