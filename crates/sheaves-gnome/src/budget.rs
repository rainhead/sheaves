//! How a project's budget reads on GNOME. The core says whether there is a
//! budget worth showing and hands over unsigned figures; this writes the words,
//! as the Mac does: "1,007 left", "240 over".

use sheaves_core::budget::ProjectBudget;
use sheaves_core::hours::HoursFormat;

use crate::format;

/// What is left of a budget, or by how much it is overspent.
pub struct Left {
    /// "1,007 left" or "240 over".
    pub text: String,
    pub over: bool,
}

/// The remaining figure, or `None` when the budget has none to show.
pub fn left(budget: &ProjectBudget, hours: HoursFormat) -> Option<Left> {
    let amount = format::figure(&budget.remaining()?, hours);
    let over = budget.is_over_budget();
    Some(Left {
        text: if over {
            format!("{amount} over")
        } else {
            format!("{amount} left")
        },
        over,
    })
}

/// The whole picture in a sentence, for the bar's tooltip and for a screen
/// reader: "Online Store: $3,993 of $5,000 used, $1,007 left".
pub fn detail(project: &str, budget: &ProjectBudget, hours: HoursFormat) -> String {
    let mut sentence = project.to_string();
    if let (Some(spent), Some(total)) = (budget.spent(), budget.total()) {
        sentence.push_str(&format!(
            ": {} of {} used",
            format::figure(&spent, hours),
            format::figure(&total, hours)
        ));
    }
    if let Some(remaining) = budget.remaining() {
        let amount = format::figure(&remaining, hours);
        if budget.is_over_budget() {
            sentence.push_str(&format!(", {amount} over budget"));
        } else {
            sentence.push_str(&format!(", {amount} left"));
        }
    }
    if budget.budget_is_monthly {
        sentence.push_str(". This budget resets monthly.");
    }
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::budget::BudgetBasis;

    fn budget(basis: BudgetBasis, total: f64, spent: f64, remaining: f64) -> ProjectBudget {
        ProjectBudget::new(1, "P", basis, Some(total), Some(spent), Some(remaining))
    }

    #[test]
    fn hours_left() {
        let budget = budget(BudgetBasis::Project, 40.0, 32.0, 8.0);
        let left = left(&budget, HoursFormat::HoursMinutes).unwrap();
        assert_eq!(left.text, "8:00 left");
        assert!(!left.over);
    }

    /// The core hands over the overrun unsigned, so the shell writes "over".
    #[test]
    fn money_over() {
        let mut budget = budget(BudgetBasis::ProjectCost, 5000.0, 5240.0, -240.0);
        budget.currency_code = Some("USD".into());
        let left = left(&budget, HoursFormat::HoursMinutes).unwrap();
        assert_eq!(left.text, "$240 over");
        assert!(left.over);
    }

    #[test]
    fn nothing_to_show_without_a_remaining_figure() {
        let mut budget = budget(BudgetBasis::Project, 40.0, 32.0, 8.0);
        budget.budget_remaining = None;
        assert!(left(&budget, HoursFormat::HoursMinutes).is_none());
    }

    #[test]
    fn the_sentence_for_a_monthly_money_budget() {
        let mut budget = budget(BudgetBasis::ProjectCost, 5000.0, 3993.0, 1007.0);
        budget.currency_code = Some("USD".into());
        budget.budget_is_monthly = true;
        assert_eq!(
            detail("Store", &budget, HoursFormat::HoursMinutes),
            "Store: $3993 of $5000 used, $1007 left. This budget resets monthly."
        );
    }
}
