//! Who is where on the company floor, from studiod's state.

use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{HeldReason, Ticket, TicketState};
use studiod::api::{ReleaseState, ReleaseView};

use super::floor::{self, Room};
use super::model::{Doing, Hq, Launch, Milestone, Person, Role, Team, Worker};
use super::panel::Extra;
use super::{Office, facts, key};
use crate::words;

const MINUTE_MS: u64 = 60_000;

/// The company floor for `office`: a team for each project, in the order of
/// `studio.toml`, and the people and rooms they share.
pub fn hq(office: &Office, _extra: &Extra) -> Hq {
    let projects = office.project_names();
    let seated = at_desks(office);
    let teams = projects
        .iter()
        .enumerate()
        .map(|(i, name)| team(office, &seated, name, facts::wrap(i)))
        .collect();
    let in_room = |which: Room| -> Vec<&Ticket> {
        office
            .tickets
            .iter()
            .filter(|t| floor::room(t) == which)
            .collect()
    };
    Hq {
        teams,
        owner_needs: facts::owner_needs(office),
        lounge: floor::resting(office)
            .into_iter()
            .filter(|m| !facts::leads(office, &m.id) && Role::of(&m.role) != Role::Lead)
            .map(|m| {
                let idle = floor::idle_since(office, &m.id)
                    .map(|at| office.now.saturating_sub(at) / MINUTE_MS);
                person(office, m.id, idle)
            })
            .collect(),
        review: in_room(Room::Review)
            .into_iter()
            .map(|t| judged(office, t))
            .collect(),
        checks: in_room(Room::Checks)
            .into_iter()
            .map(|t| worker(office, t, t.hire.to_string(), 0.5))
            .collect(),
        closed: office.status.closed,
    }
}

/// One project's team area.
fn team(office: &Office, seated: &[Worker], project: &str, tint: usize) -> Team {
    let view = office.project(project);
    Team {
        project: project.to_string(),
        tint,
        phase: facts::phase(office, project),
        needs: facts::project_needs(office, project),
        lead: view.and_then(|v| v.lead.clone()).map(|hire| Person {
            role: Role::Lead,
            name: office.name_of(&hire),
            hire,
            idle_minutes: None,
        }),
        members: seated
            .iter()
            .filter(|w| w.project == project)
            .cloned()
            .collect(),
        milestone: milestone(office, project),
        release: view.and_then(|v| v.release.as_ref()).and_then(launch),
        live: view
            .and_then(|v| v.last_release.as_ref())
            .map(|r| r.version.clone()),
    }
}

/// The project's open goal, preferring one that is a milestone towards a
/// version.
fn milestone(office: &Office, project: &str) -> Option<Milestone> {
    let goal = facts::milestone(office, project)?;
    Some(Milestone {
        key: key("goals", &goal.plan.id.to_string()),
        title: goal.name(),
        version: goal.plan.version.clone(),
        done: goal.landed(),
        total: goal.filed.len(),
    })
}

/// A release as the sign tells it; one that was cancelled is not shown.
fn launch(release: &ReleaseView) -> Option<Launch> {
    if release.state == ReleaseState::Cancelled {
        return None;
    }
    Some(Launch {
        version: release.version.clone(),
        done: release.steps.iter().filter(|s| s.done).count(),
        total: release.steps.len(),
        waiting: release.state == ReleaseState::Waiting,
        failed: release.state == ReleaseState::Failed,
    })
}

/// The workers at desks: oldest claim first, then by id so the order holds
/// still from one look to the next. A claim older than the window sorts
/// before any in it.
fn at_desks(office: &Office) -> Vec<Worker> {
    let mut seated: Vec<(Option<u64>, &Ticket)> = office
        .tickets
        .iter()
        .filter(|t| floor::room(t) == Room::Desk)
        .map(|t| {
            let claimed = office.last(&t.id.to_string(), |e| matches!(e, TicketEvent::Claimed));
            (claimed, t)
        })
        .collect();
    seated.sort_by(|(a, x), (b, y)| {
        a.cmp(b)
            .then_with(|| x.id.to_string().cmp(&y.id.to_string()))
    });
    seated
        .into_iter()
        .map(|(_, t)| worker(office, t, t.hire.to_string(), 0.25))
        .collect()
}

/// A ticket in review, with its judge in place of its own hire when known.
fn judged(office: &Office, ticket: &Ticket) -> Worker {
    let hire = office
        .judges
        .get(&ticket.id.to_string())
        .map_or_else(|| ticket.hire.to_string(), |j| j.hire.clone());
    worker(office, ticket, hire, 0.75)
}

fn worker(office: &Office, ticket: &Ticket, hire: String, progress: f32) -> Worker {
    Worker {
        ticket: ticket.id.to_string(),
        project: ticket.project.to_string(),
        title: ticket.title.clone(),
        role: role_of(office, &hire),
        name: office.name_of(&hire),
        hire,
        doing: doing(office, ticket),
        progress,
    }
}

fn doing(office: &Office, ticket: &Ticket) -> Doing {
    let id = ticket.id.to_string();
    match &ticket.state {
        TicketState::Asking { .. } => Doing::Asking(
            office
                .asks
                .iter()
                .find(|a| a.ticket == id)
                .map_or_else(|| "Needs you".to_string(), words::ask_title),
        ),
        TicketState::Held { reason } => Doing::Held(
            match reason {
                HeldReason::Paused => "paused",
                _ => floor::held_short(&ticket.state),
            }
            .to_string(),
        ),
        _ => Doing::Working,
    }
}

fn role_of(office: &Office, hire: &str) -> Role {
    if facts::leads(office, hire) {
        return Role::Lead;
    }
    office
        .member(hire)
        .map_or(Role::Other, |m| Role::of(&m.role))
}

fn person(office: &Office, hire: String, idle_minutes: Option<u64>) -> Person {
    Person {
        role: role_of(office, &hire),
        name: office.name_of(&hire),
        hire,
        idle_minutes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Happened, Member};
    use crate::summary::Judge;
    use serde_json::{Value, json};
    use studiod::api::{AskView, ProjectView};

    fn ticket(id: &str, project: &str, state: Value, extra: Value) -> Ticket {
        let mut t = json!({
            "id": id, "project": project, "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
            t.extend(extra.clone());
        }
        serde_json::from_value(t).unwrap()
    }

    fn work(id: &str, state: Value) -> Ticket {
        ticket(id, "game", state, json!({}))
    }

    fn plan(id: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({"kind": "plan"});
        if let (Some(f), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            f.extend(extra.clone());
        }
        ticket(id, "game", state, fields)
    }

    fn member(id: &str, role: &str) -> Member {
        Member {
            id: id.into(),
            role: role.into(),
            model: "m".into(),
            name: Default::default(),
        }
    }

    fn happened(id: &str, at_ms: u64, event: Value) -> Happened {
        Happened {
            ticket_id: id.into(),
            at_ms,
            event: serde_json::from_value(event).unwrap(),
        }
    }

    fn judge(hire: &str) -> Judge {
        Judge {
            hire: hire.into(),
            model: "m".into(),
            gate: None,
            at: 0,
        }
    }

    fn office(tickets: Vec<Ticket>) -> Office {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status.projects = vec!["game".into(), "site".into()];
        office.status.team = vec![
            member("dev", "engineer"),
            member("ana", "writer"),
            member("pro", "producer"),
            member("rev", "reviewer"),
            member("cri", "critic"),
            member("nora", "lead"),
            member("sam", "lead"),
            member("boss", "approver"),
            member("rout", "router"),
        ];
        office.projects = vec![
            ProjectView {
                name: "game".into(),
                lead: Some("nora".into()),
                ..ProjectView::default()
            },
            ProjectView {
                name: "site".into(),
                lead: Some("sam".into()),
                ..ProjectView::default()
            },
        ];
        office
    }

    fn release(state: &str, done: usize) -> ReleaseView {
        let steps: Vec<Value> = (0..5)
            .map(|i| json!({"name": format!("Step {i}"), "done": i < done}))
            .collect();
        serde_json::from_value(json!({
            "project": "game", "version": "1.2", "state": state, "steps": steps,
            "started_at": 0,
        }))
        .unwrap()
    }

    fn ask(ticket: &str) -> AskView {
        AskView {
            id: "a1".into(),
            ticket: ticket.into(),
            hire: "dev".into(),
            kind: "permission".into(),
            tool: "Bash".into(),
            input: json!({"command": "cargo test"}),
            reasons: Vec::new(),
            at_ms: 0,
        }
    }

    fn floor_of(office: &Office) -> Hq {
        hq(office, &Extra::default())
    }

    #[test]
    fn a_team_for_each_project_in_the_studios_order() {
        let f = floor_of(&office(vec![]));
        let names: Vec<(&str, usize)> = f
            .teams
            .iter()
            .map(|t| (t.project.as_str(), t.tint))
            .collect();
        assert_eq!(names, [("game", 0), ("site", 1)]);
        assert!(!f.closed);
        let lead = f.teams[0].lead.as_ref().unwrap();
        assert_eq!((lead.hire.as_str(), lead.role), ("nora", Role::Lead));
        assert_eq!(
            f.teams[1].lead.as_ref().map(|p| p.hire.as_str()),
            Some("sam")
        );
    }

    #[test]
    fn a_project_without_a_lead_has_none() {
        let mut o = office(vec![]);
        o.projects[1].lead = None;
        assert!(floor_of(&o).teams[1].lead.is_none());
    }

    #[test]
    fn the_members_are_the_people_on_the_projects_tasks_at_desks() {
        let mut o = office(vec![
            work("G-1", json!("working")),
            ticket("S-1", "site", json!("claimed"), json!({"hire": "ana"})),
            work("G-2", json!({"asking": {"ask": "a1"}})),
            // Not at desks: in checks, in review, ready, waiting, and plans.
            work("G-3", json!("checking")),
            work("G-4", json!("reviewing")),
            work("G-5", json!("ready")),
            work("G-6", json!("queued")),
            plan("G-7", json!("working"), json!({"hire": "pro"})),
        ]);
        o.asks = vec![ask("G-2")];
        let f = floor_of(&o);
        let ids =
            |t: &Team| -> Vec<String> { t.members.iter().map(|w| w.ticket.clone()).collect() };
        assert_eq!(ids(&f.teams[0]), ["G-1", "G-2"]);
        assert_eq!(ids(&f.teams[1]), ["S-1"]);
        let first = &f.teams[0].members[0];
        assert_eq!(
            (first.hire.as_str(), first.role, first.progress),
            ("dev", Role::Engineer, 0.25)
        );
        assert_eq!(
            f.teams[0].members[1].doing,
            Doing::Asking("Run a command".into())
        );
    }

    #[test]
    fn members_sit_oldest_claim_first_then_by_id() {
        let mut o = office(vec![
            work("G-4", json!("working")),
            work("G-2", json!("working")),
            work("G-3", json!("working")),
            work("G-1", json!("working")),
        ]);
        o.recent = vec![
            happened("G-3", 10, json!({"type": "claimed"})),
            happened("G-1", 20, json!({"type": "claimed"})),
        ];
        let f = floor_of(&o);
        let order: Vec<&str> = f.teams[0]
            .members
            .iter()
            .map(|w| w.ticket.as_str())
            .collect();
        // No claim in the window means an older claim; those go first, by id.
        assert_eq!(order, ["G-2", "G-4", "G-3", "G-1"]);
    }

    #[test]
    fn a_hold_says_why_and_an_unknown_hire_is_other() {
        let o = office(vec![
            work("G-1", json!({"held": {"reason": "budget"}})),
            work("G-2", json!({"held": {"reason": "paused"}})),
            work("G-3", json!({"asking": {"ask": "a9"}})),
            ticket("G-4", "game", json!("working"), json!({"hire": "ghost"})),
        ]);
        let f = floor_of(&o);
        let doing = |id: &str| {
            f.teams[0]
                .members
                .iter()
                .find(|w| w.ticket == id)
                .map(|w| (w.doing.clone(), w.role))
                .unwrap()
        };
        assert_eq!(doing("G-1").0, Doing::Held("budget".into()));
        assert_eq!(doing("G-2").0, Doing::Held("paused".into()));
        // An ask studiod no longer lists still shows the task needs you.
        assert_eq!(doing("G-3").0, Doing::Asking("Needs you".into()));
        assert_eq!(doing("G-4").1, Role::Other);
    }

    #[test]
    fn the_phase_comes_from_the_one_set_of_rules() {
        let mut o = office(vec![work("G-1", json!("working"))]);
        let f = floor_of(&o);
        assert_eq!(f.teams[0].phase, facts::phase(&o, "game"));
        assert_eq!(f.teams[1].phase, facts::phase(&o, "site"));
        o.status.closed = true;
        assert!(floor_of(&o).closed);
        assert_eq!(
            floor_of(&o).teams[0].phase,
            super::super::model::Phase::Paused
        );
    }

    #[test]
    fn the_milestone_is_the_open_goal_preferring_one_with_a_version() {
        let o = office(vec![
            plan("P-1", json!("landed"), json!({"goal": "Polish\nmore"})),
            ticket("G-1", "game", json!("landed"), json!({"parent": "P-1"})),
            plan(
                "P-2",
                json!("landed"),
                json!({"goal": "Boss fight", "version": "0.4"}),
            ),
            ticket("G-2", "game", json!("landed"), json!({"parent": "P-2"})),
            ticket("G-3", "game", json!("working"), json!({"parent": "P-2"})),
            ticket("G-4", "game", json!("cancelled"), json!({"parent": "P-2"})),
            // A merged goal is not open.
            plan(
                "P-3",
                json!("landed"),
                json!({"goal": "Merged", "version": "0.1"}),
            ),
            ticket("G-5", "game", json!("landed"), json!({"parent": "P-3"})),
        ]);
        let mut o = o;
        o.merged = vec!["G-5".into()];
        let f = floor_of(&o);
        let m = f.teams[0].milestone.as_ref().unwrap();
        assert_eq!(m.key, "goals/P-2");
        assert_eq!(m.title, "Boss fight");
        assert_eq!(m.version.as_deref(), Some("0.4"));
        // Landed over every task filed, cancelled ones left out.
        assert_eq!((m.done, m.total), (1, 2));
        assert!(f.teams[1].milestone.is_none());

        // Without a version, the first open goal.
        let o = office(vec![
            plan("P-1", json!("landed"), json!({"goal": "Polish\nmore"})),
            ticket("G-1", "game", json!("landed"), json!({"parent": "P-1"})),
        ]);
        let m = floor_of(&o).teams[0].milestone.clone().unwrap();
        assert_eq!(
            (m.title.as_str(), m.version, m.done, m.total),
            ("Polish", None, 1, 1)
        );
    }

    #[test]
    fn a_release_shows_its_steps_and_what_it_waits_for() {
        let mut o = office(vec![]);
        o.projects[0].release = Some(release("waiting", 4));
        o.projects[1].release = Some(release("failed", 2));
        o.projects[1].last_release = Some(release("released", 5));
        let f = floor_of(&o);
        let r = f.teams[0].release.clone().unwrap();
        assert_eq!((r.version.as_str(), r.done, r.total), ("1.2", 4, 5));
        assert!(r.waiting && !r.failed);
        let r = f.teams[1].release.clone().unwrap();
        assert!(r.failed && !r.waiting);
        assert_eq!(f.teams[1].live.as_deref(), Some("1.2"));
        assert_eq!(f.teams[0].live, None);
        // A cancelled release is gone from the sign.
        o.projects[0].release = Some(release("cancelled", 1));
        assert!(floor_of(&o).teams[0].release.is_none());
    }

    #[test]
    fn what_waits_for_the_owner_is_counted_by_project_and_across() {
        let mut o = office(vec![
            work("G-1", json!({"asking": {"ask": "a1"}})),
            ticket(
                "S-1",
                "site",
                json!({"held": {"reason": "auth"}}),
                json!({}),
            ),
        ]);
        o.asks = vec![ask("G-1")];
        o.projects[1].release = Some(release("waiting", 4));
        let f = floor_of(&o);
        assert_eq!(f.teams[0].needs, 1);
        // A held task and a release waiting for the go-ahead.
        assert_eq!(f.teams[1].needs, 2);
        assert_eq!(f.owner_needs, 3);
    }

    #[test]
    fn the_lounge_is_free_people_without_the_leads() {
        let mut o = office(vec![
            plan("P-1", json!("working"), json!({"hire": "pro"})),
            work("G-1", json!("working")),
            work("G-2", json!("reviewing")),
        ]);
        o.judges.insert("G-2".into(), judge("rev"));
        let f = floor_of(&o);
        // dev is at a desk, pro plans, rev judges; the leads and the front
        // desk are elsewhere.
        let lounge: Vec<&str> = f.lounge.iter().map(|p| p.hire.as_str()).collect();
        assert_eq!(lounge, ["ana", "cri"]);
        assert_eq!(f.lounge[0].role, Role::Writer);
    }

    #[test]
    fn the_lounge_counts_idle_minutes() {
        let mut o = office(vec![work("G-1", json!("checking"))]);
        o.recent = vec![happened(
            "G-1",
            o.now - 7 * 60_000 - 5_000,
            json!({"type": "checks_started"}),
        )];
        let f = floor_of(&o);
        let dev = f.lounge.iter().find(|p| p.hire == "dev").unwrap();
        assert_eq!(dev.idle_minutes, Some(7));
        let ana = f.lounge.iter().find(|p| p.hire == "ana").unwrap();
        assert_eq!(ana.idle_minutes, None);
    }

    #[test]
    fn checks_and_review_hold_their_tasks() {
        let mut o = office(vec![
            work("G-1", json!("checking")),
            work("G-2", json!("reviewing")),
            work("G-3", json!("reviewing")),
            ticket(
                "G-4",
                "game",
                json!({"asking": {"ask": "a1"}}),
                json!({"held_from": "reviewing"}),
            ),
        ]);
        o.asks = vec![ask("G-4")];
        o.judges.insert("G-2".into(), judge("cri"));
        let f = floor_of(&o);
        assert!(f.teams[0].members.is_empty());
        assert_eq!(f.checks.len(), 1);
        assert_eq!(f.checks[0].ticket, "G-1");
        assert_eq!(f.checks[0].progress, 0.5);
        assert_eq!(f.checks[0].hire, "dev");

        assert_eq!(f.review.len(), 3);
        let review = |id: &str| f.review.iter().find(|w| w.ticket == id).unwrap();
        assert_eq!(review("G-2").hire, "cri");
        assert_eq!(review("G-2").role, Role::Critic);
        assert_eq!(review("G-2").progress, 0.75);
        // No judge known: the task's own hire.
        assert_eq!(review("G-3").hire, "dev");
        assert_eq!(review("G-3").role, Role::Engineer);
        // A task that asked in review waits there.
        assert_eq!(review("G-4").doing, Doing::Asking("Run a command".into()));
    }
}
