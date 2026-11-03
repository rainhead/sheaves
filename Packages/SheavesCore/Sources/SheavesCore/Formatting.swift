import Foundation

// Formatting stays in each shell, which has the platform's locale-aware number
// formatting. The core decides *what* to show; this decides how it reads.

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
        let hours = Swift.max(0, self)
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

public extension BudgetFigure {
    /// The figure as text: a duration in the account's format, or money in the
    /// client's currency, or a bare number when the currency is unknown.
    func formatted(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String {
        switch self {
        case .hours(let hours):
            return hours.formattedHours(format, locale: locale)
        case .money(let amount, nil):
            return amount.formatted(.number.precision(.fractionLength(0)).locale(locale))
        case .money(let amount, let code?):
            return amount.formatted(.currency(code: code).precision(.fractionLength(0)).locale(locale))
        }
    }
}

public extension ProjectBudget {
    /// What is left, unsigned: "8:00", or "$1,200". The caller says whether that is
    /// budget left or budget overspent, because only it knows the language.
    func formattedRemaining(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        remainingFigure()?.formatted(format, locale: locale)
    }

    func formattedSpent(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        spentFigure()?.formatted(format, locale: locale)
    }

    func formattedBudget(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        totalFigure()?.formatted(format, locale: locale)
    }
}
