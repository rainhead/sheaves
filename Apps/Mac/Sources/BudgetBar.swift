import SheavesCore
import SwiftUI

/// The budget for what is on the clock: how much is left, and how much of it is gone.
///
/// It draws nothing at all unless there is a readable budget for this project. That
/// is the whole design of the feature — an account with no budgets, or a token that
/// may not read the monetary ones, gets no empty frame in place of one, because a
/// budget bar with no budget in it is worse than no bar.
struct BudgetBar: View {
    @Environment(TimeTracker.self) private var tracker

    let entry: TrackedEntry
    var format: HoursFormat

    var body: some View {
        if let budget = tracker.budget(for: entry),
           let remaining = budget.formattedRemaining(format) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(entry.project.name)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 4)
                    Text(budget.isOverBudget ? "\(remaining) over" : "\(remaining) left")
                        .monospacedDigit()
                        .foregroundStyle(budget.isOverBudget ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                        .layoutPriority(1)
                }
                ProgressView(value: budget.fractionUsed)
                    .progressViewStyle(.linear)
                    .tint(tint(for: budget))
            }
            .font(.caption)
            .help(detail(for: budget))
            // Three fragments and a bar read as noise one at a time; spoken as a
            // sentence it is the one thing this row is for.
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(detail(for: budget))
        }
    }

    /// Quiet until it matters: the accent colour while there is room, a warning as the
    /// budget runs out, and red once it is gone.
    private func tint(for budget: ProjectBudget) -> Color {
        if budget.isOverBudget { return .red }
        return budget.fractionUsed >= 0.9 ? .orange : .accentColor
    }

    /// The full picture, for the tooltip and for VoiceOver: what a glance at the bar
    /// only approximates.
    private func detail(for budget: ProjectBudget) -> String {
        var sentence = entry.project.name
        if let spent = budget.formattedSpent(format), let total = budget.formattedBudget(format) {
            sentence += ": \(spent) of \(total) used"
        }
        if let remaining = budget.formattedRemaining(format) {
            sentence += budget.isOverBudget ? ", \(remaining) over budget" : ", \(remaining) left"
        }
        if budget.budgetIsMonthly {
            sentence += ". This budget resets monthly."
        }
        return sentence
    }
}
