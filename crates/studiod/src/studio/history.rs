//! What has happened and what it cost: spend, estimates, trophies and the
//! projects' checks.

use super::{StopReason, Studio, StudioError, lock};
use crate::api::{CheckResult, Estimate, HireDay, ProjectCheck, Response};
use crate::budget::DAY_MS;
use crate::trophies::FirstMerge;
use crate::{now_ms, today, trophies};
use std::collections::{BTreeMap, HashMap};
use studio_config::ProjectConfig;
use studio_config::config::ModelPrice;
use studio_core::quote::{QUOTE_SAMPLES, Quote, quote};
use studio_core::rule::Hire;
use studio_core::ticket::{Ticket, TicketState};
use studio_store::Spend;

impl Studio {
    pub(crate) fn price(&self, model: &str) -> Option<ModelPrice> {
        self.config().prices.get(model).cloned()
    }

    /// The model a session last ran on.
    pub(crate) fn session_model(&self, session: &str) -> Result<Option<String>, StudioError> {
        Ok(lock(&self.store).session_model(session)?)
    }

    /// What the hire's recent landed tickets cost and took, if there are any.
    pub(crate) fn quote(&self, hire: &str) -> Result<Option<Quote>, StudioError> {
        let samples = lock(&self.store).landed_samples(hire, QUOTE_SAMPLES)?;
        Ok(quote(&samples))
    }

    /// What the hire may still spend on the ticket: under the ticket's cap,
    /// and under the hire's own cap for its runs on it.
    pub(crate) fn ticket_left(&self, ticket: &Ticket, hire: &Hire) -> Result<f64, StudioError> {
        let store = lock(&self.store);
        let left = self.config().budget.ticket_usd - store.ticket_spent(&ticket.id)?;
        Ok(match hire.budget_per_ticket_usd {
            Some(cap) => left.min(cap - store.ticket_hire_spent(&ticket.id, &hire.id.0)?),
            None => left,
        })
    }

    /// A quote and expected finish for every open ticket. A running ticket
    /// finishes a median duration after it started, and not sooner than a
    /// minute from now; a waiting one a median duration from now.
    pub(super) fn estimates(&self) -> Result<Vec<Estimate>, StudioError> {
        let now = now_ms();
        let mut quotes: HashMap<String, Option<Quote>> = HashMap::new();
        let mut out = Vec::new();
        let tickets = lock(&self.store).tickets()?;
        for ticket in tickets {
            use TicketState::*;
            if matches!(ticket.state, Landed | Cancelled | Failed { .. } | Ready) {
                continue;
            }
            let hire = ticket.hire.0.clone();
            if !quotes.contains_key(&hire) {
                quotes.insert(hire.clone(), self.quote(&hire)?);
            }
            let eta_ms = quotes[&hire].as_ref().map(|q| {
                let median = (q.minutes * 60_000.0) as u64;
                let running = matches!(ticket.state, Claimed | Working | Checking | Reviewing);
                let started = if running {
                    lock(&self.store).started_at(&ticket.id).ok().flatten()
                } else {
                    None
                };
                match started {
                    Some(at) => (at + median).max(now + 60_000),
                    None => now + median,
                }
            });
            out.push(Estimate {
                ticket: ticket.id.to_string(),
                quote_usd: quotes[&hire].as_ref().map(|q| q.usd),
                eta_ms,
            });
        }
        Ok(out)
    }

    pub(crate) fn session_spent(&self, session: &str) -> Result<f64, StudioError> {
        Ok(lock(&self.store).session_spent(session)?)
    }

    /// Records a run's cost, then stops every live run under a 24-hour limit
    /// that is now used up.
    pub(crate) fn record_spend(&self, spend: &Spend) -> Result<(), StudioError> {
        lock(&self.store).record_spend(spend)?;
        lock(&self.metered).remove(&spend.ticket_id.to_string());
        let spent = lock(&self.store).spent_since(now_ms().saturating_sub(DAY_MS))?;
        let budget = &self.config().budget;
        let over = |limit: Option<f64>, used: f64| limit.is_some_and(|l| used >= l);
        let reservations = lock(&self.reservations).clone();
        let active = lock(&self.active);
        for (ticket, r) in &reservations {
            let project_limit = self
                .config()
                .projects
                .iter()
                .find(|p| p.name == r.project)
                .and_then(|p| p.daily_usd);
            let hire_limit = lock(&self.hires).get(&r.hire).and_then(|h| h.daily_usd);
            let breached = spent.total >= budget.daily_usd
                || over(
                    project_limit,
                    spent.by_project.get(&r.project).copied().unwrap_or(0.0),
                )
                || over(
                    hire_limit,
                    spent.by_hire.get(&r.hire).copied().unwrap_or(0.0),
                );
            if breached && let Some(stop) = active.get(ticket) {
                stop.request(StopReason::Budget);
            }
        }
        Ok(())
    }

    pub(super) fn first_merge_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("first_merge.json")
    }

    /// Notes the first goal merged through the studio, once. The history
    /// doesn't say when a merge happened, so this is where the date comes from;
    /// a studio that merged goals before it kept this note never gets one.
    pub(super) fn remember_first_merge(&self, goal: &str, into: &str) {
        let path = self.first_merge_path();
        if path.exists() {
            return;
        }
        let first = FirstMerge {
            at_ms: now_ms(),
            goal: goal.to_string(),
            into: into.to_string(),
        };
        let written = serde_json::to_string(&first)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        if let Err(err) = written {
            eprintln!("could not note the first merge: {err}");
        }
    }

    fn first_merge(&self) -> Option<FirstMerge> {
        let text = std::fs::read_to_string(self.first_merge_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(super) fn trophies(&self) -> Result<Response, StudioError> {
        let (landings, today) = {
            let store = lock(&self.store);
            (store.landings()?, store.local_date(now_ms())?)
        };
        Ok(Response::ok(trophies::build(
            &landings,
            &today,
            self.first_merge().as_ref(),
        )))
    }

    pub(super) fn project_checks(&self) -> Result<Response, StudioError> {
        let (tickets, latest) = {
            let store = lock(&self.store);
            (store.tickets()?, store.latest_checks()?)
        };
        let checks: Vec<ProjectCheck> =
            self.config()
                .projects
                .iter()
                .map(|project| ProjectCheck {
                    project: project.name.clone(),
                    verify: ProjectConfig::load(&project.path)
                        .ok()
                        .flatten()
                        .map(|config| config.verify)
                        .unwrap_or_default(),
                    running: tickets
                        .iter()
                        .filter(|t| t.project.0 == project.name && t.state == TicketState::Checking)
                        .map(|t| t.id.to_string())
                        .collect(),
                    latest: latest.iter().find(|c| c.project == project.name).map(|c| {
                        CheckResult {
                            ticket: c.ticket_id.to_string(),
                            passed: c.passed,
                            at_ms: c.at_ms,
                        }
                    }),
                })
                .collect();
        Ok(Response::ok(checks))
    }

    /// What each hire did in the last 24 hours.
    pub(super) fn hires_today(&self) -> Result<Response, StudioError> {
        let since = now_ms().saturating_sub(DAY_MS);
        let (tickets, events) = {
            let store = lock(&self.store);
            (
                store.tickets()?,
                store.events_from(since, usize::MAX, |_| true)?,
            )
        };
        let mut days: BTreeMap<String, HireDay> = BTreeMap::new();
        today::from_events(&tickets, &events, &mut days);
        let notes = today::notes_since(&self.paths.data_dir.join("runs"), since);
        let approver = self.first_with_role("approver").map(|h| h.id.0);
        let router = self.first_with_role("router").map(|h| h.id.0);
        today::from_notes(&notes, approver.as_deref(), router.as_deref(), &mut days);
        Ok(Response::ok(days))
    }
}
