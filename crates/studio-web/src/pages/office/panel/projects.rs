//! The Projects tab: a card for each project, with History under them.

use std::collections::{HashMap, HashSet};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use topcoat::view::{View, component, view};

use super::standing::{self, Standing};
use super::{faces, phase_pill, pipeline, quiet, section};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::floor::{self};
use crate::pages::office::{Office, key, select};
use crate::pages::timeline::relative;
use crate::ui;

/// A task, or a finished milestone, as a row.
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

/// How many rows of History the Projects tab starts with, and how many more
/// "Show more" adds.
pub const HISTORY_STEP: usize = 20;
/// The most rows of History a request may ask for.
const HISTORY_MOST: usize = 2000;

/// The rows of History asked for: at least a step, and no more than the most.
pub fn history_rows(asked: usize) -> usize {
    asked.clamp(HISTORY_STEP, HISTORY_MOST)
}

/// What the project cards leave out, newest first: finished milestones, and
/// finished work that no open milestone holds. The office only has events
/// for the last day, so when an older task finished is not known: it is
/// placed by when it was filed.
fn history(office: &Office) -> Vec<Task> {
    let ended_at: HashMap<String, u64> = floor::finished(&office.recent, &office.tickets)
        .into_iter()
        .map(|(t, at)| (t.id.to_string(), at))
        .collect();
    let goals = floor::all_goals(office);
    // Tasks a project's page already shows: those its open milestone filed.
    let shown: HashSet<String> = goals
        .iter()
        .filter(|g| g.state() != "merged")
        .flat_map(|g| g.filed.iter().map(|t| t.id.to_string()))
        .collect();
    // The plans of shipped milestones, whose rows stand for their tasks.
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
            Some(at) => (at, format!("shipped {}", relative(office.now, at))),
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
                word: "shipped",
                tone: "text-fg-3",
                when,
            },
        ));
    }
    for ticket in &office.tickets {
        let id = ticket.id.to_string();
        let ended = matches!(ticket.state.kind(), "landed" | "failed" | "cancelled");
        // A landed plan only filed its milestone's tasks.
        let filed_only = ticket.kind == TicketKind::Plan && ticket.state.kind() == "landed";
        let of_merged = ticket
            .parent
            .as_ref()
            .is_some_and(|p| merged.contains(&p.to_string()));
        if !ended || filed_only || of_merged || shown.contains(&id) {
            continue;
        }
        let mut task = Task::of(ticket);
        let (at, when) = match ended_at.get(&id) {
            Some(at) => (*at, format!("{} {}", task.word, relative(office.now, *at))),
            None => (
                ticket.created_at,
                format!("filed {}", relative(office.now, ticket.created_at)),
            ),
        };
        task.when = when;
        rows.push((at, task));
    }
    rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    rows.into_iter().map(|(_, task)| task).collect()
}

#[component]
pub(super) async fn projects_pane(office: Office, history: usize) -> topcoat::Result<impl View> {
    let cards = standing::standings(&office);
    let asked = history_rows(history);
    let mut older = self::history(&office);
    let count = older.len();
    older.truncate(asked);
    let more = count > asked;
    let show_more = format!("$history = {}; @get('/office/panel')", asked + HISTORY_STEP);
    let now = office.now;
    Ok(view! {
        if cards.is_empty() {
            quiet(text: "No projects yet.")
        }
        for card in cards {
            project_card(project: card, now: now)
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

/// A project as a card: its phase and pipeline, its milestone and how far it
/// is, the release line, and who leads and works on it. Picking it opens the
/// project's page.
#[component]
async fn project_card(project: Standing, now: u64) -> topcoat::Result<impl View> {
    let pick = select(&project.key());
    let (release, tone) = standing::release_line(&project, now);
    let release_class = match tone {
        "wait" => "font-semibold text-attn-fg",
        "bad" => "text-bad-fg",
        _ => "text-fg-2",
    };
    let milestone = project.milestone.clone();
    let lead = project.lead.clone().map(|l| l.name);
    let team = project.team.clone();
    let label = format!("Open {}", project.name);
    Ok(view! {
        <button type="button" class=(format!("pj-{} flex w-full min-w-0 cursor-pointer flex-col gap-2.5 rounded-xl border border-line border-l-4 border-l-[var(--pj)] bg-bg-3 py-3 pl-3.5 pr-3 text-left hover:border-line-2 hover:border-l-[var(--pj)] hover:bg-bg-4", project.tint)) data-on:click=(pick) aria-label=(label)>
            <span class="flex items-center gap-2">
                <i class=(dot_class(project.tint))></i>
                <b class="min-w-0 truncate text-[15px] font-semibold">(project.name.clone())</b>
                if project.needs > 0 {
                    <span class="flag needs" title="Needs you">(project.needs)</span>
                }
                <span class="ml-auto">phase_pill(phase: project.phase)</span>
            </span>
            pipeline(phase: project.phase)
            if let Some(mile) = milestone {
                <span class="flex flex-col gap-1.5 text-[13px] text-fg-2">
                    <span class="flex items-baseline justify-between gap-2">
                        <b class="min-w-0 truncate font-semibold text-fg" title=(mile.name())>(mile.short())</b>
                        <span class="whitespace-nowrap text-[12.5px]">(mile.done()) " of " (mile.total()) " tasks"</span>
                    </span>
                    ui::meter(value: mile.done() as f64, max: mile.total() as f64, tone: "run")
                </span>
            } else {
                <span class="text-[13px] text-fg-3">"No milestone open."</span>
            }
            <span class=(format!("flex items-center gap-1.5 text-[12.5px] {release_class}"))>(release)</span>
            <span class="flex items-center gap-2 text-[12.5px] text-fg-3">
                faces(people: team.clone())
                if let Some(lead) = lead {
                    <span class="text-fg-2">(lead) " leads"</span>
                } else {
                    <span>"No lead yet"</span>
                }
                if team.is_empty() {
                    <span>"· nobody at work"</span>
                }
            </span>
        </button>
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
    use crate::pages::office::panel::fixtures::{HOUR, NOW, extra, html_on, office, pane, ticket};
    use crate::probe::{find, text_of};

    use serde_json::{Value, json};

    async fn tab(office: Office) -> String {
        html_on(office, extra(), "", "projects").await
    }

    #[tokio::test]
    async fn each_project_is_a_card_with_its_phase_milestone_release_and_team() {
        let html = tab(office()).await;
        let shop = find(pane(&html, "projects"), "aria-label", "Open shop");
        assert!(
            shop.attr_has("data-on:click", "$sel = 'projects/shop'"),
            "{html}"
        );
        assert!(shop.attr_has("class", "pj-0"), "{html}");
        let text = shop.text();
        // Releasing, with the pipeline and the milestone's progress.
        assert!(text.contains("Releasing"), "{text}");
        for step in ["Planning", "Building", "Testing", "Live"] {
            assert!(text.contains(step), "{step}: {text}");
        }
        assert!(text.contains("v0.4 Checkout flow"), "{text}");
        assert!(text.contains("1 of 3 tasks"), "{text}");
        assert!(text.contains("v1.2 waits for your OK"), "{text}");
        assert!(text.contains("Nora leads"), "{text}");
        // The api project has no lead and nothing open but loose work.
        let api = find(pane(&html, "projects"), "aria-label", "Open api");
        assert!(api.attr_has("class", "pj-1"), "{html}");
        let text = api.text();
        assert!(text.contains("No milestone open."), "{text}");
        assert!(text.contains("Never released"), "{text}");
        assert!(text.contains("No lead yet"), "{text}");
    }

    #[tokio::test]
    async fn the_card_lights_the_step_the_project_is_at() {
        let lit = |html: &str, project: &str| {
            let card = find(
                pane(html, "projects"),
                "aria-label",
                &format!("Open {project}"),
            );
            let inner = card.inner_html().to_string();
            let steps = crate::probe::with(&inner, "aria-current", "step");
            steps.iter().map(|s| s.text()).collect::<Vec<_>>()
        };
        let html = tab(office()).await;
        assert_eq!(lit(&html, "shop"), ["Releasing"], "{html}");
        // Without the release, shop is still building.
        let mut building = office();
        building.projects[0].release = None;
        assert_eq!(lit(&tab(building).await, "shop"), ["Building"]);
        // A closed office pauses the work, but a release under way still
        // reads as releasing.
        let mut closed = office();
        closed.status.closed = true;
        assert_eq!(lit(&tab(closed.clone()).await, "shop"), ["Releasing"]);
        // Without one, the paused project lights no step.
        closed.projects[0].release = None;
        assert!(lit(&tab(closed).await, "shop").is_empty());
    }

    #[tokio::test]
    async fn history_comes_after_the_cards() {
        let html = tab(office()).await;
        let history = html.find("History").unwrap();
        let card = html.find("aria-label=\"Open api\"").unwrap();
        assert!(card < history, "{html}");
        // API-1 failed an hour ago, outside any milestone.
        let text = text_of(&html);
        assert!(text.contains("failed 1h ago"), "{text}");
        assert!(
            html.contains("$sel = 'tickets/API-1'; @get('/office/panel')"),
            "{html}"
        );
    }

    #[test]
    fn history_holds_shipped_milestones_and_work_outside_open_ones() {
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
            // A milestone that shipped, its last task landing three hours ago.
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
            // Work that finished before the window, and a cancelled task of
            // an open milestone, which its project's page leaves out.
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
        // Newest first. Not here: the milestone's own tasks and the landed
        // plan of an open one.
        assert_eq!(
            seen,
            [
                ("tickets/API-1", "failed", "failed 1h ago"),
                ("goals/OLD-1", "shipped", "shipped 3h ago"),
                ("tickets/OLD-3", "failed", "filed 2d ago"),
                ("tickets/OLD-4", "cancelled", "filed 2d ago"),
            ]
        );
        assert_eq!(rows[1].title, "Old goal");
    }

    #[test]
    fn history_asks_for_a_step_at_least() {
        assert_eq!(history_rows(0), HISTORY_STEP);
        assert_eq!(history_rows(40), 40);
        assert_eq!(history_rows(usize::MAX), HISTORY_MOST);
    }
}
