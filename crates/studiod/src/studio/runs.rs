//! Scheduling: which ticket gets a worker, the money held back for it, and how
//! a worker is stopped.

use super::{Studio, StudioError, goal_of, lock};
use crate::api::JudgeView;
use crate::budget::{DAY_MS, Reservation};
use crate::{budget, now_ms, worker};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use studio_core::budget::Allowance;
use studio_core::event::Event;
use studio_core::ids::{ProjectId, TicketId};
use studio_core::rule::Hire;
use studio_core::ticket::{HeldReason, Ticket, TicketState};
use tokio::sync::Notify;

/// Why the studio wants a live run to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    Cancel,
    Budget,
    /// The ticket went back to its agent with a message, so its checks or
    /// review no longer matter.
    SentBack,
    /// The owner paused the ticket; it waits, held, until resumed.
    Pause,
}

/// Tells a ticket's worker to stop its run. Stored reasons survive until read,
/// and a cancel wins over a budget stop.
pub(crate) struct Stop {
    pub(crate) notify: Notify,
    reason: Mutex<Option<StopReason>>,
    /// Messages for the ticket's agent that its worker has not passed on yet.
    said: Mutex<VecDeque<String>>,
    /// Rung when a message is added to `said`.
    pub(crate) heard: Notify,
}

impl Stop {
    fn new() -> Stop {
        Stop {
            notify: Notify::new(),
            reason: Mutex::new(None),
            said: Mutex::new(VecDeque::new()),
            heard: Notify::new(),
        }
    }

    /// Leaves a message for the agent, for the worker to pass on to its run.
    pub(crate) fn say(&self, text: String) {
        lock(&self.said).push_back(text);
        self.heard.notify_one();
    }

    /// The messages left for the agent so far, oldest first.
    pub(crate) fn take_said(&self) -> Vec<String> {
        lock(&self.said).drain(..).collect()
    }

    pub(crate) fn request(&self, reason: StopReason) {
        let mut current = lock(&self.reason);
        if *current != Some(StopReason::Cancel) {
            *current = Some(reason);
        }
        self.notify.notify_one();
    }

    pub(crate) fn reason(&self) -> Option<StopReason> {
        *lock(&self.reason)
    }
}

static NEXT_EXTRA_RUN: AtomicU64 = AtomicU64::new(1);

/// Holds a small run's allowance until its spend is recorded.
pub(crate) struct ExtraRun {
    studio: Arc<Studio>,
    key: String,
}

impl Drop for ExtraRun {
    fn drop(&mut self) {
        lock(&self.studio.reservations).remove(&self.key);
        self.studio.wake.notify_one();
    }
}

/// How often held tickets are checked for a resume.
const RESUME_EVERY: Duration = Duration::from_secs(60);

/// How long a ticket held for the provider's quota waits to try again when
/// the provider didn't say when its limit resets.
pub(super) const QUOTA_RETRY_MS: u64 = 15 * 60 * 1000;

/// When a ticket held for quota at `held_at` tries again: a minute after the
/// limit resets, if the provider said when, else `QUOTA_RETRY_MS` later.
pub(crate) fn quota_retry_at(held_at: u64, resets_at: Option<u64>) -> u64 {
    match resets_at {
        Some(reset) if reset > held_at => reset + 60_000,
        _ => held_at + QUOTA_RETRY_MS,
    }
}

impl Studio {
    /// After a restart nothing is running: hold every ticket that thought it
    /// was, so the board never shows work that isn't happening.
    pub fn recover(&self) -> Result<usize, StudioError> {
        let mut store = lock(&self.store);
        let mut stuck = store.in_flight()?;
        stuck.extend(store.tickets_in("asking")?);
        for ticket in &stuck {
            store.append(
                &ticket.id,
                now_ms(),
                &Event::Held {
                    reason: HeldReason::Paused,
                },
            )?;
        }
        Ok(stuck.len())
    }

    /// Starts workers whenever there's capacity, until the process ends.
    pub fn spawn_scheduler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let studio = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(err) = studio.schedule() {
                    eprintln!("scheduler: {err}");
                }
                studio.spawn_standups();
                studio.spawn_operations();
                tokio::select! {
                    _ = studio.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        })
    }

    /// One scheduling pass: resume orphaned work first, then claim queued
    /// tickets. A closed office pauses the studio: nothing new starts.
    pub fn schedule(self: &Arc<Self>) -> Result<(), StudioError> {
        let capacity = if self.is_closed() {
            0
        } else {
            self.desks() as usize
        };
        let orphans: Vec<Ticket> = {
            let store = lock(&self.store);
            let active = lock(&self.active);
            store
                .in_flight()?
                .into_iter()
                .filter(|t| !active.contains_key(&t.id.to_string()))
                .collect()
        };
        for ticket in orphans {
            if lock(&self.active).len() >= capacity {
                return Ok(());
            }
            self.start_worker(ticket.id);
        }
        {
            let mut last = lock(&self.last_resume);
            if last.is_none_or(|at| at.elapsed() >= RESUME_EVERY) {
                *last = Some(std::time::Instant::now());
                drop(last);
                self.resume_budget_held()?;
                self.resume_quota_held(now_ms())?;
            }
        }
        while lock(&self.active).len() < capacity {
            if self.studio_budget_exhausted()? {
                break;
            }
            let Some(next) = self.next_queued()? else {
                break;
            };
            // Another claimer may have taken it; the next pass looks again.
            if let Some(ticket) = lock(&self.store).claim(&next, now_ms())? {
                self.start_worker(ticket.id);
            }
        }
        Ok(())
    }

    /// The oldest queued ticket whose goal has no other ticket in progress.
    /// A ticket with no goal is its own goal. A ticket its own worker sent
    /// back (a landing that conflicted) waits until that worker has ended.
    fn next_queued(&self) -> Result<Option<TicketId>, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let working: std::collections::HashSet<String> =
            lock(&self.active).keys().cloned().collect();
        let busy: std::collections::HashSet<(ProjectId, String)> = tickets
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    TicketState::Claimed
                        | TicketState::Working
                        | TicketState::Asking { .. }
                        | TicketState::Checking
                        | TicketState::Reviewing
                        | TicketState::Held { .. }
                        | TicketState::Ready
                )
            })
            .map(|t| (t.project.clone(), goal_of(t)))
            .collect();
        Ok(next_queued_from(&tickets, &working, &busy))
    }

    /// Resumes each ticket held for budget that a run would now be funded for.
    /// A ticket whose own cap is used up stays held. Returns how many resumed.
    pub fn resume_budget_held(&self) -> Result<usize, StudioError> {
        let held = lock(&self.store).tickets_in("held")?;
        let mut resumed = 0;
        for ticket in held {
            if ticket.state
                != (TicketState::Held {
                    reason: HeldReason::Budget,
                })
            {
                continue;
            }
            let Ok(hire) = self.hire(&ticket.hire) else {
                continue;
            };
            // A run would be refused for want of a price, whatever the money.
            let unpriced = self.price(hire.model_for(ticket.attempts)).is_none()
                && self
                    .runtime(&hire.runtime)
                    .is_ok_and(|r| !r.capabilities().reports_cost);
            if unpriced {
                continue;
            }
            let allowance = self.allowance(&ticket, &hire)?;
            if !matches!(allowance, Allowance::Run { .. }) {
                continue;
            }
            self.append(&ticket.id, Event::Resumed)?;
            crate::runlog::RunLog::new(&self.paths.data_dir, &ticket.id.to_string())
                .note("resumed: budget is available again");
            resumed += 1;
        }
        if resumed > 0 {
            self.wake.notify_one();
        }
        Ok(resumed)
    }

    /// Resumes each ticket held for the provider's quota whose wait is over
    /// at `now`; see `quota_retry_at`. Returns how many resumed.
    pub fn resume_quota_held(&self, now: u64) -> Result<usize, StudioError> {
        let (held, since) = {
            let store = lock(&self.store);
            (store.tickets_in("held")?, store.state_since()?)
        };
        let mut resumed = 0;
        for ticket in held {
            if ticket.state
                != (TicketState::Held {
                    reason: HeldReason::Quota,
                })
            {
                continue;
            }
            let Some(&held_at) = since.get(&ticket.id.to_string()) else {
                continue;
            };
            let log = crate::runlog::RunLog::new(&self.paths.data_dir, &ticket.id.to_string());
            if now < quota_retry_at(held_at, log.quota_resets_at()) {
                continue;
            }
            self.append(&ticket.id, Event::Resumed)?;
            log.note("resumed: trying the provider again");
            resumed += 1;
        }
        if resumed > 0 {
            self.wake.notify_one();
        }
        Ok(resumed)
    }

    fn start_worker(self: &Arc<Self>, id: TicketId) {
        let stop = Arc::new(Stop::new());
        // What the agent was told while its ticket waited is the first thing
        // it hears.
        for text in self.take_kept(&id.to_string()) {
            stop.say(text);
        }
        lock(&self.active).insert(id.to_string(), Arc::clone(&stop));
        tokio::spawn(worker::work(Arc::clone(self), id, stop));
    }

    /// True when the studio-wide 24-hour budget can't fund another run, so
    /// queued tickets stay queued instead of being claimed and held.
    fn studio_budget_exhausted(&self) -> Result<bool, StudioError> {
        let spent = lock(&self.store)
            .spent_since(now_ms().saturating_sub(DAY_MS))?
            .total;
        Ok(self.out_of_funds(spent))
    }

    /// `studio_budget_exhausted` for a studio that has spent `spent` in the
    /// last 24 hours.
    pub(super) fn out_of_funds(&self, spent: f64) -> bool {
        let budget = &self.config().budget;
        let reserved: f64 = lock(&self.reservations).values().map(|r| r.usd).sum();
        let left = budget.daily_usd - spent - reserved;
        !studio_core::budget::funded(left, budget.min_run_usd)
    }

    /// What the ticket's next run with `hire` could spend, without reserving it.
    fn allowance(&self, ticket: &Ticket, hire: &Hire) -> Result<Allowance, StudioError> {
        let reservations = lock(&self.reservations);
        self.decide_allowance(ticket, hire, &reservations)
    }

    fn decide_allowance(
        &self,
        ticket: &Ticket,
        hire: &Hire,
        reservations: &HashMap<String, Reservation>,
    ) -> Result<Allowance, StudioError> {
        let project_day = self.project(&ticket.project)?.daily_usd;
        let inputs = budget::Inputs {
            budget: &self.config().budget,
            project_day,
            ticket,
            hire,
        };
        Ok(budget::decide(
            &lock(&self.store),
            &inputs,
            reservations,
            now_ms(),
        )?)
    }

    /// Reserves money for the ticket's next run, or says which limit refuses it.
    pub(crate) fn reserve(&self, ticket: &Ticket, hire: &Hire) -> Result<Allowance, StudioError> {
        let mut reservations = lock(&self.reservations);
        let decision = self.decide_allowance(ticket, hire, &reservations)?;
        if let Allowance::Run { usd, .. } = decision {
            reservations.insert(
                ticket.id.to_string(),
                Reservation {
                    project: ticket.project.0.clone(),
                    hire: hire.id.0.clone(),
                    usd,
                },
            );
        }
        Ok(decision)
    }

    /// Reserves a small run under every daily limit, with a unique key even
    /// when several runs share one synthetic ledger id.
    pub(crate) fn reserve_extra(
        self: &Arc<Self>,
        project: &str,
        hire: &Hire,
        usd: f64,
        ledger: &str,
        covered_by: &str,
    ) -> Result<Option<ExtraRun>, StudioError> {
        let config = self.config();
        let project_day = config
            .projects
            .iter()
            .find(|entry| entry.name == project)
            .and_then(|entry| entry.daily_usd);
        let mut reservations = lock(&self.reservations);
        let mut others = reservations.clone();
        others.remove(covered_by);
        if !budget::has_headroom(
            &lock(&self.store),
            &config.budget,
            (project, project_day),
            hire,
            &others,
            now_ms(),
            usd,
        )? {
            return Ok(None);
        }
        let serial = NEXT_EXTRA_RUN.fetch_add(1, Ordering::Relaxed);
        let key = format!("{ledger}#extra-{serial}");
        reservations.insert(
            key.clone(),
            Reservation {
                project: project.to_string(),
                hire: hire.id.0.clone(),
                usd,
            },
        );
        Ok(Some(ExtraRun {
            studio: Arc::clone(self),
            key,
        }))
    }

    pub(crate) fn release(&self, ticket: &TicketId) {
        lock(&self.reservations).remove(&ticket.to_string());
        lock(&self.metered).remove(&ticket.to_string());
        self.clear_judge(ticket);
        self.wake.notify_one();
    }

    /// Records that `hire` (running `model`) now judges `ticket`: its
    /// reviewer, or the critic of `gate`.
    pub(crate) fn set_judge(&self, ticket: &TicketId, hire: &str, model: &str, gate: Option<&str>) {
        lock(&self.judges).insert(
            ticket.to_string(),
            JudgeView {
                hire: hire.to_string(),
                model: model.to_string(),
                gate: gate.map(str::to_string),
                at: now_ms(),
            },
        );
    }

    /// The judge's turn is over, or the ticket left review.
    pub(crate) fn clear_judge(&self, ticket: &TicketId) {
        lock(&self.judges).remove(&ticket.to_string());
    }

    pub(crate) fn worker_done(&self, id: &TicketId) {
        lock(&self.active).remove(&id.to_string());
        self.wake.notify_one();
    }

    /// What the ticket's live run has spent so far by its meter, which
    /// `TicketSpend` and `TicketSpends` count until the run's spend is recorded.
    pub(crate) fn meter(&self, ticket: &TicketId, usd: f64) {
        lock(&self.metered).insert(ticket.to_string(), usd);
    }
}

fn next_queued_from(
    tickets: &[Ticket],
    working: &std::collections::HashSet<String>,
    busy: &std::collections::HashSet<(ProjectId, String)>,
) -> Option<TicketId> {
    let known: HashMap<String, &Ticket> = tickets
        .iter()
        .map(|ticket| (ticket.id.to_string(), ticket))
        .collect();
    let mut candidates = tickets
        .iter()
        .filter(|ticket| {
            ticket.state == TicketState::Queued
                && !busy.contains(&(ticket.project.clone(), goal_of(ticket)))
                && !working.contains(&ticket.id.to_string())
                && ticket.depends_on.iter().all(|dependency| {
                    known
                        .get(&dependency.to_string())
                        .is_some_and(|ticket| ticket.state == TicketState::Landed)
                })
        })
        .collect::<Vec<_>>();
    // Stable: tasks with the same priority keep their creation order.
    candidates.sort_by_key(|ticket| std::cmp::Reverse(ticket.priority));
    candidates.first().map(|ticket| ticket.id.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use serde_json::{Value, json};

    fn ticket(
        id: &str,
        created_at: u64,
        state: Value,
        priority: u32,
        depends_on: &[&str],
        goal: &str,
    ) -> Ticket {
        serde_json::from_value(json!({
            "id":id, "project":"shop", "title":id, "goal":goal, "hire":"dev",
            "state":state, "attempts":0, "base_branch":"main", "created_at":created_at,
            "priority":priority, "depends_on":depends_on,
        }))
        .unwrap()
    }

    fn choose(tickets: &[Ticket]) -> Option<TicketId> {
        next_queued_from(tickets, &Default::default(), &Default::default())
    }

    #[test]
    fn the_highest_priority_ready_task_wins_and_ties_stay_oldest_first() {
        let tickets = vec![
            ticket("S-1", 1, json!("queued"), 10, &[], ""),
            ticket("S-2", 2, json!("queued"), 100, &[], ""),
            ticket("S-3", 3, json!("queued"), 100, &[], ""),
        ];
        assert_eq!(choose(&tickets).unwrap().0, "S-2");
        let tied = vec![
            ticket("S-1", 1, json!("queued"), 50, &[], ""),
            ticket("S-2", 2, json!("queued"), 50, &[], ""),
        ];
        assert_eq!(choose(&tied).unwrap().0, "S-1");
    }

    #[test]
    fn only_landed_dependencies_unlock_work() {
        let landed = ticket("S-1", 1, json!("landed"), 50, &[], "");
        let dependent = ticket("S-2", 2, json!("queued"), 50, &["S-1"], "");
        assert_eq!(choose(&[landed, dependent]).unwrap().0, "S-2");
        for state in [
            json!("queued"),
            json!("cancelled"),
            json!({"failed":{"class":"checks","detail":"no"}}),
        ] {
            let blocked = ticket("S-1", 1, state, 50, &[], "");
            let dependent = ticket("S-2", 2, json!("queued"), 100, &["S-1"], "");
            assert!(choose(&[blocked, dependent]).is_none_or(|id| id.0 == "S-1"));
        }
        let dependent = ticket("S-2", 2, json!("queued"), 100, &["MISSING"], "");
        assert!(choose(&[dependent]).is_none());
    }

    #[test]
    fn self_dependencies_cycles_and_busy_goals_do_not_start() {
        let own = ticket("S-1", 1, json!("queued"), 100, &["S-1"], "");
        assert!(choose(&[own]).is_none());
        let a = ticket("S-1", 1, json!("queued"), 100, &["S-2"], "");
        let b = ticket("S-2", 2, json!("queued"), 100, &["S-1"], "");
        assert!(choose(&[a, b]).is_none());

        let active = ticket("S-1", 1, json!("working"), 50, &[], "checkout");
        let queued = ticket("S-2", 2, json!("queued"), 100, &[], "checkout");
        let busy = [(ProjectId::new("shop"), "checkout".to_string())].into();
        assert!(next_queued_from(&[active, queued], &Default::default(), &busy).is_none());
    }

    #[test]
    fn quota_waits_for_the_reset_or_a_while() {
        assert_eq!(quota_retry_at(1_000, Some(5_000)), 65_000);
        // No reset time, or one already past: a while after the hold.
        assert_eq!(quota_retry_at(1_000, None), 1_000 + QUOTA_RETRY_MS);
        assert_eq!(quota_retry_at(10_000, Some(5_000)), 10_000 + QUOTA_RETRY_MS);
    }
}
