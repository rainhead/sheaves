import Foundation
import Testing
@testable import SheavesCore

@Suite("Project budgets")
struct BudgetTests {
    private let english = Locale(identifier: "en_US")

    @Test("reads the budget report, and knows which rows have nothing in them")
    func decodesReport() async throws {
        let client = HarvestClient(
            credentials: Fixture.credentials,
            transport: StubTransport(body: Fixture.projectBudgetPage),
            backoffScale: 0
        )

        let budgets = try await client.projectBudgets()

        #expect(budgets.count == 3)
        let hours = try #require(budgets.first { $0.projectID == 14308069 })
        #expect(hours.projectName == "Online Store - Phase 1")
        #expect(hours.budgetBy == .project)
        #expect(hours.budget == 40)
        #expect(hours.budgetSpent == 32)
        #expect(hours.budgetRemaining == 8)
        #expect(hours.hasReadableBudget)

        // `budget_by: none` is a project with no budget at all.
        #expect(budgets.first { $0.projectID == 14307913 }?.hasReadableBudget == false)
        // A monetary budget this token may not read comes back with null figures. It
        // is indistinguishable from the case above, and means the same to the UI.
        let unreadable = try #require(budgets.first { $0.projectID == 14307915 })
        #expect(unreadable.budgetBy == .taskFees)
        #expect(unreadable.budgetIsMonthly)
        #expect(unreadable.hasReadableBudget == false)
    }

    @Test("asks the reports endpoint, one page at a time")
    func requestsTheReport() async throws {
        let transport = StubTransport(body: Fixture.projectBudgetPage)
        let client = HarvestClient(credentials: Fixture.credentials, transport: transport, backoffScale: 0)

        _ = try await client.projectBudgets()

        let url = try #require(await transport.request(at: 0).url?.absoluteString)
        #expect(url.contains("/v2/reports/project_budget"))
        #expect(url.contains("page=1"))
    }

    /// Harvest can add a `budget_by` value at any time, and there is no unit Sheaves
    /// could render one in. Reading it as "no budget" hides it; guessing shows a
    /// number in the wrong unit.
    @Test("treats an unrecognised budget basis as no budget")
    func unknownBasis() throws {
        let json = Data("""
        {
          "project_id": 1, "project_name": "P", "client_name": "C", "is_active": true,
          "budget_by": "carrier_pigeons", "budget_is_monthly": false,
          "budget": 40.0, "budget_spent": 1.0, "budget_remaining": 39.0
        }
        """.utf8)

        let budget = try HarvestClient.decoder.decode(ProjectBudget.self, from: json)

        #expect(budget.budgetBy == .none)
        #expect(budget.hasReadableBudget == false)
    }

    @Test("a budget of zero is not a budget")
    func zeroBudget() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .project,
            budget: 0, budgetSpent: 0, budgetRemaining: 0
        )
        #expect(budget.hasReadableBudget == false)
    }

    @Test("formats an hours budget the way the account shows durations")
    func formatsHours() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .project,
            budget: 40, budgetSpent: 32, budgetRemaining: 8
        )

        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "8:00")
        #expect(budget.formattedSpent(.hoursMinutes, locale: english) == "32:00")
        #expect(budget.formattedBudget(.hoursMinutes, locale: english) == "40:00")
        #expect(budget.formattedRemaining(.decimal, locale: english) == "8.00")
        #expect(budget.fractionUsed == 0.8)
        #expect(budget.isOverBudget == false)
    }

    @Test("formats a monetary budget in the client's currency")
    func formatsMoney() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .projectCost,
            budget: 10000, budgetSpent: 7600, budgetRemaining: 2400,
            currencyCode: "USD"
        )

        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "$2,400")
        #expect(budget.formattedBudget(.hoursMinutes, locale: english) == "$10,000")
    }

    /// The bug this exists to prevent: Harvest names no currency anywhere a budget
    /// appears, so formatting money in the device's locale renders a European client's
    /// budget in dollars for anyone with a US Mac — wrong, and wrong in a way that
    /// looks entirely reasonable.
    @Test("uses the client's currency, not the device's")
    func honoursForeignCurrency() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .projectCost,
            budget: 10000, budgetSpent: 7600, budgetRemaining: 2400,
            currencyCode: "EUR"
        )

        let shown = budget.formattedRemaining(.hoursMinutes, locale: english)
        #expect(shown?.contains("€") == true)
        #expect(shown?.contains("$") == false)
    }

    /// A token that may not read clients cannot learn the currency. An unlabelled
    /// number is incomplete; a guessed symbol would be misleading, which is worse.
    @Test("shows a bare number when the currency is unknown")
    func omitsUnknownCurrency() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .projectCost,
            budget: 10000, budgetSpent: 7600, budgetRemaining: 2400
        )

        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "2,400")
    }

    @Test("an hours budget never consults a currency")
    func hoursIgnoreCurrency() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .project,
            budget: 40, budgetSpent: 32, budgetRemaining: 8, currencyCode: "EUR"
        )

        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "8:00")
    }

    /// Overspend is the one number worth showing loudest, and the hours formatter
    /// clamps negatives to zero — so it must never see the signed value.
    @Test("reports an overrun rather than clamping it to nothing")
    func overBudget() {
        let budget = ProjectBudget(
            projectID: 1, projectName: "P", budgetBy: .project,
            budget: 40, budgetSpent: 42.5, budgetRemaining: -2.5
        )

        #expect(budget.isOverBudget)
        #expect(budget.formattedRemaining(.hoursMinutes, locale: english) == "2:30")
        // The bar fills, it does not run off the end.
        #expect(budget.fractionUsed == 1)
    }
}
