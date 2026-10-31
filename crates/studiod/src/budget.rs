//! Strict spending. Every run starts with a reservation that fits under every
//! limit (studio, project and hire over a rolling 24 hours, ticket, run),
//! counting what other live runs have reserved. The runtime gets that amount
//! as its own cap, the worker meters live token usage against it, and the
//! actual cost is recorded in the ledger when the run ends.

use std::collections::HashMap;
use studio_config::config::ModelPrice;
use studio_core::budget::{Allowance, Limits, Used, allowance};
use studio_core::rule::Hire;
use studio_core::ticket::Ticket;
use studio_store::{SpendTotals, Store, StoreError};

use crate::api::DaySpend;

pub(crate) const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// Money held back for a live run.
#[derive(Clone, Debug)]
pub(crate) struct Reservation {
    pub project: String,
    pub hire: String,
    pub usd: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Reserved {
    pub total: f64,
    pub by_project: HashMap<String, f64>,
    pub by_hire: HashMap<String, f64>,
}

pub(crate) fn reserved(reservations: &HashMap<String, Reservation>, except: &str) -> Reserved {
    let mut sum = Reserved::default();
    for (ticket, r) in reservations {
        if ticket == except {
            continue;
        }
        sum.total += r.usd;
        *sum.by_project.entry(r.project.clone()).or_default() += r.usd;
        *sum.by_hire.entry(r.hire.clone()).or_default() += r.usd;
    }
    sum
}

pub(crate) struct Inputs<'a> {
    pub budget: &'a studio_config::config::BudgetConfig,
    pub project_day: Option<f64>,
    pub ticket: &'a Ticket,
    pub hire: &'a Hire,
}

/// What this ticket may spend on its next run.
pub(crate) fn decide(
    store: &Store,
    inputs: &Inputs,
    reservations: &HashMap<String, Reservation>,
    now_ms: u64,
) -> Result<Allowance, studio_store::StoreError> {
    let spent: SpendTotals = store.spent_since(now_ms.saturating_sub(DAY_MS))?;
    let ticket_id = inputs.ticket.id.to_string();
    let others = reserved(reservations, &ticket_id);
    let project = &inputs.ticket.project.0;
    let hire = &inputs.hire.id.0;
    let per = |map: &std::collections::BTreeMap<String, f64>, key: &str| {
        map.get(key).copied().unwrap_or(0.0)
    };
    // The studio's cap covers the whole ticket; a hire's own cap covers only
    // that hire's runs on it, so a reviewer's small cap still lets it review
    // work that cost more to build.
    let limits = Limits {
        studio_day: inputs.budget.daily_usd,
        project_day: inputs.project_day,
        hire_day: inputs.hire.daily_usd,
        ticket: Some(inputs.budget.ticket_usd),
        hire_ticket: inputs.hire.budget_per_ticket_usd,
        run: inputs.budget.run_usd,
        min_run: inputs.budget.min_run_usd,
    };
    let used = Used {
        studio_day: spent.total + others.total,
        project_day: per(&spent.by_project, project)
            + others.by_project.get(project).copied().unwrap_or(0.0),
        hire_day: per(&spent.by_hire, hire) + others.by_hire.get(hire).copied().unwrap_or(0.0),
        ticket: store.ticket_spent(&inputs.ticket.id)?,
        hire_ticket: store.ticket_hire_spent(&inputs.ticket.id, hire)?,
    };
    Ok(allowance(&limits, &used))
}

/// True when `usd` more fits under every limit that has one, given what each
/// scope has used. Each pair is a limit and what is spent or reserved against it.
pub(crate) fn fits(usd: f64, scopes: &[(Option<f64>, f64)]) -> bool {
    scopes
        .iter()
        .all(|(limit, used)| limit.is_none_or(|l| used + usd <= l))
}

/// Whether a small extra call of `usd` fits under the studio, project and hire
/// daily limits, counting spend and every live reservation.
pub(crate) fn has_headroom(
    store: &Store,
    budget: &studio_config::config::BudgetConfig,
    project: (&str, Option<f64>),
    hire: &Hire,
    reservations: &HashMap<String, Reservation>,
    now_ms: u64,
    usd: f64,
) -> Result<bool, studio_store::StoreError> {
    let spent: SpendTotals = store.spent_since(now_ms.saturating_sub(DAY_MS))?;
    let held = reserved(reservations, "");
    let (name, project_limit) = project;
    let of = |map: &std::collections::BTreeMap<String, f64>, key: &str| {
        map.get(key).copied().unwrap_or(0.0)
    };
    Ok(fits(
        usd,
        &[
            (Some(budget.daily_usd), spent.total + held.total),
            (
                project_limit,
                of(&spent.by_project, name) + held.by_project.get(name).copied().unwrap_or(0.0),
            ),
            (
                hire.daily_usd,
                of(&spent.by_hire, &hire.id.0)
                    + held.by_hire.get(&hire.id.0).copied().unwrap_or(0.0),
            ),
        ],
    ))
}

/// Spend on each of the last `days` calendar days in this machine's time
/// zone, oldest first; the last is the day holding `now_ms`. At most 90.
pub(crate) fn by_day(store: &Store, now_ms: u64, days: u32) -> Result<Vec<DaySpend>, StoreError> {
    let days = store.spend_by_local_day(now_ms, days.min(90))?;
    Ok(days
        .into_iter()
        .map(|(date, start_ms, usd)| DaySpend {
            date,
            start_ms,
            usd,
        })
        .collect())
}

/// Counts a run's spend from token usage as it happens.
pub(crate) struct Meter {
    price: Option<ModelPrice>,
    by_message: HashMap<String, (u64, u64, u64, u64)>,
}

impl Meter {
    pub(crate) fn new(price: Option<ModelPrice>) -> Meter {
        Meter {
            price,
            by_message: HashMap::new(),
        }
    }

    pub(crate) fn has_price(&self) -> bool {
        self.price.is_some()
    }

    pub(crate) fn update(&mut self, message_id: &str, tokens: (u64, u64, u64, u64)) {
        self.by_message.insert(message_id.to_string(), tokens);
    }

    pub(crate) fn tokens(&self) -> (u64, u64) {
        self.by_message
            .values()
            .fold((0, 0), |(i, o), t| (i + t.0 + t.2 + t.3, o + t.1))
    }

    /// Estimated USD so far; zero without a price.
    pub(crate) fn usd(&self) -> f64 {
        let Some(p) = &self.price else {
            return 0.0;
        };
        self.by_message
            .values()
            .map(|(input, output, cache_read, cache_write)| {
                (*input as f64 * p.input_per_mtok
                    + *output as f64 * p.output_per_mtok
                    + *cache_read as f64 * p.cache_read_per_mtok
                    + *cache_write as f64 * p.cache_write_per_mtok)
                    / 1_000_000.0
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn the_meter_counts_each_message_once() {
        let mut meter = Meter::new(Some(ModelPrice {
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
            cache_read_per_mtok: 0.1,
            cache_write_per_mtok: 1.25,
        }));
        meter.update("m1", (1_000_000, 0, 0, 0));
        meter.update("m1", (1_000_000, 200_000, 0, 0));
        meter.update("m2", (0, 0, 1_000_000, 0));
        assert!((meter.usd() - (1.0 + 1.0 + 0.1)).abs() < 1e-9);
        assert_eq!(meter.tokens(), (2_000_000, 200_000));
        assert_eq!(Meter::new(None).usd(), 0.0);
    }

    #[test]
    fn a_small_call_needs_room_under_every_limit() {
        assert!(fits(0.05, &[(Some(1.0), 0.5), (None, 99.0)]));
        assert!(fits(0.05, &[(Some(1.0), 0.95)]));
        assert!(!fits(0.05, &[(Some(1.0), 0.96)]));
        assert!(!fits(0.05, &[(Some(1.0), 0.0), (Some(0.01), 0.0)]));
    }

    #[test]
    fn headroom_counts_reservations() {
        let store = Store::open_in_memory().unwrap();
        let hire = Hire {
            id: studio_core::ids::HireId::new("approver"),
            name: "Approver".into(),
            role: "approver".into(),
            runtime: "fake".into(),
            model: "m".into(),
            escalate: Vec::new(),
            effort: None,
            budget_per_ticket_usd: None,
            daily_usd: None,
            tools: Vec::new(),
            user_mcp: false,
            prompt: String::new(),
        };
        let budget = studio_config::config::BudgetConfig {
            daily_usd: 1.0,
            ..Default::default()
        };
        let mut held = HashMap::new();
        assert!(has_headroom(&store, &budget, ("a", None), &hire, &held, 1_000, 0.05).unwrap());
        held.insert(
            "A-1".to_string(),
            Reservation {
                project: "a".into(),
                hire: "dev".into(),
                usd: 0.97,
            },
        );
        assert!(!has_headroom(&store, &budget, ("a", None), &hire, &held, 1_000, 0.05).unwrap());
    }

    #[test]
    fn spend_falls_on_its_calendar_day() {
        let mut store = Store::open_in_memory().unwrap();
        let now = 10 * DAY_MS + 12 * 3_600_000;
        let days = by_day(&store, now, 3).unwrap();
        let today = days[2].start_ms;
        let spend = |at_ms: u64, usd: f64| studio_store::Spend {
            at_ms,
            ticket_id: studio_core::ids::TicketId::new("A-1"),
            project: "a".into(),
            hire: "dev".into(),
            runtime: "fake".into(),
            model: "m".into(),
            session: None,
            usd,
            reported: true,
            input_tokens: 0,
            output_tokens: 0,
        };
        for (at, usd) in [(now, 1.0), (today, 2.0), (today - 1, 4.0)] {
            store.record_spend(&spend(at, usd)).unwrap();
        }
        let days = by_day(&store, now, 3).unwrap();
        let usd: Vec<f64> = days.iter().map(|d| d.usd).collect();
        assert_eq!(usd, [0.0, 4.0, 3.0]);
        assert!(days[0].date < days[1].date && days[1].date < days[2].date);
        assert_eq!(days[2].date.len(), "1970-01-11".len());
        assert!(by_day(&store, now, 0).unwrap().is_empty());
        assert_eq!(by_day(&store, now, 500).unwrap().len(), 90);
    }

    #[test]
    fn other_runs_reservations_count_but_your_own_does_not() {
        let mut map = HashMap::new();
        map.insert(
            "A-1".to_string(),
            Reservation {
                project: "a".into(),
                hire: "dev".into(),
                usd: 1.0,
            },
        );
        map.insert(
            "B-1".to_string(),
            Reservation {
                project: "b".into(),
                hire: "dev".into(),
                usd: 2.0,
            },
        );
        let r = reserved(&map, "A-1");
        assert_eq!(r.total, 2.0);
        assert_eq!(r.by_hire["dev"], 2.0);
        assert!(!r.by_project.contains_key("a"));
    }
}
