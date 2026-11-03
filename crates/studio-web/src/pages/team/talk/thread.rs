//! What a conversation's thread holds: the messages kept for the project, and
//! the cards of what is true now (questions waiting, work ready to ship, a
//! release waiting, the day's numbers), merged by time.

use std::collections::HashMap;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{
    CheckState, ConversationView, ReleaseState, SetupState, SetupView, TalkCard, TalkFrom,
    TalkMessage,
};

use crate::pages::office::model::Role;
use crate::pages::office::{Office, facts, floor};
use crate::pages::team::{goal_result, hq, task_result, version};
use crate::ui;

use super::asked;

/// A day, in milliseconds: what "today" covers.
const DAY_MS: u64 = 86_400_000;

/// How close in time the same voice's words are to run together, in milliseconds.
const TOGETHER_MS: u64 = 5 * 60_000;

/// Lines added and removed by a goal that is ready to ship, by its plan
/// ticket's id.
pub type Sizes = HashMap<String, (u64, u64)>;

/// A person as the thread draws them: a face and a name.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub hire: String,
    pub name: String,
    pub role: Role,
}

impl Person {
    /// The hire `hire`, called by its name, in the shirt of its role.
    pub fn of(office: &Office, hire: &str) -> Person {
        let role = if facts::leads(office, hire) {
            Role::Lead
        } else {
            office
                .member(hire)
                .map_or(Role::Other, |member| Role::of(&member.role))
        };
        Person {
            hire: hire.to_string(),
            name: office.name_of(hire),
            role,
        }
    }
}

/// Who says something.
#[derive(Clone, Debug, PartialEq)]
pub enum Voice {
    Owner,
    Lead(Person),
    /// The studio itself: the project has no lead to speak for it.
    Studio,
}

/// One thing in the thread: some words, and what shows under them.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The same voice spoke just before: no face or name over it again.
    pub follows: bool,
    /// A stable name for the element, so a patch keeps what it has.
    pub id: String,
    /// When, in epoch milliseconds: where it stands in the thread.
    pub at: u64,
    /// When it says it was said, when that is not `at`: a standup stands at
    /// the day's start but was written when it was written, as HQ says.
    pub stamped: Option<u64>,
    pub voice: Voice,
    pub text: String,
    pub cards: Vec<Card>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    Plan(Plan),
    Acted(Acted),
    /// How the project runs, as the lead proposed it and the studio tried it.
    Setup {
        view: Box<SetupView>,
        /// What the lead is called.
        lead: String,
    },
    Ask(Question),
    Ship(Ship),
    Release(Release),
    Stats(Stats),
}

/// A goal the lead planned, with its tasks.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub title: String,
    pub version: Option<String>,
    pub tasks: Vec<PlanRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanRow {
    pub title: String,
    pub who: Option<Person>,
    /// Where the task stands, in a word; empty when it can't be told.
    pub status: &'static str,
    /// What the task came to once it is done, in a line; empty before.
    pub result: String,
}

/// Something the lead did to a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Acted {
    pub title: String,
    pub action: String,
    pub note: String,
}

/// A question that waits for the owner.
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub ask: String,
    pub ticket: String,
    pub title: String,
    /// Who asks, and about which task.
    pub line: String,
    /// It can be answered with Allow or Deny; any other wants a look at the
    /// task's page.
    pub plain: bool,
}

/// What the checks of work ready to ship came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Checks {
    Passed,
    Failed(String),
    Running,
    /// Nothing has run them.
    Unknown,
}

/// Work ready to ship: a goal whose tasks are all done, or a task that passed
/// its checks and review.
#[derive(Clone, Debug, PartialEq)]
pub struct Ship {
    /// What `?look=` opens: a goal's plan ticket or the task.
    pub look: String,
    pub title: String,
    pub lines: Option<(u64, u64)>,
    pub checks: Checks,
    /// Whether a reviewer read it; none when that isn't told.
    pub reviewed: Option<bool>,
    /// What the work came to, in a line; empty when its agents said none.
    pub result: String,
    /// Where Ship posts.
    pub post: String,
}

impl Ship {
    /// "+79 −8 · checks passed · reviewed", the parts there are to say.
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        match &self.checks {
            Checks::Passed => notes.push("checks passed".to_string()),
            Checks::Failed(why) if why.is_empty() => notes.push("checks failed".to_string()),
            Checks::Failed(why) => notes.push(format!("checks failed: {why}")),
            Checks::Running => notes.push("checks running".to_string()),
            Checks::Unknown => {}
        }
        match self.reviewed {
            Some(true) => notes.push("reviewed".to_string()),
            Some(false) => notes.push("not reviewed".to_string()),
            None => {}
        }
        notes
    }
}

/// A release that waits for the owner's go-ahead.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub project: String,
    /// As the release has it.
    pub version: String,
}

/// The day's numbers for the project.
#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub done: usize,
    pub working: usize,
    pub sent_back: usize,
    pub for_you: usize,
}

/// The person who speaks for `project`: its lead.
pub fn lead_of(office: &Office, project: &str) -> Option<Person> {
    let hire = office.project(project)?.lead.as_deref()?;
    Some(Person::of(office, hire))
}

/// The work whose sizes the thread wants and the office doesn't give: the
/// goals ready to ship, by their plan tickets. A goal's size is what its
/// branch adds to the base it ships to, which studiod reads from git when its
/// tasks' worktrees are gone.
pub fn wanted_sizes(office: &Office, project: &str) -> Vec<String> {
    ready_goals(office, project)
        .into_iter()
        .map(|goal| goal.plan.id.to_string())
        .collect()
}

/// The goals of `project` whose tasks are all done and not yet shipped.
pub fn ready_goals(office: &Office, project: &str) -> Vec<floor::Goal> {
    floor::all_goals(office)
        .into_iter()
        .filter(|goal| goal.plan.project.to_string() == project && goal.state() == "done")
        .collect()
}

/// The tasks of `project` that passed their checks and review and wait to be
/// added.
pub fn ready_tasks<'a>(office: &'a Office, project: &str) -> Vec<&'a Ticket> {
    office
        .tickets
        .iter()
        .filter(|t| {
            t.project.to_string() == project
                && t.kind != TicketKind::Plan
                && t.state.kind() == "ready"
        })
        .collect()
}

/// The thread of `project`: what was said, and what is true now, by time.
pub fn thread(
    office: &Office,
    project: &str,
    convo: &ConversationView,
    sizes: &Sizes,
) -> Vec<Item> {
    let lead = lead_of(office, project);
    let voice = || lead.clone().map_or(Voice::Studio, Voice::Lead);
    let mut items: Vec<Item> = convo
        .messages
        .iter()
        .map(|message| said(office, message))
        .collect();

    for ask in office.asks.iter().filter(|a| {
        office
            .ticket(&a.ticket)
            .is_some_and(|t| t.project.to_string() == project)
    }) {
        let task = office.ticket(&ask.ticket);
        let by = office.name_of(&ask.hire);
        let line = match task {
            Some(task) => format!("{by} · {}", task.title),
            None => by.clone(),
        };
        let title = asked::title(ask);
        items.push(Item {
            follows: false,
            stamped: None,
            id: format!("live-ask-{}", ask.id),
            at: if ask.at_ms > 0 { ask.at_ms } else { office.now },
            voice: voice(),
            text: format!("{by} is waiting for you."),
            cards: vec![Card::Ask(Question {
                ask: ask.id.clone(),
                ticket: ask.ticket.clone(),
                title,
                line,
                plain: ask.kind == "permission",
            })],
        });
    }

    for goal in ready_goals(office, project) {
        let id = goal.plan.id.to_string();
        let Some(landed) = goal.filed.iter().find(|t| t.state.kind() == "landed") else {
            continue;
        };
        let at = goal
            .filed
            .iter()
            .filter_map(|t| office.last(&t.id.to_string(), |e| matches!(e, TicketEvent::Landed)))
            .max()
            .unwrap_or(goal.plan.created_at);
        let lines = sizes
            .get(&id)
            .copied()
            .filter(|(added, removed)| added + removed > 0);
        let checks = match office.ship_checks.get(&id) {
            Some(check) => match check.state {
                CheckState::Passed => Checks::Passed,
                CheckState::Failed => Checks::Failed(check.summary.clone()),
                CheckState::Running => Checks::Running,
            },
            None => Checks::Unknown,
        };
        let name = goal.name();
        let text = match &checks {
            Checks::Passed => format!("{name} is done and checked."),
            Checks::Failed(_) => format!("{name} is done, but its checks failed."),
            Checks::Running => format!("{name} is done. I'm checking it."),
            Checks::Unknown => format!("{name} is done."),
        };
        items.push(Item {
            follows: false,
            stamped: None,
            id: format!("live-ship-{id}"),
            at,
            voice: voice(),
            text,
            cards: vec![Card::Ship(Ship {
                post: format!("/tickets/{}/merge", landed.id),
                result: goal_result(office, &goal),
                look: id,
                title: name,
                lines,
                checks,
                reviewed: None,
            })],
        });
    }

    for task in ready_tasks(office, project) {
        let id = task.id.to_string();
        let at = office
            .last(&id, |e| {
                matches!(e, TicketEvent::ReviewPassed | TicketEvent::ChecksPassed)
            })
            .unwrap_or(task.created_at);
        items.push(Item {
            follows: false,
            stamped: None,
            id: format!("live-ship-{id}"),
            at,
            voice: voice(),
            text: format!("{} is ready.", task.title),
            cards: vec![Card::Ship(Ship {
                post: format!("/tickets/{id}/land"),
                look: id.clone(),
                title: task.title.clone(),
                lines: office
                    .sizes
                    .get(&id)
                    .copied()
                    .filter(|(added, removed)| added + removed > 0),
                checks: Checks::Passed,
                reviewed: Some(!office.unreviewed.contains(&id)),
                result: task_result(office, &id),
            })],
        });
    }

    if let Some(release) = office
        .project(project)
        .and_then(|p| p.release.as_ref())
        .filter(|r| r.state == ReleaseState::Waiting)
    {
        items.push(Item {
            follows: false,
            stamped: None,
            id: format!("live-release-{}", release.version),
            at: release.started_at,
            voice: voice(),
            text: format!("{} is ready to publish.", version(&release.version)),
            cards: vec![Card::Release(Release {
                project: project.to_string(),
                version: release.version.clone(),
            })],
        });
    }

    // A setup that is still going on, or that failed, has no card among the
    // messages: the lead posts it once the setup is ready. Until then the card
    // is as live as the setup, so the owner sees the lead at work and, when it
    // failed, can try again.
    if let Some(view) = office.setups.get(project).filter(|view| {
        matches!(
            view.state,
            SetupState::Reading | SetupState::Checking | SetupState::Failed
        )
    }) {
        items.push(Item {
            follows: false,
            stamped: None,
            id: "live-setup".to_string(),
            at: office.now,
            voice: voice(),
            text: String::new(),
            cards: vec![setup(office, view)],
        });
    }

    // The day's numbers come before the day's first word, with the lead's own
    // words for the day, its standup, when it has written one. Four zeros say
    // nothing, so the numbers show only when something is counted, and a day
    // with neither numbers nor a standup has nothing to show.
    let numbers = stats(office, project);
    let standup = office
        .standups
        .get(project)
        .filter(|standup| !standup.text.trim().is_empty());
    let counted = numbers.done + numbers.working + numbers.sent_back + numbers.for_you > 0;
    if counted || standup.is_some() {
        let since = office.now.saturating_sub(DAY_MS);
        let first = items.iter().map(|i| i.at).filter(|at| *at >= since).min();
        items.push(Item {
            follows: false,
            stamped: standup.map(|standup| standup.at),
            id: "live-stats".to_string(),
            at: first.map_or(office.now, |at| at.saturating_sub(1)),
            voice: voice(),
            text: standup.map_or_else(
                || format!("Here's {project} today:"),
                |standup| standup.text.trim().to_string(),
            ),
            cards: counted
                .then_some(Card::Stats(numbers))
                .into_iter()
                .collect(),
        });
    }

    // Equal times keep the order they were put in: what was said first.
    items.sort_by_key(|item| item.at);
    let mut items = together(items);
    for n in 1..items.len() {
        let (before, now) = (&items[n - 1], &items[n]);
        let close = now.at.saturating_sub(before.at) < TOGETHER_MS;
        items[n].follows = close && now.voice == before.voice && now.voice != Voice::Owner;
    }
    items
}

/// Whether an item is something that waits for the owner: a question, work
/// ready to ship or a release, as the studio finds them now.
fn waits(item: &Item) -> bool {
    ["live-ask-", "live-ship-", "live-release-"]
        .iter()
        .any(|kind| item.id.starts_with(kind))
}

/// Cards that wait for the owner and come one after the other, with nothing
/// said between them, stand under one line.
fn together(items: Vec<Item>) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    let mut run: Vec<Item> = Vec::new();
    for item in items {
        if waits(&item) {
            run.push(item);
        } else {
            out.extend(joined(std::mem::take(&mut run)));
            out.push(item);
        }
    }
    out.extend(joined(run));
    out
}

/// One line for what waits together: "Three things wait for you:" or "Two
/// pieces of work are ready to ship:", with the cards under it. A single item
/// keeps its own line.
fn joined(mut run: Vec<Item>) -> Option<Item> {
    if run.len() < 2 {
        return run.pop();
    }
    let ships = run.iter().all(|item| item.id.starts_with("live-ship-"));
    let many = number(run.len());
    let first = &run[0];
    let (id, at, voice) = (
        format!("live-group-{}", first.id),
        first.at,
        first.voice.clone(),
    );
    Some(Item {
        follows: false,
        stamped: None,
        id,
        at,
        voice,
        text: if ships {
            format!("{many} pieces of work are ready to ship:")
        } else {
            format!("{many} things wait for you:")
        },
        cards: run.into_iter().flat_map(|item| item.cards).collect(),
    })
}

/// A count as it starts a sentence: "Two", "Three", ... and digits past ten.
fn number(n: usize) -> String {
    const WORDS: [&str; 9] = [
        "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
    ];
    n.checked_sub(2)
        .and_then(|i| WORDS.get(i))
        .map_or_else(|| n.to_string(), |word| (*word).to_string())
}

/// A stored message as the thread shows it.
fn said(office: &Office, message: &TalkMessage) -> Item {
    let voice = match &message.from {
        TalkFrom::Owner => Voice::Owner,
        TalkFrom::Lead { hire } => Voice::Lead(Person::of(office, hire)),
        TalkFrom::Studio => Voice::Studio,
    };
    let cards: Vec<Card> = message
        .card
        .as_ref()
        .and_then(|card| card_of(office, card))
        .into_iter()
        .collect();
    // What the studio did to a task is told once, by its card.
    let text = match cards.first() {
        Some(Card::Acted(_)) if voice == Voice::Studio => String::new(),
        _ => message.text.clone(),
    };
    Item {
        follows: false,
        stamped: None,
        id: format!("m-{}", message.id),
        at: message.at,
        voice,
        text,
        cards,
    }
}

/// What a message's card shows; none for one whose subject is gone.
fn card_of(office: &Office, card: &TalkCard) -> Option<Card> {
    Some(match card {
        TalkCard::Plan {
            goal,
            title,
            version,
            tasks,
        } => Card::Plan(Plan {
            title: title.clone(),
            version: version.clone(),
            tasks: tasks
                .iter()
                .map(|task| {
                    let filed = office.tickets.iter().find(|t| {
                        t.parent.as_ref().is_some_and(|p| p.to_string() == *goal)
                            && t.title == task.title
                    });
                    PlanRow {
                        title: task.title.clone(),
                        who: task.hire.as_deref().map(|hire| Person::of(office, hire)),
                        status: filed.map_or("", |t| status(t.state.kind())),
                        // What a done task came to is said under it.
                        result: filed
                            .filter(|t| t.state.kind() == "landed")
                            .map(|t| task_result(office, &t.id.to_string()))
                            .unwrap_or_default(),
                    }
                })
                .collect(),
        }),
        TalkCard::Acted {
            title,
            action,
            note,
            ..
        } => Card::Acted(Acted {
            title: title.clone(),
            action: action.clone(),
            note: note.clone(),
        }),
        TalkCard::Setup { project } => setup(office, office.setups.get(project)?),
    })
}

/// The card of a project's setup: its state as the studio has it now, and the
/// lead by name.
fn setup(office: &Office, view: &SetupView) -> Card {
    let lead = view
        .lead
        .as_deref()
        .map(|hire| office.name_of(hire))
        .unwrap_or_default();
    Card::Setup {
        view: Box::new(view.clone()),
        lead,
    }
}

/// Where a task stands, in a word for a plan's list.
fn status(state: &str) -> &'static str {
    match state {
        "landed" => "done",
        "ready" => "ready",
        "claimed" | "working" | "checking" | "reviewing" => "working",
        "asking" | "held" => "waiting",
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ => "next",
    }
}

/// Today's numbers for `project`.
fn stats(office: &Office, project: &str) -> Stats {
    let mine: Vec<&Ticket> = office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project && t.kind != TicketKind::Plan)
        .collect();
    let in_window = |id: &str, pick: &dyn Fn(&TicketEvent) -> bool| {
        office
            .recent
            .iter()
            .any(|e| e.ticket_id == id && pick(&e.event))
    };
    let done = mine
        .iter()
        .filter(|t| in_window(&t.id.to_string(), &|e| matches!(e, TicketEvent::Landed)))
        .count();
    let working = mine.iter().filter(|t| ui::at_work(t.state.kind())).count();
    let sent_back = mine
        .iter()
        .filter(|t| !matches!(t.state.kind(), "landed" | "cancelled"))
        .filter(|t| {
            t.feedback.is_some()
                || in_window(&t.id.to_string(), &|e| {
                    matches!(
                        e,
                        TicketEvent::ReviewFailed { .. } | TicketEvent::ChecksFailed { .. }
                    )
                })
        })
        .count();
    Stats {
        done,
        working,
        sent_back,
        // Whatever waits for the owner, work that is ready to ship too: the
        // number the sidebar gives this lead.
        for_you: facts::project_needs(office, project)
            + hq::ships(office, project, &Sizes::new()).len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::team::LeadEntry;
    use serde_json::{Value, json};
    use studiod::api::{PlanTask, ProjectView, ShipCheck};

    fn ticket(id: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({
            "id": id, "project": "shop", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 1_000,
        });
        if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            fields.extend(extra.clone());
        }
        serde_json::from_value(fields).unwrap()
    }

    fn office(tickets: Vec<Ticket>) -> Office {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects": ["shop"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
            ],
        }))
        .unwrap();
        office.projects = vec![ProjectView {
            name: "shop".into(),
            lead: Some("nora".into()),
            ..ProjectView::default()
        }];
        office
    }

    fn message(
        id: u64,
        at: u64,
        from: TalkFrom,
        text: &str,
        card: Option<TalkCard>,
    ) -> TalkMessage {
        TalkMessage {
            id,
            at,
            from,
            text: text.into(),
            card,
        }
    }

    fn convo(messages: Vec<TalkMessage>) -> ConversationView {
        ConversationView {
            messages,
            thinking: false,
        }
    }

    fn texts(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.text.as_str()).collect()
    }

    fn goal_done() -> Vec<Ticket> {
        vec![
            ticket(
                "S-1",
                json!("landed"),
                json!({"kind": "plan", "goal": "Save carts", "created_at": 5_000}),
            ),
            ticket(
                "S-2",
                json!("landed"),
                json!({"parent": "S-1", "goal": "Save carts"}),
            ),
            ticket(
                "S-3",
                json!("landed"),
                json!({"parent": "S-1", "goal": "Save carts"}),
            ),
        ]
    }

    #[test]
    fn what_was_said_and_what_is_true_now_are_merged_by_time() {
        let mut office = office(vec![ticket(
            "S-9",
            json!("ready"),
            json!({"created_at": 9_000_000}),
        )]);
        office.sizes.insert("S-9".into(), (12, 3));
        let convo = convo(vec![
            message(1, 9_100_000, TalkFrom::Owner, "Test first.", None),
            message(
                2,
                9_200_000,
                TalkFrom::Lead {
                    hire: "nora".into(),
                },
                "Test sent.",
                None,
            ),
        ]);
        let items = thread(&office, "shop", &convo, &Sizes::new());
        // The ready task is older than the words, and the day's numbers come
        // before all of it.
        assert_eq!(
            texts(&items),
            [
                "Here's shop today:",
                "Title S-9 is ready.",
                "Test first.",
                "Test sent."
            ]
        );
        assert_eq!(items[2].voice, Voice::Owner);
        assert!(
            matches!(&items[3].voice, Voice::Lead(p) if p.name == "Nora" && p.role == Role::Lead)
        );
        let Some(Card::Ship(ship)) = items[1].cards.first() else {
            panic!("{:?}", items[1]);
        };
        assert_eq!(ship.lines, Some((12, 3)));
        assert_eq!(ship.post, "/tickets/S-9/land");
        assert_eq!(ship.notes(), ["checks passed", "reviewed"]);
    }

    #[test]
    fn the_same_voice_within_minutes_runs_together_and_the_owner_never_does() {
        let by_lead = |id, at, text| {
            message(
                id,
                at,
                TalkFrom::Lead {
                    hire: "nora".into(),
                },
                text,
                None,
            )
        };
        let convo = convo(vec![
            by_lead(1, 9_000_000, "One."),
            by_lead(2, 9_060_000, "Two, a minute later."),
            message(3, 9_070_000, TalkFrom::Owner, "Owner.", None),
            message(4, 9_080_000, TalkFrom::Owner, "Owner again.", None),
            by_lead(5, 9_090_000, "After the owner."),
            by_lead(6, 9_090_000 + 6 * 60_000, "Six minutes on."),
        ]);
        // A project at work, so the day has its numbers.
        let working = office(vec![ticket("S-1", json!("working"), json!({}))]);
        let items = thread(&working, "shop", &convo, &Sizes::new());
        let together: Vec<(&str, bool)> = items
            .iter()
            .filter(|i| i.id.starts_with("m-"))
            .map(|i| (i.text.as_str(), i.follows))
            .collect();
        assert_eq!(
            together,
            [
                // The day's numbers speak first, just before.
                ("One.", true),
                ("Two, a minute later.", true),
                ("Owner.", false),
                ("Owner again.", false),
                ("After the owner.", false),
                ("Six minutes on.", false),
            ]
        );
    }

    /// The numbers of the day's first item, when it has them.
    fn numbers(items: &[Item]) -> Option<(usize, usize, usize, usize)> {
        items.iter().find_map(|item| match item.cards.first() {
            Some(Card::Stats(s)) => Some((s.done, s.working, s.sent_back, s.for_you)),
            _ => None,
        })
    }

    #[test]
    fn a_day_with_nothing_to_count_shows_no_numbers() {
        // A project whose only work was cancelled: four zeros say nothing.
        let quiet = office(vec![ticket("S-1", json!("cancelled"), json!({}))]);
        let items = thread(&quiet, "shop", &convo(vec![]), &Sizes::new());
        assert!(items.is_empty(), "{items:?}");
        // A plan at work has no task of its own to count yet either.
        let planning = office(vec![ticket(
            "S-1",
            json!("working"),
            json!({"kind": "plan", "goal": "Save carts"}),
        )]);
        let items = thread(&planning, "shop", &convo(vec![]), &Sizes::new());
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn a_day_with_something_counted_has_its_numbers_first() {
        let working = office(vec![ticket("S-1", json!("working"), json!({}))]);
        let items = thread(&working, "shop", &convo(vec![]), &Sizes::new());
        assert_eq!(texts(&items), ["Here's shop today:"]);
        assert_eq!(numbers(&items), Some((0, 1, 0, 0)));
    }

    #[test]
    fn work_ready_to_ship_counts_as_waiting_for_the_owner_as_the_sidebar_says() {
        // A task that passed review and a goal with every task done wait for
        // the owner; the sidebar's badge counts both, so these numbers do.
        let mut tickets = goal_done();
        tickets.push(ticket("S-9", json!("ready"), json!({})));
        let office = office(tickets);
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        assert_eq!(numbers(&items).map(|(.., for_you)| for_you), Some(2));
        let entry = LeadEntry::of(&office, &["shop".to_string()], "shop");
        assert_eq!(entry.needs, 2);
    }

    #[test]
    fn a_new_project_has_no_numbers_to_show_until_it_has_work_or_a_standup() {
        let items = thread(&office(vec![]), "shop", &convo(vec![]), &Sizes::new());
        assert!(items.is_empty(), "{items:?}");
        // The lead's standup is words worth showing, with the numbers.
        let mut office = office(vec![]);
        office.standups.insert(
            "shop".into(),
            studiod::api::Standup {
                at: 1,
                text: "Just getting started.".into(),
            },
        );
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        assert_eq!(texts(&items), ["Just getting started."]);
        // With four zeros to go with them: none.
        assert_eq!(numbers(&items), None);
        // The same words come with the numbers once something is counted.
        office.tickets = vec![ticket("S-1", json!("working"), json!({}))];
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        assert_eq!(texts(&items), ["Just getting started."]);
        assert_eq!(numbers(&items), Some((0, 1, 0, 0)));
    }

    #[test]
    fn a_standup_stands_at_the_days_start_but_says_when_it_was_written() {
        let mut office = office(vec![ticket("S-1", json!("working"), json!({}))]);
        office.standups.insert(
            "shop".into(),
            studiod::api::Standup {
                at: 9_900_000,
                text: "Quiet.".into(),
            },
        );
        let convo = convo(vec![message(1, 9_000_000, TalkFrom::Owner, "Hello.", None)]);
        let items = thread(&office, "shop", &convo, &Sizes::new());
        assert_eq!(texts(&items), ["Quiet.", "Hello."]);
        assert_eq!(
            (items[0].at, items[0].stamped),
            (8_999_999, Some(9_900_000))
        );
        // What was said stands where it was said and says so.
        assert_eq!((items[1].at, items[1].stamped), (9_000_000, None));
    }

    #[test]
    fn a_goal_with_every_task_done_is_ready_to_ship_with_its_checks() {
        let mut office = office(goal_done());
        let mut sizes = Sizes::new();
        sizes.insert("S-1".into(), (79, 8));
        office.ship_checks.insert(
            "S-1".into(),
            ShipCheck {
                state: CheckState::Failed,
                summary: "cargo test: 2 failed".into(),
                at: 0,
            },
        );
        // The goal's size is asked for by its plan ticket.
        assert_eq!(wanted_sizes(&office, "shop"), ["S-1"]);
        let items = thread(&office, "shop", &convo(vec![]), &sizes);
        let Some(ship) = items.iter().find_map(|i| match i.cards.first() {
            Some(Card::Ship(ship)) => Some(ship),
            _ => None,
        }) else {
            panic!("{items:?}");
        };
        assert_eq!(ship.title, "Save carts");
        assert_eq!(ship.look, "S-1");
        assert_eq!(ship.lines, Some((79, 8)));
        // Shipping adds the goal's work to the branch it started from, through
        // one of its tasks.
        assert_eq!(ship.post, "/tickets/S-2/merge");
        assert_eq!(ship.notes(), ["checks failed: cargo test: 2 failed"]);
        // Once it is merged it is no longer ready.
        office.merged = vec!["S-2".into()];
        assert!(ready_goals(&office, "shop").is_empty());
    }

    #[test]
    fn a_goal_still_at_work_is_not_ready() {
        let mut tickets = goal_done();
        tickets.push(ticket("S-4", json!("working"), json!({"parent": "S-1"})));
        assert!(ready_goals(&office(tickets), "shop").is_empty());
    }

    #[test]
    fn a_question_waits_in_the_thread_with_its_plain_title() {
        let mut office = office(vec![ticket(
            "S-5",
            json!({"asking": {"ask": "a1"}}),
            json!({}),
        )]);
        office.asks = vec![
            serde_json::from_value(json!({
                "id": "a1", "ticket": "S-5", "hire": "dev", "kind": "permission",
                "tool": "Bash", "input": {"command": "./send.sh"}, "reasons": [],
                "at_ms": 9_500_000, "title": "Send the launch email?",
            }))
            .unwrap(),
        ];
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        let ask = items.iter().find(|i| i.id == "live-ask-a1").unwrap();
        assert_eq!(ask.text, "Dev is waiting for you.");
        let Some(Card::Ask(question)) = ask.cards.first() else {
            panic!()
        };
        assert_eq!(question.title, "Send the launch email?");
        assert_eq!(question.line, "Dev · Title S-5");
        assert!(question.plain);
        // Without a written question the ask says what it wants.
        office.asks[0].title = String::new();
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        let Some(Card::Ask(question)) = items
            .iter()
            .find(|i| i.id == "live-ask-a1")
            .unwrap()
            .cards
            .first()
        else {
            panic!()
        };
        assert_eq!(question.title, "Run ./send.sh?");
    }

    /// An ask by `hire` about `id`, at `at`.
    fn ask_of(id: &str, hire: &str, at: u64) -> studiod::api::AskView {
        serde_json::from_value(json!({
            "id": id, "ticket": "S-5", "hire": hire, "kind": "permission",
            "tool": "Bash", "input": {"command": "./go.sh"}, "reasons": [], "at_ms": at,
        }))
        .unwrap()
    }

    #[test]
    fn cards_that_wait_together_stand_under_one_line() {
        let mut office = office(vec![ticket(
            "S-5",
            json!({"asking": {"ask": "a1"}}),
            json!({}),
        )]);
        office.asks = vec![ask_of("a1", "dev", 9_500_000)];
        // One thing waits: it has its own line.
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        let waiting: Vec<&Item> = items.iter().filter(|i| waits(i)).collect();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].text, "Dev is waiting for you.");
        // Cards of two kinds that come together: one line, all the cards.
        office.asks.push(ask_of("a2", "dev", 9_600_000));
        office.tickets.push(ticket(
            "S-9",
            json!("ready"),
            json!({"created_at": 9_300_000}),
        ));
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        let group = items
            .iter()
            .find(|i| i.id.starts_with("live-group-"))
            .unwrap();
        assert_eq!(group.text, "Three things wait for you:");
        assert_eq!(group.cards.len(), 3);
        assert!(items.iter().all(|i| !waits(i)));
        // Only work to ship says so.
        let mut ships = goal_done();
        ships.push(ticket("S-9", json!("ready"), json!({})));
        let items = thread(&self::office(ships), "shop", &convo(vec![]), &Sizes::new());
        let group = items
            .iter()
            .find(|i| i.id.starts_with("live-group-"))
            .unwrap();
        assert_eq!(group.text, "Two pieces of work are ready to ship:");
    }

    #[test]
    fn something_said_between_two_cards_keeps_them_apart() {
        let mut office = office(vec![ticket(
            "S-5",
            json!({"asking": {"ask": "a1"}}),
            json!({}),
        )]);
        office.asks = vec![
            ask_of("a1", "dev", 9_000_000),
            ask_of("a2", "dev", 9_200_000),
        ];
        let convo = convo(vec![message(1, 9_100_000, TalkFrom::Owner, "Wait.", None)]);
        let items = thread(&office, "shop", &convo, &Sizes::new());
        assert!(items.iter().all(|i| !i.id.starts_with("live-group-")));
        assert_eq!(items.iter().filter(|i| waits(i)).count(), 2);
    }

    #[test]
    fn what_the_studio_did_to_a_task_is_one_line() {
        let acted = TalkCard::Acted {
            task: "S-6".into(),
            title: "Design the product cards".into(),
            action: "sent back".into(),
            note: "Too many colours.".into(),
        };
        let convo = convo(vec![message(
            1,
            9_000_000,
            TalkFrom::Studio,
            "Sent \"Design the product cards\" back.",
            Some(acted.clone()),
        )]);
        let items = thread(&office(vec![]), "shop", &convo, &Sizes::new());
        let item = items.iter().find(|i| i.id == "m-1").unwrap();
        // The card says it, so the words above it do not say it again.
        assert_eq!(item.text, "");
        assert_eq!(item.cards.len(), 1);
        // The lead's own words about it stay.
        let said = convo_by_lead("Sent it back.", acted);
        let items = thread(&office(vec![]), "shop", &said, &Sizes::new());
        assert_eq!(
            items.iter().find(|i| i.id == "m-1").unwrap().text,
            "Sent it back."
        );
    }

    fn convo_by_lead(text: &str, card: TalkCard) -> ConversationView {
        convo(vec![message(
            1,
            9_000_000,
            TalkFrom::Lead {
                hire: "nora".into(),
            },
            text,
            Some(card),
        )])
    }

    #[test]
    fn counts_read_as_words_up_to_ten() {
        assert_eq!(number(2), "Two");
        assert_eq!(number(10), "Ten");
        assert_eq!(number(11), "11");
    }

    #[test]
    fn only_a_release_that_waits_is_a_card() {
        let release = |state: &str| {
            serde_json::from_value(json!({
                "project": "shop", "version": "1.2", "state": state,
                "steps": [], "started_at": 9_900_000,
            }))
            .unwrap()
        };
        let mut office = office(vec![]);
        office.projects[0].release = Some(release("checking"));
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        assert!(
            items
                .iter()
                .all(|i| !matches!(i.cards.first(), Some(Card::Release(_))))
        );
        office.projects[0].release = Some(release("waiting"));
        let items = thread(&office, "shop", &convo(vec![]), &Sizes::new());
        let item = items
            .iter()
            .find(|i| matches!(i.cards.first(), Some(Card::Release(_))))
            .unwrap();
        assert_eq!(item.text, "v1.2 is ready to publish.");
    }

    #[test]
    fn a_plan_card_lists_its_tasks_with_who_and_where_they_stand() {
        let mut tickets = vec![ticket(
            "S-1",
            json!("landed"),
            json!({"kind": "plan", "goal": "Checkout"}),
        )];
        tickets.push(ticket(
            "S-2",
            json!("working"),
            json!({"parent": "S-1", "title": "Add a cart drawer"}),
        ));
        let office = office(tickets);
        let plan = TalkCard::Plan {
            goal: "S-1".into(),
            title: "Checkout flow".into(),
            version: Some("0.5".into()),
            tasks: vec![
                PlanTask {
                    title: "Add a cart drawer".into(),
                    hire: Some("dev".into()),
                },
                PlanTask {
                    title: "Design the order confirmation".into(),
                    hire: None,
                },
            ],
        };
        let convo = convo(vec![message(
            1,
            9_000_000,
            TalkFrom::Lead {
                hire: "nora".into(),
            },
            "Checkout's next:",
            Some(plan),
        )]);
        let items = thread(&office, "shop", &convo, &Sizes::new());
        let Some(Card::Plan(plan)) = items.iter().find(|i| i.id == "m-1").unwrap().cards.first()
        else {
            panic!()
        };
        assert_eq!(plan.version.as_deref(), Some("0.5"));
        assert_eq!(plan.tasks[0].status, "working");
        assert_eq!(
            plan.tasks[0].who.as_ref().map(|p| p.name.as_str()),
            Some("Dev")
        );
        assert_eq!(plan.tasks[1].status, "");
    }

    #[test]
    fn a_setup_card_is_left_out_when_its_setup_is_gone() {
        let card = TalkCard::Setup {
            project: "shop".into(),
        };
        let convo = convo(vec![message(
            1,
            9_000_000,
            TalkFrom::Studio,
            "Read it.",
            Some(card),
        )]);
        let mut office = office(vec![]);
        let items = thread(&office, "shop", &convo, &Sizes::new());
        assert!(
            items
                .iter()
                .find(|i| i.id == "m-1")
                .unwrap()
                .cards
                .is_empty()
        );
        office.setups.insert("shop".into(), SetupView::default());
        let items = thread(&office, "shop", &convo, &Sizes::new());
        assert!(matches!(
            items.iter().find(|i| i.id == "m-1").unwrap().cards.first(),
            Some(Card::Setup { .. })
        ));
    }

    #[test]
    fn the_days_numbers_count_what_landed_what_works_and_what_went_back() {
        let mut office = office(vec![
            ticket("S-1", json!("landed"), json!({})),
            ticket("S-2", json!("working"), json!({})),
            ticket(
                "S-3",
                json!("working"),
                json!({"feedback": "Fewer colours."}),
            ),
            ticket("S-4", json!({"asking": {"ask": "a1"}}), json!({})),
        ]);
        office.asks = vec![
            serde_json::from_value(json!({
                "id": "a1", "ticket": "S-4", "hire": "dev", "kind": "permission",
                "tool": "Bash", "input": {}, "reasons": [],
            }))
            .unwrap(),
        ];
        office.recent = vec![crate::client::Happened {
            ticket_id: "S-1".into(),
            at_ms: 9_000_000,
            event: TicketEvent::Landed,
        }];
        let stats = stats(&office, "shop");
        assert_eq!(
            (stats.done, stats.working, stats.sent_back, stats.for_you),
            (1, 2, 1, 1)
        );
    }
}
