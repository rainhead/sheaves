//! Project budgets, from Harvest's budget report.

use serde::{Deserialize, Deserializer, Serialize};

use crate::models::PaginatedItem;

/// What a project's budget is measured in, per Harvest's `budget_by`.
///
/// The distinction that matters here is hours against money: it decides how the
/// figures are formatted, and money is the case a token may not be allowed to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum BudgetBasis {
    /// Hours across the whole project.
    Project,
    /// Fees across the whole project.
    ProjectCost,
    /// Hours per task.
    Task,
    /// Fees per task.
    TaskFees,
    /// Hours per person.
    Person,
    /// The project has no budget.
    None,
}

impl BudgetBasis {
    pub fn is_monetary(self) -> bool {
        matches!(self, BudgetBasis::ProjectCost | BudgetBasis::TaskFees)
    }
}

/// An unrecognised basis has no unit Sheaves could render, so it is read as no
/// budget rather than shown in whichever unit happened to be guessed.
impl<'de> Deserialize<'de> for BudgetBasis {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
        Ok(match raw.as_str() {
            "project" => BudgetBasis::Project,
            "project_cost" => BudgetBasis::ProjectCost,
            "task" => BudgetBasis::Task,
            "task_fees" => BudgetBasis::TaskFees,
            "person" => BudgetBasis::Person,
            _ => BudgetBasis::None,
        })
    }
}

/// One project's budget, as `GET /v2/reports/project_budget` reports it.
///
/// Every figure is optional because Harvest omits them in two situations that look
/// identical from here: a project with no budget at all, and a monetary budget that
/// only administrators and managers with the billable-rates permission may read.
/// `has_readable_budget` collapses both into the one question the UI asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct ProjectBudget {
    pub project_id: i64,
    pub project_name: String,
    /// Kept so a monetary budget can be joined to its client's currency, which is
    /// the only place Harvest reports one.
    pub client_id: Option<i64>,
    pub client_name: Option<String>,
    pub is_active: bool,
    pub budget_by: BudgetBasis,
    /// True when the budget resets each month, so the figures describe this month
    /// rather than the life of the project.
    pub budget_is_monthly: bool,
    pub budget: Option<f64>,
    pub budget_spent: Option<f64>,
    pub budget_remaining: Option<f64>,
    /// Filled in after the fact, from the client record. Absent when the token may
    /// not read clients, and a monetary budget then shows no symbol rather than a
    /// wrong one. Not on the wire; carried so the cache keeps it across a launch.
    #[serde(default)]
    pub currency_code: Option<String>,
}

/// A budget figure ready for a shell to format: unsigned, and in the unit it must be
/// shown in.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum BudgetFigure {
    /// A duration in hours, formatted the way the account shows durations.
    Hours(f64),
    /// An amount of money. `currency` is `None` when the client's currency is
    /// unknown, and the amount is then shown as a bare number.
    ///
    /// Harvest names no currency anywhere a budget appears, so falling back to the
    /// locale's currency would render a EUR budget as `$2,400` for anyone with a US
    /// machine: wrong, and wrong in a way that looks perfectly reasonable. An
    /// unlabelled number is merely incomplete.
    Money {
        amount: f64,
        currency: Option<String>,
    },
}

impl ProjectBudget {
    /// A budget with only the fields a test or a demo needs.
    pub fn new(
        project_id: i64,
        project_name: impl Into<String>,
        budget_by: BudgetBasis,
        budget: Option<f64>,
        budget_spent: Option<f64>,
        budget_remaining: Option<f64>,
    ) -> Self {
        Self {
            project_id,
            project_name: project_name.into(),
            client_id: None,
            client_name: None,
            is_active: true,
            budget_by,
            budget_is_monthly: false,
            budget,
            budget_spent,
            budget_remaining,
            currency_code: None,
        }
    }

    /// Whether there is a budget here worth drawing at all.
    ///
    /// A budget of zero counts as none: it is what an account that has switched
    /// budgets off leaves behind, and a bar that is full before any work happens
    /// says nothing.
    pub fn has_readable_budget(&self) -> bool {
        self.budget_by != BudgetBasis::None
            && self.budget.is_some_and(|budget| budget > 0.0)
            && self.budget_remaining.is_some()
    }

    pub fn is_over_budget(&self) -> bool {
        self.budget_remaining.unwrap_or(0.0) < 0.0
    }

    /// How much of the budget is used, from 0 to 1.
    ///
    /// Clamped, so an overrun draws a full bar rather than one running off the end.
    /// `is_over_budget` is what distinguishes a bar that is full from one that is
    /// past full.
    pub fn fraction_used(&self) -> f64 {
        let Some(budget) = self.budget.filter(|&budget| budget > 0.0) else {
            return 0.0;
        };
        let spent = self
            .budget_spent
            .unwrap_or_else(|| budget - self.budget_remaining.unwrap_or(budget));
        (spent / budget).clamp(0.0, 1.0)
    }

    /// What is left, unsigned. The caller says whether that is budget left or
    /// budget overspent, because only it knows the language; the hours formatter
    /// clamps negatives to zero, so it must never see the signed value.
    pub fn remaining(&self) -> Option<BudgetFigure> {
        self.budget_remaining
            .map(|amount| self.figure(amount.abs()))
    }

    pub fn spent(&self) -> Option<BudgetFigure> {
        self.budget_spent.map(|amount| self.figure(amount))
    }

    pub fn total(&self) -> Option<BudgetFigure> {
        self.budget.map(|amount| self.figure(amount))
    }

    fn figure(&self, amount: f64) -> BudgetFigure {
        if self.budget_by.is_monetary() {
            BudgetFigure::Money {
                amount,
                currency: self.currency_code.clone(),
            }
        } else {
            BudgetFigure::Hours(amount)
        }
    }
}

impl PaginatedItem for ProjectBudget {
    const PAGE_KEY: &'static str = "results";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(
        basis: BudgetBasis,
        total: f64,
        spent: f64,
        remaining: f64,
        currency: Option<&str>,
    ) -> ProjectBudget {
        ProjectBudget {
            currency_code: currency.map(str::to_string),
            ..ProjectBudget::new(1, "P", basis, Some(total), Some(spent), Some(remaining))
        }
    }

    /// Harvest can add a `budget_by` value at any time, and there is no unit Sheaves
    /// could render one in. Reading it as "no budget" hides it; guessing shows a
    /// number in the wrong unit.
    #[test]
    fn unknown_basis() {
        let json = r#"{
          "project_id": 1, "project_name": "P", "client_name": "C", "is_active": true,
          "budget_by": "carrier_pigeons", "budget_is_monthly": false,
          "budget": 40.0, "budget_spent": 1.0, "budget_remaining": 39.0
        }"#;
        let budget: ProjectBudget = serde_json::from_str(json).unwrap();
        assert_eq!(budget.budget_by, BudgetBasis::None);
        assert!(!budget.has_readable_budget());
    }

    #[test]
    fn zero_budget() {
        let budget = ProjectBudget::new(
            1,
            "P",
            BudgetBasis::Project,
            Some(0.0),
            Some(0.0),
            Some(0.0),
        );
        assert!(!budget.has_readable_budget());
    }

    /// The figures an hours budget hands its shell. The Swift test also checks the
    /// strings, such as "8:00", which the shells now produce.
    #[test]
    fn formats_hours() {
        let budget = budget(BudgetBasis::Project, 40.0, 32.0, 8.0, None);
        assert_eq!(budget.remaining(), Some(BudgetFigure::Hours(8.0)));
        assert_eq!(budget.spent(), Some(BudgetFigure::Hours(32.0)));
        assert_eq!(budget.total(), Some(BudgetFigure::Hours(40.0)));
        assert_eq!(budget.fraction_used(), 0.8);
        assert!(!budget.is_over_budget());
    }

    #[test]
    fn formats_money() {
        let budget = budget(
            BudgetBasis::ProjectCost,
            10000.0,
            7600.0,
            2400.0,
            Some("USD"),
        );
        let usd = Some("USD".to_string());
        assert_eq!(
            budget.remaining(),
            Some(BudgetFigure::Money {
                amount: 2400.0,
                currency: usd.clone()
            })
        );
        assert_eq!(
            budget.total(),
            Some(BudgetFigure::Money {
                amount: 10000.0,
                currency: usd
            })
        );
    }

    /// The bug this exists to prevent: Harvest names no currency anywhere a budget
    /// appears, so formatting money in the device's locale renders a European
    /// client's budget in dollars for anyone with a US machine.
    #[test]
    fn honours_foreign_currency() {
        let budget = budget(
            BudgetBasis::ProjectCost,
            10000.0,
            7600.0,
            2400.0,
            Some("EUR"),
        );
        assert_eq!(
            budget.remaining(),
            Some(BudgetFigure::Money {
                amount: 2400.0,
                currency: Some("EUR".to_string())
            })
        );
    }

    /// A token that may not read clients cannot learn the currency. An unlabelled
    /// number is incomplete; a guessed symbol would be misleading, which is worse.
    #[test]
    fn omits_unknown_currency() {
        let budget = budget(BudgetBasis::ProjectCost, 10000.0, 7600.0, 2400.0, None);
        assert_eq!(
            budget.remaining(),
            Some(BudgetFigure::Money {
                amount: 2400.0,
                currency: None
            })
        );
    }

    #[test]
    fn hours_ignore_currency() {
        let budget = budget(BudgetBasis::Project, 40.0, 32.0, 8.0, Some("EUR"));
        assert_eq!(budget.remaining(), Some(BudgetFigure::Hours(8.0)));
    }

    /// Overspend is the one number worth showing loudest, and the hours formatter
    /// clamps negatives to zero, so it must never see the signed value.
    #[test]
    fn over_budget() {
        let budget = budget(BudgetBasis::Project, 40.0, 42.5, -2.5, None);
        assert!(budget.is_over_budget());
        assert_eq!(budget.remaining(), Some(BudgetFigure::Hours(2.5)));
        // The bar fills, it does not run off the end.
        assert_eq!(budget.fraction_used(), 1.0);
    }
}
