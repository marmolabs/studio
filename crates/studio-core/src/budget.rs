//! Budget arithmetic. Pure: callers pass the limits, what has been spent, and
//! what other live runs have reserved. A run only starts with an allowance that
//! fits under every limit at once.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// The whole studio over a rolling 24 hours.
    Studio,
    /// One project over a rolling 24 hours.
    Project,
    /// One hire over a rolling 24 hours.
    Hire,
    /// One ticket across all its runs.
    Ticket,
    /// One hire's runs on one ticket.
    HireTicket,
    /// A single run.
    Run,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    pub studio_day: f64,
    pub project_day: Option<f64>,
    pub hire_day: Option<f64>,
    pub ticket: Option<f64>,
    pub hire_ticket: Option<f64>,
    pub run: Option<f64>,
    /// Runs never start with less than this.
    pub min_run: f64,
}

/// Spent plus reserved by other live runs, per scope.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Used {
    pub studio_day: f64,
    pub project_day: f64,
    pub hire_day: f64,
    pub ticket: f64,
    pub hire_ticket: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "allowance", rename_all = "snake_case")]
pub enum Allowance {
    /// Start the run with at most `usd`; `bound_by` is the tightest limit.
    Run { usd: f64, bound_by: BudgetScope },
    /// Don't start: `scope` has less than the minimum left.
    Refused {
        scope: BudgetScope,
        limit: f64,
        used: f64,
    },
}

pub fn allowance(limits: &Limits, used: &Used) -> Allowance {
    let mut scopes = vec![(BudgetScope::Studio, limits.studio_day, used.studio_day)];
    if let Some(limit) = limits.project_day {
        scopes.push((BudgetScope::Project, limit, used.project_day));
    }
    if let Some(limit) = limits.hire_day {
        scopes.push((BudgetScope::Hire, limit, used.hire_day));
    }
    if let Some(limit) = limits.ticket {
        scopes.push((BudgetScope::Ticket, limit, used.ticket));
    }
    if let Some(limit) = limits.hire_ticket {
        scopes.push((BudgetScope::HireTicket, limit, used.hire_ticket));
    }
    if let Some(limit) = limits.run {
        scopes.push((BudgetScope::Run, limit, 0.0));
    }
    let mut tightest = scopes[0];
    for scope in &scopes[1..] {
        if scope.1 - scope.2 < tightest.1 - tightest.2 {
            tightest = *scope;
        }
    }
    let (scope, limit, used) = tightest;
    let left = limit - used;
    if !funded(left, limits.min_run) {
        return Allowance::Refused { scope, limit, used };
    }
    Allowance::Run {
        usd: left,
        bound_by: scope,
    }
}

/// True when `left` can pay for a run. NaN, infinities, zero and a negative
/// minimum never let a run through.
pub fn funded(left: f64, min_run: f64) -> bool {
    left.is_finite() && left > 0.0 && left >= min_run.max(0.0)
}

/// This run's spend from a runtime's report. Some runtimes report the session's
/// running total, so earlier runs of the same session are subtracted.
pub fn run_spend(reported: f64, session_recorded: f64, cumulative: bool) -> f64 {
    let spend = if cumulative {
        reported - session_recorded
    } else {
        reported
    };
    if spend.is_finite() {
        spend.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            studio_day: 10.0,
            project_day: Some(4.0),
            hire_day: Some(6.0),
            ticket: Some(2.0),
            hire_ticket: None,
            run: None,
            min_run: 0.05,
        }
    }

    #[test]
    fn the_tightest_limit_wins() {
        let used = Used {
            studio_day: 1.0,
            project_day: 1.0,
            hire_day: 1.0,
            ticket: 0.5,
            hire_ticket: 0.0,
        };
        assert_eq!(
            allowance(&limits(), &used),
            Allowance::Run {
                usd: 1.5,
                bound_by: BudgetScope::Ticket
            }
        );
        let used = Used {
            studio_day: 9.5,
            ..used
        };
        assert_eq!(
            allowance(&limits(), &used),
            Allowance::Run {
                usd: 0.5,
                bound_by: BudgetScope::Studio
            }
        );
    }

    #[test]
    fn a_hire_cap_counts_only_that_hires_runs_on_the_ticket() {
        // The engineer spent $1.22; the reviewer's $0.50 cap is still untouched.
        let limits = Limits {
            hire_ticket: Some(0.5),
            ..limits()
        };
        let used = Used {
            ticket: 1.22,
            hire_ticket: 0.0,
            ..Used::default()
        };
        assert_eq!(
            allowance(&limits, &used),
            Allowance::Run {
                usd: 0.5,
                bound_by: BudgetScope::HireTicket
            }
        );
        let used = Used {
            hire_ticket: 0.48,
            ..used
        };
        assert!(matches!(
            allowance(&limits, &used),
            Allowance::Refused {
                scope: BudgetScope::HireTicket,
                ..
            }
        ));
    }

    #[test]
    fn a_run_cap_applies_on_its_own() {
        let limits = Limits {
            run: Some(0.25),
            ..limits()
        };
        assert_eq!(
            allowance(&limits, &Used::default()),
            Allowance::Run {
                usd: 0.25,
                bound_by: BudgetScope::Run
            }
        );
    }

    #[test]
    fn refuses_below_the_minimum_and_when_overspent() {
        let used = Used {
            project_day: 3.97,
            ..Used::default()
        };
        assert_eq!(
            allowance(&limits(), &used),
            Allowance::Refused {
                scope: BudgetScope::Project,
                limit: 4.0,
                used: 3.97
            }
        );
        let used = Used {
            studio_day: 12.0,
            ..Used::default()
        };
        assert!(matches!(
            allowance(&limits(), &used),
            Allowance::Refused {
                scope: BudgetScope::Studio,
                ..
            }
        ));
    }

    #[test]
    fn zero_or_broken_limits_refuse() {
        let zero = Limits {
            studio_day: 0.0,
            ..Limits::default()
        };
        assert!(matches!(
            allowance(&zero, &Used::default()),
            Allowance::Refused { .. }
        ));
        let nan = Limits {
            studio_day: f64::NAN,
            ..Limits::default()
        };
        assert!(matches!(
            allowance(&nan, &Used::default()),
            Allowance::Refused { .. }
        ));
        let negative_min = Limits {
            studio_day: 1.0,
            min_run: -5.0,
            ..Limits::default()
        };
        let used = Used {
            studio_day: 1.0,
            ..Used::default()
        };
        assert!(matches!(
            allowance(&negative_min, &used),
            Allowance::Refused { .. }
        ));
    }

    #[test]
    fn cumulative_reports_become_per_run_spend() {
        assert!((run_spend(0.0042, 0.0024, true) - 0.0018).abs() < 1e-12);
        assert_eq!(run_spend(0.5, 0.2, false), 0.5);
        assert_eq!(run_spend(0.1, 0.3, true), 0.0);
        assert_eq!(run_spend(f64::NAN, 0.0, false), 0.0);
    }
}
