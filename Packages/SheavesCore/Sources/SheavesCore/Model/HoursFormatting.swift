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
    func formattedHours(_ format: HoursFormat = .hoursMinutes) -> String {
        let hours = max(0, self)
        switch format {
        case .decimal:
            return String(format: "%.2f", hours)
        case .hoursMinutes:
            // Round to the nearest minute first, so 0.99999 h reads 1:00, not 0:60.
            let totalMinutes = Int((hours * 60).rounded())
            return String(format: "%d:%02d", totalMinutes / 60, totalMinutes % 60)
        }
    }

    /// `1:24:07` — used while a timer runs, where seconds ticking is the point.
    func formattedHoursWithSeconds() -> String {
        let totalSeconds = Int((max(0, self) * 3600).rounded())
        return String(
            format: "%d:%02d:%02d",
            totalSeconds / 3600,
            (totalSeconds % 3600) / 60,
            totalSeconds % 60
        )
    }
}
