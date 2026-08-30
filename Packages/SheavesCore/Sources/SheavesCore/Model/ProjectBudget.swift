import Foundation

/// What a project's budget is measured in, per Harvest's `budget_by`.
///
/// The distinction that matters here is hours against money: it decides how the
/// figures are formatted, and money is the case a token may not be allowed to read.
public enum BudgetBasis: String, Codable, Sendable, Hashable {
    /// Hours across the whole project.
    case project
    /// Fees across the whole project.
    case projectCost = "project_cost"
    /// Hours per task.
    case task
    /// Fees per task.
    case taskFees = "task_fees"
    /// Hours per person.
    case person
    /// The project has no budget.
    case none

    /// An unrecognised basis has no unit Sheaves could render, so it is read as no
    /// budget rather than shown in whichever unit happened to be guessed.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        let raw = (try? container.decode(String.self)) ?? ""
        self = BudgetBasis(rawValue: raw) ?? .none
    }

    public var isMonetary: Bool { self == .projectCost || self == .taskFees }
}

/// One project's budget, as `GET /v2/reports/project_budget` reports it.
///
/// Every figure is optional because Harvest omits them in two situations that look
/// identical from here: a project with no budget at all, and a monetary budget that
/// only administrators and managers with the billable-rates permission may read.
/// `hasReadableBudget` collapses both into the one question the UI asks.
public struct ProjectBudget: Identifiable, Codable, Sendable, Hashable {
    public var projectID: Int
    public var projectName: String
    /// Kept so a monetary budget can be joined to its client's currency, which is the
    /// only place Harvest reports one.
    public var clientID: Int?
    public var clientName: String?
    public var isActive: Bool
    public var budgetBy: BudgetBasis
    /// True when the budget resets each month, so the figures below describe this
    /// month rather than the life of the project.
    public var budgetIsMonthly: Bool
    public var budget: Double?
    public var budgetSpent: Double?
    public var budgetRemaining: Double?
    /// Filled in after the fact, from the client record. Absent when the token may not
    /// read clients, and a monetary budget then shows no symbol rather than a wrong one.
    public var currencyCode: String?

    public var id: Int { projectID }

    /// The decoder converts snake_case to camelCase before keys are matched, so these
    /// are the already-converted names rather than what Harvest sends on the wire.
    private enum CodingKeys: String, CodingKey {
        case projectID = "projectId"
        case clientID = "clientId"
        case projectName, clientName, isActive, budgetBy, budgetIsMonthly
        case budget, budgetSpent, budgetRemaining
        // Not on the wire; carried so the cache keeps it across a launch.
        case currencyCode
    }

    public init(
        projectID: Int,
        projectName: String,
        clientID: Int? = nil,
        clientName: String? = nil,
        isActive: Bool = true,
        budgetBy: BudgetBasis,
        budgetIsMonthly: Bool = false,
        budget: Double? = nil,
        budgetSpent: Double? = nil,
        budgetRemaining: Double? = nil,
        currencyCode: String? = nil
    ) {
        self.projectID = projectID
        self.projectName = projectName
        self.clientID = clientID
        self.clientName = clientName
        self.isActive = isActive
        self.budgetBy = budgetBy
        self.budgetIsMonthly = budgetIsMonthly
        self.budget = budget
        self.budgetSpent = budgetSpent
        self.budgetRemaining = budgetRemaining
        self.currencyCode = currencyCode
    }

    /// Whether there is a budget here worth drawing at all.
    ///
    /// A budget of zero counts as none: it is what an account that has switched
    /// budgets off leaves behind, and a bar that is full before any work happens
    /// says nothing.
    public var hasReadableBudget: Bool {
        guard budgetBy != .none, let budget, budget > 0 else { return false }
        return budgetRemaining != nil
    }

    public var isOverBudget: Bool { (budgetRemaining ?? 0) < 0 }

    /// How much of the budget is used, 0…1.
    ///
    /// Clamped, so an overrun draws a full bar rather than one running off the end.
    /// `isOverBudget` is what distinguishes a bar that is full from one that is past
    /// full.
    public var fractionUsed: Double {
        guard let budget, budget > 0 else { return 0 }
        let spent = budgetSpent ?? budget - (budgetRemaining ?? budget)
        return min(1, max(0, spent / budget))
    }

    /// What is left, unsigned: "8:00", or "$1,200". The caller says whether that is
    /// budget left or budget overspent, because only it knows the language.
    public func formattedRemaining(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        budgetRemaining.map { formattedAmount(abs($0), format, locale: locale) }
    }

    public func formattedSpent(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        budgetSpent.map { formattedAmount($0, format, locale: locale) }
    }

    public func formattedBudget(_ format: HoursFormat, locale: Locale = .autoupdatingCurrent) -> String? {
        budget.map { formattedAmount($0, format, locale: locale) }
    }

    /// Harvest names no currency anywhere a budget appears — not on the report, not on
    /// `company`, and not on the client stub inside a project assignment. It comes from
    /// the client record instead, which only an administrator or a manager who may edit
    /// clients can read.
    ///
    /// Without it, the figure is shown as a bare number. Falling back to the locale's
    /// currency would render a EUR budget as `$2,400` for anyone with a US Mac: wrong,
    /// and wrong in a way that looks perfectly reasonable. An unlabelled number is
    /// merely incomplete, and this feature would rather be incomplete than misleading.
    private func formattedAmount(_ amount: Double, _ format: HoursFormat, locale: Locale) -> String {
        guard budgetBy.isMonetary else { return amount.formattedHours(format, locale: locale) }
        guard let currencyCode else {
            return amount.formatted(.number.precision(.fractionLength(0)).locale(locale))
        }
        return amount.formatted(.currency(code: currencyCode).precision(.fractionLength(0)).locale(locale))
    }
}

extension ProjectBudget: PaginatedItem {
    public static var pageKey: String { "results" }
}
