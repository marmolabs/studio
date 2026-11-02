//! The office's rules, written once: what needs the owner, which colour a
//! project has, which phase a project is in, and what counts as shipped or at
//! work. The team pages and the floor all ask here, so they cannot tell
//! different stories.

use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{AskView, ReleaseState, ReleaseView};

use super::Office;
use super::floor::{self, shipped};
use super::model::{Phase, TINTS};
use crate::ui;

// ------------------------------------------------------------ needs you

/// What waits for the owner: open asks, and tickets held for a reason that
/// doesn't end on its own. A hold for budget or quota starts again by itself,
/// so it does not count.
pub struct Needs<'a> {
    pub asks: &'a [AskView],
    /// Held for the owner.
    pub held: Vec<&'a Ticket>,
}

/// What needs the owner in `office`.
pub fn needs_you(office: &Office) -> Needs<'_> {
    let held = office
        .tickets
        .iter()
        .filter(|t| t.state.kind() == "held" && ui::needs_owner(&t.state))
        .collect();
    Needs {
        asks: &office.asks,
        held,
    }
}

impl Needs<'_> {
    /// How many things wait for the owner.
    pub fn count(&self) -> usize {
        self.asks.len() + self.held.len()
    }

    /// How many of them belong to `project`.
    pub fn in_project(&self, office: &Office, project: &str) -> usize {
        let asks = self
            .asks
            .iter()
            .filter(|a| {
                office
                    .ticket(&a.ticket)
                    .is_some_and(|t| t.project.to_string() == project)
            })
            .count();
        asks + self
            .held
            .iter()
            .filter(|t| t.project.to_string() == project)
            .count()
    }
}

// ------------------------------------------------------------- colours

/// The colour index of `project`: its place in `projects` (or 0), wrapped to
/// the colours there are.
pub fn tint(projects: &[String], project: &str) -> usize {
    wrap(projects.iter().position(|p| p == project).unwrap_or(0))
}

/// A place in the list of projects, wrapped to the colours there are: a
/// ninth project shares the first one's.
pub fn wrap(index: usize) -> usize {
    index % TINTS
}

/// The class of a project's colour dot, for the project at `index`.
pub fn dot_class(index: usize) -> String {
    format!("pjd pj-{}", wrap(index))
}

// -------------------------------------------------------------- counts

/// Tickets that landed in the window, newest first, with when. A landed plan
/// only filed its goal's tickets, so it isn't one of them.
pub fn landed(office: &Office) -> Vec<(Ticket, u64)> {
    shipped(office)
        .into_iter()
        .filter(|(t, _)| t.state.kind() == "landed")
        .collect()
}

// -------------------------------------------------------------- phases

/// A release the owner can still see move: checking, waiting for the
/// go-ahead or publishing.
pub fn is_releasing(release: &ReleaseView) -> bool {
    matches!(
        release.state,
        ReleaseState::Checking | ReleaseState::Waiting | ReleaseState::Publishing
    )
}

/// Where `project` stands, by these rules in this order: Releasing when a
/// release is checking, waiting or publishing, closed office or not, as a
/// release carries on and may wait for the owner; Building when any of its
/// work tasks is claimed, working, asking or held; Planning when a goal's
/// plan is open and nothing is building; Testing when anything is in checks,
/// review or ready and nothing is building; Live when nothing is open and it
/// has been released; Idle otherwise. A closed office pauses the projects
/// that have work, so those are Paused instead; Live and Idle ones have
/// nothing to pause.
pub fn phase(office: &Office, project: &str) -> Phase {
    let view = office.project(project);
    if view
        .and_then(|v| v.release.as_ref())
        .is_some_and(is_releasing)
    {
        return Phase::Releasing;
    }
    match at_work(office, project) {
        Phase::Planning | Phase::Building | Phase::Testing if office.status.closed => Phase::Paused,
        phase => phase,
    }
}

/// Where `project` stands by its own work, the office open or not.
fn at_work(office: &Office, project: &str) -> Phase {
    let view = office.project(project);
    let own: Vec<&Ticket> = office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project)
        .collect();
    let work = |t: &&&Ticket| t.kind != TicketKind::Plan;
    let building = own
        .iter()
        .filter(work)
        .any(|t| matches!(t.state.kind(), "claimed" | "working" | "asking" | "held"));
    if building {
        return Phase::Building;
    }
    let planning = own
        .iter()
        .any(|t| t.kind == TicketKind::Plan && !is_finished(t));
    if planning {
        return Phase::Planning;
    }
    if own
        .iter()
        .any(|t| matches!(t.state.kind(), "checking" | "reviewing" | "ready"))
    {
        return Phase::Testing;
    }
    let released = view.is_some_and(|v| v.last_release.is_some());
    if released && own.iter().all(|t| is_finished(t)) {
        return Phase::Live;
    }
    Phase::Idle
}

/// The open goal `project` works towards: the first with a version, else
/// the oldest.
pub fn milestone(office: &Office, project: &str) -> Option<floor::Goal> {
    let mut goals: Vec<floor::Goal> = floor::goals(office)
        .into_iter()
        .filter(|g| g.plan.project.to_string() == project)
        .collect();
    let at = goals
        .iter()
        .position(|g| g.plan.version.is_some())
        .unwrap_or(0);
    (at < goals.len()).then(|| goals.swap_remove(at))
}

/// Landed, failed or cancelled.
fn is_finished(ticket: &Ticket) -> bool {
    matches!(ticket.state.kind(), "landed" | "failed" | "cancelled")
}

/// Whether `hire` leads a project.
pub fn leads(office: &Office, hire: &str) -> bool {
    office
        .projects
        .iter()
        .any(|p| p.lead.as_deref() == Some(hire))
}

/// How many releases wait for the owner's go-ahead.
pub fn releases_waiting(office: &Office) -> usize {
    office
        .projects
        .iter()
        .filter_map(|p| p.release.as_ref())
        .filter(|r| r.state == ReleaseState::Waiting)
        .count()
}

/// Everything that waits for the owner: what `needs_you` counts, and the
/// releases that wait for a go-ahead.
pub fn owner_needs(office: &Office) -> usize {
    needs_you(office).count() + releases_waiting(office)
}

/// What waits for the owner in `project`, as `owner_needs` counts it.
pub fn project_needs(office: &Office, project: &str) -> usize {
    let waiting = office
        .project(project)
        .and_then(|p| p.release.as_ref())
        .is_some_and(|r| r.state == ReleaseState::Waiting);
    needs_you(office).in_project(office, project) + usize::from(waiting)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ticket(id: &str, project: &str, state: Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": project, "title": id, "goal": "", "hire": "dev",
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    fn ask(id: &str, ticket: &str) -> AskView {
        serde_json::from_value(json!({
            "id": id, "ticket": ticket, "hire": "dev", "kind": "permission",
            "tool": "Bash", "input": {}, "reasons": [],
        }))
        .unwrap()
    }

    #[test]
    fn needs_you_is_open_asks_and_tickets_held_for_the_owner() {
        let office = Office {
            tickets: vec![
                ticket("S-1", "shop", json!({"asking": {"ask": "a1"}})),
                // A budget hold starts again by itself; a pause waits.
                ticket("S-2", "shop", json!({"held": {"reason": "budget"}})),
                ticket("S-3", "shop", json!({"held": {"reason": "paused"}})),
                ticket("A-1", "api", json!({"held": {"reason": "auth"}})),
                ticket("A-2", "api", json!("working")),
            ],
            asks: vec![ask("a1", "S-1"), ask("a2", "S-1"), ask("a3", "A-2")],
            ..Office::default()
        };
        let needs = needs_you(&office);
        let ids = |tickets: &[&Ticket]| -> Vec<String> {
            tickets.iter().map(|t| t.id.to_string()).collect()
        };
        assert_eq!(ids(&needs.held), ["S-3", "A-1"]);
        // Every ask counts, however many a ticket has.
        assert_eq!(needs.count(), 5);
        assert_eq!(needs.in_project(&office, "shop"), 3);
        assert_eq!(needs.in_project(&office, "api"), 2);
        assert_eq!(needs.in_project(&office, "other"), 0);
        assert_eq!(needs_you(&Office::default()).count(), 0);
    }

    #[test]
    fn a_project_has_one_colour_wherever_it_is_drawn() {
        let projects: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        assert_eq!(tint(&projects, "p0"), 0);
        assert_eq!(tint(&projects, "p3"), 3);
        // The ninth project shares the first one's colour; an unknown one gets it too.
        assert_eq!(tint(&projects, "p8"), 0);
        assert_eq!(tint(&projects, "p9"), 1);
        assert_eq!(tint(&projects, "nope"), 0);
        assert_eq!(dot_class(3), "pjd pj-3");
        assert_eq!(dot_class(9), "pjd pj-1");
    }

    #[test]
    fn nothing_landed_in_an_empty_office() {
        assert!(landed(&Office::default()).is_empty());
    }

    fn plan_ticket(id: &str, project: &str, state: Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": project, "title": id, "goal": "Goal", "hire": "pro",
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
            "kind": "plan",
        }))
        .unwrap()
    }

    fn release(state: &str) -> ReleaseView {
        serde_json::from_value(json!({
            "project": "shop", "version": "1.2", "state": state,
            "steps": [{"name": "Final checks", "done": true}, {"name": "Go-ahead", "done": false}],
            "started_at": 0,
        }))
        .unwrap()
    }

    /// An office with the shop's `tickets`, released before or not.
    fn shop(tickets: Vec<Ticket>, released: bool) -> Office {
        let mut project = studiod::api::ProjectView {
            name: "shop".into(),
            ..Default::default()
        };
        project.last_release = released.then(|| release("released"));
        Office {
            tickets,
            projects: vec![project],
            ..Office::default()
        }
    }

    fn phase_of(tickets: Vec<Ticket>, released: bool) -> Phase {
        phase(&shop(tickets, released), "shop")
    }

    #[test]
    fn a_project_with_nothing_open_is_idle_or_live() {
        assert_eq!(phase_of(vec![], false), Phase::Idle);
        assert_eq!(phase_of(vec![], true), Phase::Live);
        let done = vec![
            ticket("S-1", "shop", json!("landed")),
            ticket("S-2", "shop", json!("cancelled")),
            ticket(
                "S-3",
                "shop",
                json!({"failed": {"class": "crash", "detail": ""}}),
            ),
        ];
        assert_eq!(phase_of(done.clone(), false), Phase::Idle);
        assert_eq!(phase_of(done, true), Phase::Live);
        // Work that waits its turn is open: not live, and nothing is going on.
        let queued = vec![ticket("S-1", "shop", json!("queued"))];
        assert_eq!(phase_of(queued, true), Phase::Idle);
        // Another project's tasks are not this one's.
        let other = vec![ticket("A-1", "api", json!("working"))];
        assert_eq!(phase_of(other, true), Phase::Live);
    }

    #[test]
    fn work_in_hand_means_building() {
        for state in [
            json!("claimed"),
            json!("working"),
            json!({"asking": {"ask": "a"}}),
            json!({"held": {"reason": "paused"}}),
            json!({"held": {"reason": "budget"}}),
        ] {
            let tickets = vec![ticket("S-1", "shop", state.clone())];
            assert_eq!(phase_of(tickets, true), Phase::Building, "{state}");
        }
        // Building wins over an open plan and over checks.
        let busy = vec![
            ticket("S-1", "shop", json!("working")),
            ticket("S-2", "shop", json!("checking")),
            plan_ticket("S-3", "shop", json!("working")),
        ];
        assert_eq!(phase_of(busy, false), Phase::Building);
        // A plan at work is planning, not building.
        let planned = vec![plan_ticket("S-1", "shop", json!("working"))];
        assert_eq!(phase_of(planned, false), Phase::Planning);
    }

    #[test]
    fn an_open_plan_means_planning_when_nothing_builds() {
        for state in [
            json!("queued"),
            json!("working"),
            json!({"asking": {"ask": "a"}}),
        ] {
            let tickets = vec![plan_ticket("S-1", "shop", state)];
            assert_eq!(phase_of(tickets, true), Phase::Planning);
        }
        // A plan that landed has filed its tasks; they decide.
        let filed = vec![plan_ticket("S-1", "shop", json!("landed"))];
        assert_eq!(phase_of(filed, true), Phase::Live);
        // Planning comes before testing.
        let both = vec![
            plan_ticket("S-1", "shop", json!("queued")),
            ticket("S-2", "shop", json!("reviewing")),
        ];
        assert_eq!(phase_of(both, false), Phase::Planning);
    }

    #[test]
    fn checks_review_and_ready_work_means_testing() {
        for state in ["checking", "reviewing", "ready"] {
            let tickets = vec![
                ticket("S-1", "shop", json!(state)),
                ticket("S-2", "shop", json!("landed")),
            ];
            assert_eq!(phase_of(tickets, true), Phase::Testing, "{state}");
        }
    }

    #[test]
    fn a_release_under_way_means_releasing() {
        for state in ["checking", "waiting", "publishing"] {
            let mut office = shop(vec![ticket("S-1", "shop", json!("working"))], true);
            office.projects[0].release = Some(release(state));
            // It comes before building.
            assert_eq!(phase(&office, "shop"), Phase::Releasing, "{state}");
        }
        // A release that failed or was cancelled is no longer under way.
        for state in ["failed", "cancelled", "released"] {
            let mut office = shop(vec![], true);
            office.projects[0].release = Some(release(state));
            assert_eq!(phase(&office, "shop"), Phase::Live, "{state}");
        }
    }

    #[test]
    fn a_closed_office_pauses_the_projects_at_work_but_not_a_release_under_way() {
        let mut office = shop(vec![ticket("S-1", "shop", json!("working"))], true);
        office.status.closed = true;
        assert_eq!(phase(&office, "shop"), Phase::Paused);
        // A project with nothing open has nothing to pause.
        assert_eq!(phase(&office, "anything"), Phase::Idle);
        // A release carries on, and may still wait for the owner.
        for state in ["checking", "waiting", "publishing"] {
            office.projects[0].release = Some(release(state));
            assert_eq!(phase(&office, "shop"), Phase::Releasing, "{state}");
        }
        office.projects[0].release = Some(release("failed"));
        assert_eq!(phase(&office, "shop"), Phase::Paused);
    }

    #[test]
    fn a_release_waiting_for_the_go_ahead_needs_the_owner() {
        let held = ticket("S-1", "shop", json!({"held": {"reason": "auth"}}));
        let mut office = shop(vec![held], true);
        assert_eq!(project_needs(&office, "shop"), 1);
        assert_eq!(owner_needs(&office), 1);
        office.projects[0].release = Some(release("waiting"));
        assert_eq!(project_needs(&office, "shop"), 2);
        assert_eq!(owner_needs(&office), 2);
        assert_eq!(releases_waiting(&office), 1);
        office.projects[0].release = Some(release("publishing"));
        assert_eq!(owner_needs(&office), 1);
    }

    #[test]
    fn a_lead_is_whoever_a_project_names() {
        let mut office = shop(vec![], false);
        office.projects[0].lead = Some("nora".into());
        assert!(leads(&office, "nora"));
        assert!(!leads(&office, "kit"));
    }
}
