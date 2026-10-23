//! Which budgets a run measures: all of them, or those of one id (`--only`).

use crate::budget::{Budget, is_budget_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wants(Option<String>);

impl Wants {
    /// Every budget.
    pub fn all() -> Wants {
        Wants(None)
    }

    /// The budgets with this id, if there is such an id.
    pub fn only(id: &str) -> Result<Wants, String> {
        if is_budget_id(id) {
            Ok(Wants(Some(id.to_owned())))
        } else {
            Err(format!(
                "`{id}` is not a budget; they are: {}",
                crate::budget::budget_ids().join(", ")
            ))
        }
    }

    /// Whether this budget is to be measured.
    pub fn has(&self, budget: &Budget) -> bool {
        self.0.as_deref().is_none_or(|id| id == budget.id)
    }

    /// Whether any of these budgets is to be measured.
    pub fn any(&self, budgets: &[&Budget]) -> bool {
        budgets.iter().any(|budget| self.has(budget))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{INGEST_DROPPED, INGEST_P99, INGEST_RATE, LOGS_AT, LOGS_COUNT};

    #[test]
    fn all_wants_everything() {
        let wants = Wants::all();

        assert!(wants.has(&LOGS_AT) && wants.has(&INGEST_P99));
    }

    #[test]
    fn only_wants_every_budget_of_the_id_and_nothing_else() {
        let wants = Wants::only("ingest").unwrap();

        assert!(wants.has(&INGEST_RATE) && wants.has(&INGEST_DROPPED) && wants.has(&INGEST_P99));
        assert!(!wants.has(&LOGS_AT));
        assert!(wants.any(&[&LOGS_AT, &INGEST_RATE]));
        assert!(!wants.any(&[&LOGS_AT, &LOGS_COUNT]));
    }

    #[test]
    fn an_unknown_id_says_which_there_are() {
        let error = Wants::only("logs").unwrap_err();

        assert!(error.contains("`logs` is not a budget"), "{error}");
        assert!(
            error.contains("logs_at") && error.contains("rss_10m"),
            "{error}"
        );
    }
}
