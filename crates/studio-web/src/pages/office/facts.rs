//! The office's rules, written once: what needs the owner, how a ticket's
//! state reads in words and colour, which colour a project has, and what
//! counts as shipped or at work. The top bar, the floor, the corners and the
//! panel all ask here, so they cannot tell different stories.

use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketState};
use studiod::api::AskView;

use super::Office;
use super::floor::shipped;
use super::model::TINTS;
use crate::ui;

// ------------------------------------------------------------ needs you

/// What waits for the owner: open asks, and tickets held for a reason that
/// doesn't end on its own. A hold for budget or quota starts again by itself,
/// so it waits apart.
pub struct Needs<'a> {
    pub asks: &'a [AskView],
    /// Held for the owner.
    pub held: Vec<&'a Ticket>,
    /// Held until money or the provider is back.
    pub on_hold: Vec<&'a Ticket>,
}

/// What needs the owner in `office`.
pub fn needs_you(office: &Office) -> Needs<'_> {
    let (held, on_hold) = office
        .tickets
        .iter()
        .filter(|t| t.state.kind() == "held")
        .partition(|t| ui::needs_owner(&t.state));
    Needs {
        asks: &office.asks,
        held,
        on_hold,
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

// ------------------------------------------------------- state in words

/// A ticket's state as the Projects tab words it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateWords {
    /// The tick's modifier class.
    pub tick: &'static str,
    /// The state in plain words.
    pub word: &'static str,
    /// The word's text colour, from the state's tone.
    pub tone: &'static str,
}

/// How `state` reads on a task row. A hold that ends on its own reads "on
/// hold"; one that waits for the owner reads "needs you", as asking does.
pub fn state_words(state: &TicketState) -> StateWords {
    let (tick, word) = match ui::shown(state) {
        "landed" => ("done", "landed"),
        "failed" => ("bad", "failed"),
        "cancelled" => ("held", "cancelled"),
        "asking" | "held" => ("needs", "needs you"),
        "on hold" => ("held", "on hold"),
        "claimed" | "working" => ("now", "working"),
        "checking" => ("now", "checking"),
        "reviewing" => ("now", "in review"),
        "ready" => ("ready", "ready to land"),
        _ => ("", "up next"),
    };
    StateWords {
        tick,
        word,
        tone: text_tone(ui::look(ui::shown(state)).tone),
    }
}

/// The text colour class of a tone (`ui::Look::tone`).
fn text_tone(tone: &str) -> &'static str {
    match tone {
        "attn" => "text-attn-fg",
        "run" => "text-run-fg",
        "ok" => "text-ok-fg",
        "bad" => "text-bad-fg",
        _ => "text-fg-3",
    }
}

/// The state an event leaves a ticket looking like, as `ui::look` takes it:
/// for the glyph beside an event, and for the colour of its dot.
pub fn event_state(event: &TicketEvent) -> &'static str {
    match event {
        TicketEvent::Created { .. } | TicketEvent::Retried | TicketEvent::Conflicted { .. } => {
            "queued"
        }
        TicketEvent::Claimed | TicketEvent::RunStarted { .. } | TicketEvent::Resumed => "working",
        TicketEvent::AskOpened { .. } => "asking",
        TicketEvent::AskResolved { approved: true, .. }
        | TicketEvent::ChecksPassed
        | TicketEvent::ReviewPassed => "ready",
        TicketEvent::AskResolved { .. }
        | TicketEvent::ChecksFailed { .. }
        | TicketEvent::ReviewFailed { .. }
        | TicketEvent::Failed { .. }
        | TicketEvent::ChangesRequested { .. } => "failed",
        TicketEvent::ChecksStarted => "checking",
        TicketEvent::Held { .. } => "on hold",
        TicketEvent::Landed => "landed",
        TicketEvent::Cancelled => "cancelled",
    }
}

/// The `.dot` colour of a state: `run`, `ok`, `needs`, `bad` or `held`.
pub fn dot(state: &str) -> &'static str {
    match ui::look(state).tone {
        "run" => "run",
        "ok" => "ok",
        "attn" => "needs",
        "bad" => "bad",
        _ => "held",
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

/// Tickets of `project` with an agent at work: not paused on an ask.
pub fn working_in(office: &Office, project: &str) -> usize {
    office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project && ui::at_work(t.state.kind()))
        .count()
}

/// Tickets that landed in the window, newest first, with when. A landed plan
/// only filed its goal's tickets, so it isn't one of them.
pub fn landed(office: &Office) -> Vec<(Ticket, u64)> {
    shipped(office)
        .into_iter()
        .filter(|(t, _)| t.state.kind() == "landed")
        .collect()
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
        assert_eq!(ids(&needs.on_hold), ["S-2"]);
        // Every ask counts, however many a ticket has.
        assert_eq!(needs.count(), 5);
        assert_eq!(needs.in_project(&office, "shop"), 3);
        assert_eq!(needs.in_project(&office, "api"), 2);
        assert_eq!(needs.in_project(&office, "other"), 0);
        assert_eq!(needs_you(&Office::default()).count(), 0);
    }

    #[test]
    fn a_task_reads_by_its_state() {
        let words = |state: Value| {
            let w = state_words(&ticket("G-1", "game", state).state);
            w.word
        };
        assert_eq!(words(json!("landed")), "landed");
        assert_eq!(
            words(json!({"failed": {"class": "crash", "detail": ""}})),
            "failed"
        );
        assert_eq!(words(json!("cancelled")), "cancelled");
        assert_eq!(words(json!({"asking": {"ask": "a"}})), "needs you");
        assert_eq!(words(json!({"held": {"reason": "paused"}})), "needs you");
        assert_eq!(words(json!({"held": {"reason": "quota"}})), "on hold");
        assert_eq!(words(json!("claimed")), "working");
        assert_eq!(words(json!("working")), "working");
        assert_eq!(words(json!("checking")), "checking");
        assert_eq!(words(json!("reviewing")), "in review");
        assert_eq!(words(json!("ready")), "ready to land");
        assert_eq!(words(json!("queued")), "up next");
    }

    #[test]
    fn an_event_has_one_state_and_one_dot() {
        let event = |value: Value| -> TicketEvent { serde_json::from_value(value).unwrap() };
        let dot_of = |value: Value| dot(event_state(&event(value)));
        assert_eq!(dot_of(json!({"type": "claimed"})), "run");
        assert_eq!(dot_of(json!({"type": "checks_started"})), "run");
        assert_eq!(dot_of(json!({"type": "ask_opened", "ask": "a"})), "needs");
        assert_eq!(
            dot_of(json!({"type": "ask_resolved", "ask": "a", "approved": true})),
            "ok"
        );
        assert_eq!(
            dot_of(json!({"type": "ask_resolved", "ask": "a", "approved": false})),
            "bad"
        );
        assert_eq!(
            dot_of(json!({"type": "checks_failed", "detail": ""})),
            "bad"
        );
        assert_eq!(dot_of(json!({"type": "landed"})), "ok");
        // A hold and a cancel are quiet whatever the reason.
        assert_eq!(dot_of(json!({"type": "held", "reason": "paused"})), "held");
        assert_eq!(dot_of(json!({"type": "held", "reason": "budget"})), "held");
        assert_eq!(dot_of(json!({"type": "cancelled"})), "held");
        assert_eq!(
            dot_of(
                json!({"type": "created", "id": "G-1", "project": "g", "title": "t", "goal": "", "hire": "dev", "base_branch": "main", "created_at": 0})
            ),
            "held"
        );
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
    fn work_counts_by_project() {
        let office = Office {
            tickets: vec![
                ticket("S-1", "shop", json!("working")),
                ticket("S-2", "shop", json!("reviewing")),
                // Paused on an ask: it holds a desk but isn't at work.
                ticket("S-3", "shop", json!({"asking": {"ask": "a"}})),
                ticket("S-4", "shop", json!("queued")),
                ticket("A-1", "api", json!("checking")),
            ],
            ..Office::default()
        };
        assert_eq!(working_in(&office, "shop"), 2);
        assert_eq!(working_in(&office, "api"), 1);
        assert!(landed(&Office::default()).is_empty());
    }
}
