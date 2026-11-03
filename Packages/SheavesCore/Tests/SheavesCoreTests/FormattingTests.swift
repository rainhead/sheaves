import Foundation
import Testing
@testable import SheavesCore

/// Formatting stays in the shell, so these stay in Swift. What is shown is decided
/// by the core and tested there; these check how it reads.
@Suite("Hours formatting")
struct HoursFormattingTests {
    /// Pinned so the suite does not depend on the machine's region settings.
    private let english = Locale(identifier: "en_US")

    @Test("formats as hours and minutes")
    func hoursMinutes() {
        #expect(1.4.formattedHours(.hoursMinutes, locale: english) == "1:24")
        #expect(0.0.formattedHours(.hoursMinutes, locale: english) == "0:00")
        #expect(10.5.formattedHours(.hoursMinutes, locale: english) == "10:30")
    }

    /// Rounding to minutes before splitting keeps 0:60 from ever appearing.
    @Test("never renders sixty minutes")
    func neverSixtyMinutes() {
        #expect(0.99999.formattedHours(.hoursMinutes, locale: english) == "1:00")
        #expect(1.99999.formattedHours(.hoursMinutes, locale: english) == "2:00")
    }

    @Test("formats as decimal hours")
    func decimal() {
        #expect(1.4.formattedHours(.decimal, locale: english) == "1.40")
        #expect(0.25.formattedHours(.decimal, locale: english) == "0.25")
    }

    @Test("clamps negative durations")
    func negatives() {
        #expect((-1.0).formattedHours(.hoursMinutes, locale: english) == "0:00")
    }

    /// Decimal separators and digits are not universal: a hand-rolled "%.2f" gives a
    /// French user "1.40" where they expect "1,40".
    @Test("respects the reader's locale")
    func respectsLocale() {
        #expect(1.4.formattedHours(.decimal, locale: Locale(identifier: "fr_FR")) == "1,40")
        #expect(1.4.formattedHours(.decimal, locale: Locale(identifier: "de_DE")) == "1,40")
        // Arabic-Indic digits, which the colon form needs too.
        #expect(1.4.formattedHours(.hoursMinutes, locale: Locale(identifier: "ar_EG")) == "١:٢٤")
    }

    /// The edit field prefills with the formatted duration, so whatever this side
    /// displays must read back through the core's parser as the value it displayed.
    /// The one test that crosses the boundary in both directions.
    @Test("round-trips what the app displays through the core's parser")
    func roundTripsDisplay() {
        for identifier in ["en_US", "fr_FR", "ar_EG"] {
            let locale = Locale(identifier: identifier)
            for hours in [0.0, 0.25, 1.4, 10.5] {
                let shown = hours.formattedHours(.hoursMinutes, locale: locale)
                #expect(Double.hours(parsing: shown, locale: locale) == hours, "\(identifier) \(shown)")
            }
            let decimal = 1.4.formattedHours(.decimal, locale: locale)
            #expect(Double.hours(parsing: decimal, locale: locale) == 1.4, "\(identifier) \(decimal)")
        }
    }
}

@Suite("Budget formatting")
struct BudgetFormattingTests {
    private let english = Locale(identifier: "en_US")

    private func budget(
        _ basis: BudgetBasis, _ total: Double, _ spent: Double, _ remaining: Double,
        currency: String? = nil
    ) -> ProjectBudget {
        ProjectBudget(
            projectId: 1, projectName: "P", clientId: nil, clientName: nil, isActive: true,
            budgetBy: basis, budgetIsMonthly: false, budget: total, budgetSpent: spent,
            budgetRemaining: remaining, currencyCode: currency
        )
    }

    @Test("formats an hours budget the way the account shows durations")
    func formatsHours() {
        let budget = budget(.project, 40, 32, 8)
        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "8:00")
        #expect(budget.formattedSpent(.hoursMinutes, locale: english) == "32:00")
        #expect(budget.formattedBudget(.hoursMinutes, locale: english) == "40:00")
        #expect(budget.formattedRemaining(.decimal, locale: english) == "8.00")
    }

    @Test("formats a monetary budget in the client's currency")
    func formatsMoney() {
        let budget = budget(.projectCost, 10000, 7600, 2400, currency: "USD")
        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "$2,400")
        #expect(budget.formattedBudget(.hoursMinutes, locale: english) == "$10,000")
    }

    @Test("uses the client's currency, not the device's")
    func honoursForeignCurrency() {
        let shown = budget(.projectCost, 10000, 7600, 2400, currency: "EUR")
            .formattedRemaining(.hoursMinutes, locale: english)
        #expect(shown?.contains("€") == true)
        #expect(shown?.contains("$") == false)
    }

    @Test("shows a bare number when the currency is unknown")
    func omitsUnknownCurrency() {
        #expect(budget(.projectCost, 10000, 7600, 2400).formattedRemaining(.hoursMinutes, locale: english) == "2,400")
    }

    /// Overspend is the one number worth showing loudest, and the hours formatter
    /// clamps negatives to zero, so it must never see the signed value.
    @Test("shows an overrun as the amount over")
    func overBudget() {
        #expect(budget(.project, 40, 42.5, -2.5).formattedRemaining(.hoursMinutes, locale: english) == "2:30")
    }
}
