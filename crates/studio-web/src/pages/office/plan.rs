//! Who is where on the isometric floor, from studiod's state.

use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{HeldReason, Ticket, TicketState};

use super::floor::{self, Room};
use super::model::{Crate, Desk, Doing, Floor, Note, Person, Role, Worker};
use super::panel::Extra;
use super::{Office, key};
use crate::words;

/// Desks the office can grow to; at this many there is no "+" spot.
const MOST_DESKS: usize = 9;
const MINUTE_MS: u64 = 60_000;

/// The floor for `office`, with what the panel read beyond it: the
/// trophies and the rule files.
pub fn floor(office: &Office, extra: &Extra) -> Floor {
    let closed = office.status.closed;
    let seated = at_desks(office);
    let slots = usize::try_from(office.status.max_runs)
        .unwrap_or(usize::MAX)
        .max(seated.len());
    let desks: Vec<Desk> = (0..slots)
        .map(|i| Desk {
            worker: seated.get(i).cloned(),
        })
        .collect();
    let in_room = |which: Room| -> Vec<&Ticket> {
        office
            .tickets
            .iter()
            .filter(|t| floor::room(t) == which)
            .collect()
    };
    let checks = in_room(Room::Checks)
        .into_iter()
        .map(|t| worker(office, t, t.hire.to_string(), 0.5))
        .collect();
    let review = in_room(Room::Review)
        .into_iter()
        .map(|t| judged(office, t))
        .collect();
    let finished = floor::shipped(office);
    let ended = |state: &str| -> Vec<Crate> {
        finished
            .iter()
            .filter(|(t, _)| t.state.kind() == state)
            .map(|(t, _)| crate_of(t))
            .collect()
    };
    let mut planners: Vec<Person> = Vec::new();
    for ticket in in_room(Room::Planning) {
        let hire = ticket.hire.to_string();
        if !planners.iter().any(|p| p.hire == hire) {
            planners.push(person(office, hire, None));
        }
    }
    Floor {
        room_for_desk: desks.len() < MOST_DESKS,
        desks,
        checks,
        review,
        ready: in_room(Room::Ready).into_iter().map(crate_of).collect(),
        shipped: ended("landed"),
        returns: ended("failed"),
        goals: floor::goals(office)
            .into_iter()
            .map(|g| Note {
                key: key("goals", &g.plan.id.to_string()),
                project: g.plan.project.to_string(),
                title: g.name(),
                landed: g.landed(),
                total: g.filed.len(),
                done: g.state() == "done",
            })
            .collect(),
        planners,
        lounge: floor::resting(office)
            .into_iter()
            .map(|m| {
                let idle = floor::idle_since(office, &m.id)
                    .map(|at| office.now.saturating_sub(at) / MINUTE_MS);
                person(office, m.id, idle)
            })
            .collect(),
        front: floor::front_desk(office)
            .into_iter()
            .map(|m| person(office, m.id, None))
            .collect(),
        queued: in_room(Room::UpNext).len(),
        closed,
        projects: office.status.projects.clone(),
        trophies: extra
            .trophies
            .iter()
            .filter(|t| t.earned_at.is_some())
            .count(),
        rules: extra.rules.len(),
        hires: office
            .status
            .team
            .iter()
            .filter(|m| !floor::FRONT.contains(&m.role.as_str()))
            .count(),
    }
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
    office
        .member(hire)
        .map_or(Role::Other, |m| Role::of(&m.role))
}

fn person(office: &Office, hire: String, idle_minutes: Option<u64>) -> Person {
    Person {
        role: role_of(office, &hire),
        hire,
        idle_minutes,
    }
}

fn crate_of(ticket: &Ticket) -> Crate {
    Crate {
        ticket: ticket.id.to_string(),
        project: ticket.project.to_string(),
        title: ticket.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Happened, Member};
    use crate::summary::Judge;
    use serde_json::{Value, json};
    use studiod::api::AskView;

    fn ticket(id: &str, state: Value, extra: Value) -> Ticket {
        let mut t = json!({
            "id": id, "project": "game", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
            t.extend(extra.clone());
        }
        serde_json::from_value(t).unwrap()
    }

    fn work(id: &str, state: Value) -> Ticket {
        ticket(id, state, json!({}))
    }

    fn plan(id: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({"kind": "plan"});
        if let (Some(f), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            f.extend(extra.clone());
        }
        ticket(id, state, fields)
    }

    fn member(id: &str, role: &str) -> Member {
        Member {
            id: id.into(),
            role: role.into(),
            model: "m".into(),
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

    fn office(max_runs: u64, tickets: Vec<Ticket>) -> Office {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status.max_runs = max_runs;
        office.status.team = vec![
            member("dev", "engineer"),
            member("ana", "writer"),
            member("pro", "producer"),
            member("rev", "reviewer"),
            member("cri", "critic"),
            member("boss", "approver"),
            member("rout", "router"),
        ];
        office
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

    fn seated(f: &Floor) -> Vec<&Worker> {
        f.desks.iter().filter_map(|d| d.worker.as_ref()).collect()
    }

    #[test]
    fn desks_follow_max_runs_and_grow_to_fit_workers() {
        let free = floor(
            &office(3, vec![work("G-1", json!("working"))]),
            &Extra::default(),
        );
        assert_eq!(free.desks.len(), 3);
        assert_eq!(free.desks.iter().filter(|d| d.worker.is_none()).count(), 2);
        assert!(free.room_for_desk);

        let crowded = floor(
            &office(
                1,
                vec![work("G-1", json!("working")), work("G-2", json!("claimed"))],
            ),
            &Extra::default(),
        );
        assert_eq!(crowded.desks.len(), 2);
        assert!(crowded.desks.iter().all(|d| d.worker.is_some()));

        let full = floor(&office(9, vec![]), &Extra::default());
        assert_eq!(full.desks.len(), 9);
        assert!(!full.room_for_desk);
        assert!(floor(&office(8, vec![]), &Extra::default()).room_for_desk);
    }

    #[test]
    fn workers_sit_oldest_claim_first_then_by_id() {
        let mut o = office(
            4,
            vec![
                work("G-4", json!("working")),
                work("G-2", json!("working")),
                work("G-3", json!("working")),
                work("G-1", json!("working")),
            ],
        );
        o.recent = vec![
            happened("G-3", 10, json!({"type": "claimed"})),
            happened("G-1", 20, json!({"type": "claimed"})),
        ];
        let f = floor(&o, &Extra::default());
        let order: Vec<&str> = seated(&f).iter().map(|w| w.ticket.as_str()).collect();
        // No claim in the window means an older claim; those go first, by id.
        assert_eq!(order, ["G-2", "G-4", "G-3", "G-1"]);
    }

    #[test]
    fn only_desk_tickets_sit_at_desks() {
        let o = office(
            5,
            vec![
                work("G-1", json!("working")),
                work("G-2", json!("checking")),
                work("G-3", json!("reviewing")),
                work("G-4", json!("ready")),
                work("G-5", json!("queued")),
                plan("G-6", json!("working"), json!({})),
            ],
        );
        let f = floor(&o, &Extra::default());
        let at = seated(&f);
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].ticket, "G-1");
        assert_eq!(at[0].progress, 0.25);
        assert_eq!(at[0].role, Role::Engineer);
        assert_eq!(at[0].hire, "dev");
        assert_eq!(at[0].project, "game");
        assert_eq!(at[0].title, "Title G-1");
        assert_eq!(at[0].doing, Doing::Working);
        assert_eq!(f.queued, 1);
    }

    #[test]
    fn asking_and_held_say_what_they_want() {
        let mut o = office(
            7,
            vec![
                work("G-1", json!({"asking": {"ask": "a1"}})),
                work("G-2", json!({"held": {"reason": "budget"}})),
                work("G-3", json!({"held": {"reason": "quota"}})),
                work("G-4", json!({"held": {"reason": "auth"}})),
                work("G-5", json!({"held": {"reason": "paused"}})),
                work("G-6", json!("claimed")),
                work("G-7", json!({"asking": {"ask": "a9"}})),
            ],
        );
        o.asks = vec![ask("G-1")];
        let f = floor(&o, &Extra::default());
        let doing = |id: &str| {
            seated(&f)
                .into_iter()
                .find(|w| w.ticket == id)
                .map(|w| w.doing.clone())
                .unwrap()
        };
        assert_eq!(doing("G-1"), Doing::Asking("Run a command".into()));
        assert_eq!(doing("G-2"), Doing::Held("budget".into()));
        assert_eq!(doing("G-3"), Doing::Held("quota".into()));
        assert_eq!(doing("G-4"), Doing::Held("login".into()));
        assert_eq!(doing("G-5"), Doing::Held("paused".into()));
        assert_eq!(doing("G-6"), Doing::Working);
        // An ask studiod no longer lists still shows the ticket needs you.
        assert_eq!(doing("G-7"), Doing::Asking("Needs you".into()));
    }

    #[test]
    fn checks_and_review_hold_their_tickets() {
        let mut o = office(
            3,
            vec![
                work("G-1", json!("checking")),
                work("G-2", json!("reviewing")),
                work("G-3", json!("reviewing")),
                ticket(
                    "G-4",
                    json!({"asking": {"ask": "a1"}}),
                    json!({"held_from": "reviewing"}),
                ),
            ],
        );
        o.asks = vec![ask("G-4")];
        o.judges.insert("G-2".into(), judge("cri"));
        let f = floor(&o, &Extra::default());
        assert!(seated(&f).is_empty());
        assert_eq!(f.checks.len(), 1);
        assert_eq!(f.checks[0].ticket, "G-1");
        assert_eq!(f.checks[0].progress, 0.5);
        assert_eq!(f.checks[0].hire, "dev");

        assert_eq!(f.review.len(), 3);
        let review = |id: &str| f.review.iter().find(|w| w.ticket == id).unwrap();
        assert_eq!(review("G-2").hire, "cri");
        assert_eq!(review("G-2").role, Role::Critic);
        assert_eq!(review("G-2").progress, 0.75);
        // No judge known: the ticket's own hire.
        assert_eq!(review("G-3").hire, "dev");
        assert_eq!(review("G-3").role, Role::Engineer);
        // A ticket that asked in review waits there.
        assert_eq!(review("G-4").doing, Doing::Asking("Run a command".into()));
    }

    #[test]
    fn unknown_hires_are_other() {
        let o = office(
            1,
            vec![ticket("G-1", json!("working"), json!({"hire": "ghost"}))],
        );
        let f = floor(&o, &Extra::default());
        assert_eq!(seated(&f)[0].role, Role::Other);
    }

    #[test]
    fn the_dock_holds_ready_shipped_and_returned_tickets() {
        let mut o = office(
            1,
            vec![
                work("G-1", json!("ready")),
                work("G-2", json!("landed")),
                work("G-3", json!("landed")),
                work("G-4", json!({"failed": {"class": "checks", "detail": "x"}})),
                work("G-5", json!("cancelled")),
                work("G-6", json!("landed")),
            ],
        );
        o.recent = vec![
            happened("G-2", 100, json!({"type": "landed"})),
            happened("G-3", 300, json!({"type": "landed"})),
            happened(
                "G-4",
                200,
                json!({"type": "failed", "class": "checks", "detail": "x"}),
            ),
            happened("G-5", 250, json!({"type": "cancelled"})),
        ];
        let f = floor(&o, &Extra::default());
        assert_eq!(f.ready.len(), 1);
        assert_eq!(f.ready[0].ticket, "G-1");
        assert_eq!(f.ready[0].title, "Title G-1");
        let shipped: Vec<&str> = f.shipped.iter().map(|c| c.ticket.as_str()).collect();
        // Newest first; G-6 landed before the window and cancelled is no crate.
        assert_eq!(shipped, ["G-3", "G-2"]);
        let returns: Vec<&str> = f.returns.iter().map(|c| c.ticket.as_str()).collect();
        assert_eq!(returns, ["G-4"]);
    }

    #[test]
    fn goals_become_notes_keyed_for_the_inspector() {
        let mut o = office(
            1,
            vec![
                plan("P-1", json!("landed"), json!({"goal": "Ship waves\nmore"})),
                ticket("G-1", json!("landed"), json!({"parent": "P-1"})),
                ticket("G-2", json!("working"), json!({"parent": "P-1"})),
                plan("P-2", json!("landed"), json!({"goal": "Polish"})),
                ticket("G-3", json!("landed"), json!({"parent": "P-2"})),
                plan("P-3", json!("landed"), json!({"goal": "Merged"})),
                ticket("G-4", json!("landed"), json!({"parent": "P-3"})),
                plan(
                    "P-4",
                    json!({"failed": {"class": "crash", "detail": "y"}}),
                    json!({}),
                ),
            ],
        );
        o.merged = vec!["G-4".into()];
        let f = floor(&o, &Extra::default());
        assert_eq!(f.goals.len(), 2);
        let waves = &f.goals[0];
        assert_eq!(waves.key, "goals/P-1");
        assert_eq!(waves.title, "Ship waves");
        assert_eq!(waves.project, "game");
        assert_eq!((waves.landed, waves.total, waves.done), (1, 2, false));
        let polish = &f.goals[1];
        assert_eq!(polish.key, "goals/P-2");
        assert_eq!((polish.landed, polish.total, polish.done), (1, 1, true));
    }

    #[test]
    fn producers_plan_and_the_rest_rest() {
        let mut o = office(
            2,
            vec![
                plan("P-1", json!("working"), json!({"hire": "pro"})),
                work("G-1", json!("working")),
                work("G-2", json!("reviewing")),
            ],
        );
        o.judges.insert("G-2".into(), judge("rev"));
        let f = floor(&o, &Extra::default());
        assert_eq!(f.planners.len(), 1);
        assert_eq!(f.planners[0].hire, "pro");
        assert_eq!(f.planners[0].role, Role::Producer);
        assert_eq!(f.planners[0].idle_minutes, None);
        // dev is at a desk, pro plans, rev judges; the front desk stays out.
        let lounge: Vec<&str> = f.lounge.iter().map(|p| p.hire.as_str()).collect();
        assert_eq!(lounge, ["ana", "cri"]);
        assert_eq!(f.lounge[0].role, Role::Writer);

        let front: Vec<(&str, Role)> = f.front.iter().map(|p| (p.hire.as_str(), p.role)).collect();
        assert_eq!(front, [("boss", Role::Approver), ("rout", Role::Router)]);
    }

    #[test]
    fn the_lounge_counts_idle_minutes() {
        let mut o = office(1, vec![work("G-1", json!("checking"))]);
        o.recent = vec![happened(
            "G-1",
            o.now - 7 * 60_000 - 5_000,
            json!({"type": "checks_started"}),
        )];
        let f = floor(&o, &Extra::default());
        let dev = f.lounge.iter().find(|p| p.hire == "dev").unwrap();
        assert_eq!(dev.idle_minutes, Some(7));
        let ana = f.lounge.iter().find(|p| p.hire == "ana").unwrap();
        assert_eq!(ana.idle_minutes, None);
    }

    #[test]
    fn the_rest_is_carried_over() {
        let mut o = office(
            1,
            vec![work("G-1", json!("queued")), work("G-2", json!("queued"))],
        );
        o.status.projects = vec!["game".into(), "site".into()];
        assert!(!floor(&o, &Extra::default()).closed);
        o.status.closed = true;
        let f = floor(&o, &Extra::default());
        assert_eq!(f.queued, 2);
        assert!(f.closed);
        assert_eq!(f.projects, ["game", "site"]);
        assert_eq!(f.tint("site"), 1);
    }
}
