//! `Request::Office`: everything the office shows, read at one moment. The
//! store is read once under one lock; what the run logs say is read at most
//! every `NOTES_EVERY` ms, and a ready ticket's change size once per attempt.

use super::{Studio, StudioError, lock};
use crate::api::{OFFICE_KINDS, OfficeView, RECENT_EVENTS, Response, RunView, StoredEvent};
use crate::budget::DAY_MS;
use crate::{now_ms, runview, today};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use studio_core::ticket::{Ticket, TicketKind};

/// How long the run logs' notes stand: reading every fresh log is the
/// dearest part of the answer, and the figures they feed move slowly.
const NOTES_EVERY: u64 = 30_000;

impl Studio {
    /// `Request::Status` for a studio with `tickets` and `spent_24h` dollars
    /// spent over the last 24 hours.
    pub(super) fn status(&self, tickets: &[Ticket], spent_24h: f64) -> Value {
        let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
        for ticket in tickets {
            *by_state.entry(ticket.state.kind()).or_default() += 1;
        }
        let goals = tickets
            .iter()
            .filter(|t| t.kind == TicketKind::Plan)
            .count();
        json!({
            "tickets": by_state,
            "goals": goals,
            "rules": self.rule_files(),
            "running": lock(&self.active).len(),
            "spent_24h": spent_24h,
            "daily_usd": self.config().budget.daily_usd,
            "out_of_budget": self.out_of_funds(spent_24h),
            "asks": lock(&self.asks).len(),
            "projects": self.config().projects.iter().map(|p| &p.name).collect::<Vec<_>>(),
            "hires": lock(&self.hires).keys().collect::<Vec<_>>(),
            "max_runs": self.desks(),
            "closed": self.is_closed(),
            "team": self.team(),
        })
    }

    pub(super) fn office(&self) -> Result<Response, StudioError> {
        let now = now_ms();
        let since = now.saturating_sub(DAY_MS);
        let (tickets, day, spent_24h, mut spend) = {
            let store = lock(&self.store);
            (
                store.tickets()?,
                store.events_from(since, usize::MAX, |_| true)?,
                store.spent_since(since)?.total,
                store.spent_by_ticket()?,
            )
        };
        for (ticket, usd) in lock(&self.metered).iter() {
            *spend.entry(ticket.clone()).or_default() += usd;
        }
        let asks = lock(&self.asks).values().map(|a| a.view.clone()).collect();

        let mut events: Vec<StoredEvent> = day
            .iter()
            .filter(|e| OFFICE_KINDS.contains(&e.event.kind()))
            .cloned()
            .collect();
        events.drain(..events.len().saturating_sub(RECENT_EVENTS));

        let runs = self.paths.data_dir.join("runs");
        let mut today = BTreeMap::new();
        today::from_events(&tickets, &day, &mut today);
        let approver = self.first_with_role("approver").map(|h| h.id.0);
        let router = self.first_with_role("router").map(|h| h.id.0);
        let notes = notes(&runs, since, now);
        today::from_notes(&notes, approver.as_deref(), router.as_deref(), &mut today);

        let judges = {
            let judging = lock(&self.judges);
            tickets
                .iter()
                .filter(|t| in_review(t))
                .filter_map(|t| {
                    let id = t.id.to_string();
                    Some((id.clone(), judging.get(&id)?.clone()))
                })
                .collect()
        };
        let sizes = self.sizes(&tickets);
        let (run_views, unreviewed) = run_logs(&runs, &tickets);
        Ok(Response::ok(OfficeView {
            status: self.status(&tickets, spent_24h),
            asks,
            events,
            spend,
            merged: self.merged_among(&tickets),
            sizes,
            today,
            judges,
            runs: run_views,
            unreviewed,
            now,
            tickets,
        }))
    }

    /// Lines added and removed by each ready ticket. A ticket's change only
    /// moves with a new attempt, so git is asked once per attempt.
    fn sizes(&self, tickets: &[Ticket]) -> BTreeMap<String, (u64, u64)> {
        type Key = (PathBuf, String, u32);
        static SEEN: Mutex<BTreeMap<Key, (u64, u64)>> = Mutex::new(BTreeMap::new());
        let mut sizes = BTreeMap::new();
        for ticket in tickets.iter().filter(|t| t.state.kind() == "ready") {
            let id = ticket.id.to_string();
            let key = (self.paths.data_dir.clone(), id.clone(), ticket.attempts);
            let known = lock(&SEEN).get(&key).copied();
            let size = known.or_else(|| {
                let stat = self.stat(&ticket.id).ok().flatten()?;
                let size = totals(&stat)?;
                lock(&SEEN).insert(key, size);
                Some(size)
            });
            if let Some(size) = size {
                sizes.insert(id, size);
            }
        }
        sizes
    }
}

/// What each ticket with a run under way is doing, and which ready tickets
/// went unreviewed, from the run logs in `runs`.
fn run_logs(runs: &Path, tickets: &[Ticket]) -> (BTreeMap<String, RunView>, Vec<String>) {
    let mut views = BTreeMap::new();
    let mut unreviewed = Vec::new();
    for ticket in tickets {
        let id = ticket.id.to_string();
        match ticket.state.kind() {
            kind @ ("claimed" | "working" | "checking" | "reviewing" | "asking") => {
                views.insert(id.clone(), runview::run_of(runs, &id, kind == "checking"));
            }
            "ready" if runview::unreviewed(runs, &id) => unreviewed.push(id),
            _ => {}
        }
    }
    (views, unreviewed)
}

/// Whether the ticket is in the review room: reviewing, or asking or held
/// from review, so a critic's question waits there.
fn in_review(ticket: &Ticket) -> bool {
    let stopped = matches!(ticket.state.kind(), "asking" | "held");
    let kind = match &ticket.held_from {
        Some(from) if stopped => from.kind(),
        _ => ticket.state.kind(),
    };
    kind == "reviewing"
}

/// Lines added and removed, from the summary line of `git diff --stat`, such
/// as "2 files changed, 6 insertions(+), 1 deletion(-)".
fn totals(stat: &str) -> Option<(u64, u64)> {
    let summary = stat.lines().rev().find(|l| l.contains(" changed"))?;
    let count = |word: &str| {
        summary
            .split(", ")
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next()?.parse().ok())
            .unwrap_or(0)
    };
    Some((count("insertion"), count("deletion")))
}

/// The studio's notes of the last 24 hours from the run logs, as of at most
/// `NOTES_EVERY` ms ago.
fn notes(runs: &Path, since: u64, now: u64) -> Vec<String> {
    static SEEN: Mutex<BTreeMap<PathBuf, (u64, Vec<String>)>> = Mutex::new(BTreeMap::new());
    if let Some((at, notes)) = lock(&SEEN).get(runs)
        && now.saturating_sub(*at) < NOTES_EVERY
    {
        return notes.clone();
    }
    let notes = today::notes_since(runs, since);
    lock(&SEEN).insert(runs.to_path_buf(), (now, notes.clone()));
    notes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn sizes_come_from_the_summary_line() {
        let stat = " src/a.rs | 6 +++++-\n 2 files changed, 6 insertions(+), 1 deletion(-)\n";
        assert_eq!(totals(stat), Some((6, 1)));
        assert_eq!(totals(" 1 file changed, 3 deletions(-)\n"), Some((0, 3)));
        assert_eq!(totals(""), None);
    }
}
