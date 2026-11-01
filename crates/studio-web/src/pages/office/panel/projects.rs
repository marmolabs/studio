//! The Projects tab: each project's goals and tasks, with History under them.

use std::collections::HashSet;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use topcoat::view::{View, component, view};

use super::{quiet, section, share};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::floor::{self};
use crate::pages::office::{Office, key, safe, select};
use crate::pages::timeline::relative;
use crate::ui;

/// A ticket, or a merged goal, as a task row.
#[derive(Clone, Debug, PartialEq)]
struct Task {
    /// What picking it selects, such as `tickets/SHOP-3`.
    key: String,
    id: String,
    title: String,
    /// The tick's modifier class.
    tick: &'static str,
    /// The state in plain words.
    word: &'static str,
    /// The word's colour.
    tone: &'static str,
    /// When it happened, under the title; History's rows have it.
    when: String,
}

impl Task {
    fn of(ticket: &Ticket) -> Task {
        let words = facts::state_words(&ticket.state);
        Task {
            key: key("tickets", &ticket.id.to_string()),
            id: ticket.id.to_string(),
            title: ticket.title.clone(),
            tick: words.tick,
            word: words.word,
            tone: words.tone,
            when: String::new(),
        }
    }
}

/// An open goal in a project.
#[derive(Clone, Debug, PartialEq)]
struct GoalRow {
    key: String,
    name: String,
    landed: usize,
    total: usize,
    done: bool,
    /// The merge request and the branch it merges into, for a done goal.
    merge: Option<(String, String)>,
    tasks: Vec<Task>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProjectRow {
    name: String,
    tint: usize,
    needs: usize,
    working: usize,
    goals: Vec<GoalRow>,
    /// Tickets without a goal that are up next, in progress or finished
    /// lately.
    loose: Vec<Task>,
}

/// Every project, in the order of the status.
fn project_rows(office: &Office) -> Vec<ProjectRow> {
    let goals = floor::goals(office);
    let shipped = floor::shipped(office);
    let needs = facts::needs_you(office);
    office
        .status
        .projects
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let own: Vec<&Ticket> = office
                .tickets
                .iter()
                .filter(|t| t.project.to_string() == *name)
                .collect();
            let working = facts::working_in(office, name);
            let rows = goals
                .iter()
                .filter(|g| g.plan.project.to_string() == *name)
                .map(|g| {
                    let done = g.state() == "done";
                    let merge = done
                        .then(|| g.filed.iter().find(|t| t.state.kind() == "landed"))
                        .flatten()
                        .map(|t| {
                            (
                                format!("@post('/tickets/{}/merge')", t.id),
                                t.base_branch.clone(),
                            )
                        });
                    GoalRow {
                        key: key("goals", &g.plan.id.to_string()),
                        name: g.name(),
                        landed: g.landed(),
                        total: g.filed.len(),
                        done,
                        merge,
                        tasks: g.filed.iter().map(Task::of).collect(),
                    }
                })
                .collect();
            let loose = own
                .iter()
                .filter(|t| t.kind == TicketKind::Work && t.parent.is_none())
                .filter(|t| {
                    let kind = t.state.kind();
                    !matches!(kind, "landed" | "failed" | "cancelled")
                        || shipped.iter().any(|(s, _)| s.id == t.id)
                })
                .map(|t| Task::of(t))
                .collect();
            ProjectRow {
                name: name.clone(),
                tint: facts::wrap(i),
                needs: needs.in_project(office, name),
                working,
                goals: rows,
                loose,
            }
        })
        .collect()
}

/// How many rows of History the Projects tab starts with, and how many more
/// "Show more" adds.
pub const HISTORY_STEP: usize = 20;
/// The most rows of History a request may ask for.
const HISTORY_MOST: usize = 2000;

/// The rows of History asked for: at least a step, and no more than the most.
pub fn history_rows(asked: usize) -> usize {
    asked.clamp(HISTORY_STEP, HISTORY_MOST)
}

/// What the Projects tab's other rows leave out, newest first: merged goals,
/// and tickets that finished more than a day ago. The office only has events
/// for the last day, so when an older ticket finished is not known: it is
/// placed by when it was filed.
fn history(office: &Office) -> Vec<Task> {
    let lately: HashSet<String> = floor::finished(&office.recent, &office.tickets)
        .into_iter()
        .map(|(t, _)| t.id.to_string())
        .collect();
    let goals = floor::all_goals(office);
    // Tickets the rows above already show: those filed by an open goal.
    let shown: HashSet<String> = goals
        .iter()
        .filter(|g| g.state() != "merged")
        .flat_map(|g| g.filed.iter().map(|t| t.id.to_string()))
        .collect();
    // The plans of merged goals, whose rows stand for their tickets.
    let merged: HashSet<String> = goals
        .iter()
        .filter(|g| g.state() == "merged")
        .map(|g| g.plan.id.to_string())
        .collect();
    let mut rows: Vec<(u64, Task)> = Vec::new();
    for goal in goals.iter().filter(|g| g.state() == "merged") {
        let landed = goal
            .filed
            .iter()
            .filter_map(|t| office.last(&t.id.to_string(), |e| matches!(e, TicketEvent::Landed)))
            .max();
        let id = goal.plan.id.to_string();
        let (at, when) = match landed {
            Some(at) => (at, format!("landed {}", relative(office.now, at))),
            None => (
                goal.plan.created_at,
                format!("filed {}", relative(office.now, goal.plan.created_at)),
            ),
        };
        rows.push((
            at,
            Task {
                key: key("goals", &id),
                id,
                title: goal.name(),
                tick: "done",
                word: "merged",
                tone: "text-fg-3",
                when,
            },
        ));
    }
    for ticket in &office.tickets {
        let id = ticket.id.to_string();
        let ended = matches!(ticket.state.kind(), "landed" | "failed" | "cancelled");
        // A landed plan only filed its goal's tickets.
        let filed_only = ticket.kind == TicketKind::Plan && ticket.state.kind() == "landed";
        let of_merged = ticket
            .parent
            .as_ref()
            .is_some_and(|p| merged.contains(&p.to_string()));
        if !ended || filed_only || of_merged || lately.contains(&id) || shown.contains(&id) {
            continue;
        }
        let mut task = Task::of(ticket);
        task.when = format!("filed {}", relative(office.now, ticket.created_at));
        rows.push((ticket.created_at, task));
    }
    rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    rows.into_iter().map(|(_, task)| task).collect()
}

#[component]
pub(super) async fn projects_pane(office: Office, history: usize) -> topcoat::Result<impl View> {
    let rows = project_rows(&office);
    let asked = history_rows(history);
    let mut older = self::history(&office);
    let count = older.len();
    older.truncate(asked);
    let more = count > asked;
    let show_more = format!("$history = {}; @get('/office/panel')", asked + HISTORY_STEP);
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "No projects yet.")
        }
        for row in rows {
            project_block(row: row)
        }
        if count > 0 {
            section(label: "History", note: format!("{count} · newest first"),
                <ul class="tasks m-0 list-none p-0">
                    for task in older {
                        task_row(task: task)
                    }
                </ul>
                if more {
                    <button type="button" class=(format!("mt-2 {}", ui::BUTTON_SM)) data-on:click=(show_more)>
                        "Show more"
                    </button>
                }
            )
        }
    })
}

#[component]
pub(super) async fn project_block(row: ProjectRow) -> topcoat::Result<impl View> {
    let id = format!("proj-{}", safe(&row.name));
    let nothing = row.goals.is_empty() && row.loose.is_empty();
    Ok(view! {
        <details id=(id) class="proj" data-preserve-attr="open" open="">
            <summary>
                <i class=(dot_class(row.tint))></i>
                <span class="nm"><b>(row.name.clone())</b></span>
                <span class="pcounts">
                    if row.needs > 0 {
                        <span class="flag needs" title="Need you">(row.needs)</span>
                    }
                    if row.working > 0 {
                        <span class="flag run" title="Working">(row.working)</span>
                    }
                </span>
            </summary>
            <div class="proj-body">
                if nothing {
                    quiet(text: "Nothing in progress.")
                }
                for goal in row.goals {
                    goal_block(goal: goal)
                }
                if !row.loose.is_empty() {
                    <div class="goal loose">
                        <div class="g-h"><b>"Without a goal"</b></div>
                        <ul class="tasks m-0 list-none p-0">
                            for task in row.loose {
                                task_row(task: task)
                            }
                        </ul>
                    </div>
                }
            </div>
        </details>
    })
}

#[component]
async fn goal_block(goal: GoalRow) -> topcoat::Result<impl View> {
    let pick = select(&goal.key);
    let width = share(goal.landed as f64, goal.total as f64);
    let empty = goal.tasks.is_empty();
    Ok(view! {
        <div class="goal">
            <div class="g-h">
                <b><button type="button" class="max-w-full cursor-pointer truncate border-0 bg-transparent p-0 text-left font-[inherit] text-[13px] font-semibold text-fg hover:underline" data-on:click=(pick)>(goal.name.clone())</button></b>
                <span>(goal.landed) " of " (goal.total)</span>
            </div>
            <div class=(if goal.done { "xp done" } else { "xp" })><i style=(format!("width: {width:.0}%"))></i></div>
            if goal.done {
                <div class="btns">
                    <span class="chip ok">"Done"</span>
                    if let Some((post, base)) = goal.merge {
                        <button type="button" class=(format!("{} push", ui::BUTTON_SM)) data-on:click=(post)>
                            ui::icon(name: "git-merge", small: true)
                            "Merge into " (base)
                        </button>
                    }
                </div>
            } else if empty {
                <p class="why">"The producer has not filed tickets yet."</p>
            } else {
                <ul class="tasks m-0 list-none p-0">
                    for task in goal.tasks {
                        task_row(task: task)
                    }
                </ul>
            }
        </div>
    })
}

#[component]
async fn task_row(task: Task) -> topcoat::Result<impl View> {
    let pick = select(&task.key);
    Ok(view! {
        <li>
            <button type="button" class="task w-full border-0 bg-transparent text-left" data-on:click=(pick)>
                <span class=(format!("tick {}", task.tick))></span>
                <span class="mono font-mono">(task.id.clone())</span>
                <span class="t">
                    (task.title.clone())
                    if !task.when.is_empty() {
                        <small class="block text-[11px] text-fg-3">(task.when.clone())</small>
                    }
                </span>
                <span class=(format!("whitespace-nowrap text-[11.5px] {}", task.tone))>(task.word)</span>
            </button>
        </li>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::panel::fixtures::{HOUR, NOW, extra, html_of, office, ticket};

    use serde_json::{Value, json};

    #[tokio::test]
    async fn projects_list_goals_tasks_and_loose_tickets() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("pjd pj-0"), "{html}");
        assert!(html.contains("pjd pj-1"), "{html}");
        // The goal: one of three landed.
        assert!(html.contains("Checkout flow"), "{html}");
        assert!(html.contains("1 of 3"), "{html}");
        for word in [
            "landed",
            "working",
            "up next",
            "needs you",
            "failed",
            "ready to land",
        ] {
            assert!(html.contains(&format!(">{word}<")), "{word}: {html}");
        }
        assert!(
            html.contains("$sel = 'tickets/SHOP-3'; @get('/office/panel')"),
            "{html}"
        );
        // The held, ready, failed and queued ones have no goal, and are
        // listed apart, the queued one as up next.
        assert!(html.contains("Without a goal"), "{html}");
        assert!(html.contains("tickets/API-3'"), "{html}");
    }

    #[tokio::test]
    async fn a_done_goal_waits_for_its_merge_in_two_places() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets.remove(3);
        let html = html_of(office, extra(), "").await;
        assert!(html.contains("Goals done"), "{html}");
        assert!(html.contains("Merge into"), "{html}");
        // Once under Needs you and once in its project.
        assert_eq!(
            html.matches("@post('/tickets/SHOP-2/merge')").count(),
            2,
            "{html}"
        );
    }

    #[test]
    fn a_done_goal_waits_for_its_merge() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets.remove(3);
        let shop = &project_rows(&office)[0];
        assert!(shop.goals[0].done);
        assert_eq!(
            shop.goals[0].merge,
            Some(("@post('/tickets/SHOP-2/merge')".into(), "main".into()))
        );
    }

    #[test]
    fn history_holds_merged_goals_and_tickets_finished_before_the_window() {
        let mut office = office();
        let old = |hours: u64| json!({"created_at": NOW - hours * HOUR});
        let with = |hours: u64, more: Value| {
            let mut extra = old(hours);
            extra
                .as_object_mut()
                .unwrap()
                .extend(more.as_object().unwrap().clone());
            extra
        };
        office.tickets.extend([
            // A goal that merged, its last ticket landing three hours ago.
            ticket(
                "OLD-1",
                "shop",
                json!("landed"),
                with(90, json!({"kind": "plan", "goal": "Old goal"})),
            ),
            ticket(
                "OLD-2",
                "shop",
                json!("landed"),
                with(89, json!({"parent": "OLD-1"})),
            ),
            // Work that finished before the window, and a cancelled ticket of
            // an open goal, which the goal's row leaves out.
            ticket(
                "OLD-3",
                "shop",
                json!({"failed": {"class": "checks", "detail": "x"}}),
                old(50),
            ),
            ticket(
                "OLD-4",
                "shop",
                json!("cancelled"),
                with(60, json!({"parent": "SHOP-1"})),
            ),
        ]);
        office.merged = vec!["OLD-2".into()];
        office.recent.push(
            serde_json::from_value(json!({
                "ticket_id": "OLD-2", "at_ms": NOW - 3 * HOUR, "event": {"type": "landed"},
            }))
            .unwrap(),
        );

        let rows = history(&office);
        let seen: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|t| (t.key.as_str(), t.word, t.when.as_str()))
            .collect();
        // Newest first. Not here: the goal's own ticket, the landed plan of
        // an open goal, and what finished in the last day.
        assert_eq!(
            seen,
            [
                ("goals/OLD-1", "merged", "landed 3h ago"),
                ("tickets/OLD-3", "failed", "filed 2d ago"),
                ("tickets/OLD-4", "cancelled", "filed 2d ago"),
            ]
        );
        assert_eq!(rows[0].title, "Old goal");
    }

    #[test]
    fn history_asks_for_a_step_at_least() {
        assert_eq!(history_rows(0), HISTORY_STEP);
        assert_eq!(history_rows(40), 40);
        assert_eq!(history_rows(usize::MAX), HISTORY_MOST);
    }
}
