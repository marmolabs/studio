use futures_util::Stream;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use studio_core::ticket::{Ticket, TicketKind};
use studio_git::Repo;
use studiod::api::Request;
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{
        content::{
            Form,
            sse::{Event, Sse},
        },
        page,
        request::uri,
        route,
    },
    view::{View, component, view},
};

use super::ticket::merged_ids;
use super::timeline::relative;
use crate::{AppConfig, client, live, ui};

#[page("/goals")]
async fn goals_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let status: client::Status = client::call(&config.socket, Request::Status).await?;
    let goals = load(&config.socket).await?;
    let open = open_goal(cx, &goals);
    // Only a real plan id gets this far, and it is encoded all the same:
    // nothing from the request may reach the expression unchecked.
    let open_query: String = url::form_urlencoded::byte_serialize(open.as_bytes()).collect();
    let stream = format!("@get('/goals/stream?open={open_query}')");
    Ok(view! {
        ui::top_actions(
            <button type="button" class=(ui::PRIMARY) popovertarget="new-goal">
                ui::icon(name: "plus")
                <span class=(ui::LABEL)>"New goal"</span>
            </button>
        )
        new_goal(projects: status.projects)
        <div data-init=(stream)>
            goal_list(list: goals, open: open)
        </div>
    })
}

#[route(GET "/goals/stream")]
async fn goals_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    Ok(live::watch(move || {
        let socket = config.socket.clone();
        let cx = cx.clone();
        async move {
            let goals = load(&socket).await?;
            let open = open_goal(&cx, &goals);
            ui::html(&cx, view! { cx => goal_list(list: goals, open: open) }).await
        }
    }))
}

/// The goal shown open: `?open=ID` when it names a goal, none for an empty
/// `?open=`, else the newest goal still under way.
fn open_goal(cx: &Cx, goals: &[Goal]) -> String {
    let query = uri(cx).query().unwrap_or_default();
    let asked = url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "open")
        .map(|(_, id)| id.into_owned());
    pick_open(asked.as_deref(), goals)
}

fn pick_open(asked: Option<&str>, goals: &[Goal]) -> String {
    match asked {
        Some("") => String::new(),
        Some(id) if goals.iter().any(|g| g.plan.id.to_string() == id) => id.to_string(),
        _ => goals
            .iter()
            .find(|g| !matches!(g.status(), "done" | "merged" | "failed" | "cancelled"))
            .map(|g| g.plan.id.to_string())
            .unwrap_or_default(),
    }
}

#[derive(Deserialize)]
struct NewGoal {
    project: String,
    text: String,
    /// The brief for the producer.
    #[serde(default)]
    detail: String,
    #[serde(default)]
    hire: String,
}

#[route(POST "/goals")]
async fn create_goal(cx: &Cx, Form(form): Form<NewGoal>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::CreateGoal {
        project: form.project,
        text: form.text,
        hire: Some(form.hire).filter(|h| !h.is_empty()),
        detail: form.detail.replace("\r\n", "\n"),
    };
    match client::call::<Ticket>(&config.socket, request).await {
        Ok(ticket) => ui::notice(cx, format!("Started {}.", ticket.id), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// A plan ticket and the tickets it filed.
#[derive(Clone, Debug)]
struct Goal {
    plan: Ticket,
    filed: Vec<Ticket>,
    /// What the plan and its tickets have cost so far; `None` when studiod
    /// can't say.
    spent: Option<f64>,
    /// Each ticket's own spend, by id.
    spend: BTreeMap<String, f64>,
    /// Whether the goal branch is already merged into its base branch.
    merged: bool,
    /// When the plan or any of its tickets last changed, if known.
    updated: Option<u64>,
}

impl Goal {
    fn landed(&self) -> usize {
        self.filed
            .iter()
            .filter(|t| t.state.kind() == "landed")
            .count()
    }

    /// Where the goal stands, which the plan ticket's own state can't say:
    /// a landed plan only means the tickets were filed.
    fn status(&self) -> &'static str {
        ui::goal_state(&self.plan, &self.filed, self.merged)
    }

    /// A landed ticket to merge the goal with: the goal is done and not merged.
    fn merge_ticket(&self) -> Option<String> {
        if self.status() != "done" {
            return None;
        }
        self.filed
            .iter()
            .find(|t| t.state.kind() == "landed")
            .map(|t| t.id.to_string())
    }
}

/// The goals with their tickets, spend, merge state and last change.
async fn load(socket: &Path) -> topcoat::Result<Vec<Goal>> {
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await?;
    // Spend and times that can't be read are left out, not shown as zero.
    let spend: Option<BTreeMap<String, f64>> =
        client::call(socket, Request::TicketSpends).await.ok();
    let updated: BTreeMap<String, u64> = client::call(socket, Request::UpdatedAt)
        .await
        .unwrap_or_default();
    Ok(goals_of(
        tickets,
        spend.as_ref(),
        &updated,
        &merged_ids(socket).await,
    ))
}

/// Plan tickets, newest first, each with its filed tickets (cancelled ones
/// don't count toward progress, but what they cost does count).
fn goals_of(
    tickets: Vec<Ticket>,
    spend: Option<&BTreeMap<String, f64>>,
    updated: &BTreeMap<String, u64>,
    merged: &[String],
) -> Vec<Goal> {
    let cost = |t: &Ticket| {
        spend
            .and_then(|s| s.get(&t.id.to_string()))
            .copied()
            .unwrap_or(0.0)
    };
    let mut out: Vec<Goal> = tickets
        .iter()
        .filter(|t| t.kind == TicketKind::Plan)
        .map(|plan| {
            let family: Vec<&Ticket> = tickets
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&plan.id))
                .collect();
            let everyone = || std::iter::once(plan).chain(family.iter().copied());
            Goal {
                plan: plan.clone(),
                filed: family
                    .iter()
                    .filter(|t| t.state.kind() != "cancelled")
                    .map(|t| (*t).clone())
                    .collect(),
                spent: spend.map(|_| everyone().map(cost).sum()),
                spend: everyone().map(|t| (t.id.to_string(), cost(t))).collect(),
                merged: family
                    .iter()
                    .filter(|t| t.state.kind() == "landed")
                    .any(|t| merged.contains(&t.id.to_string())),
                updated: everyone()
                    .filter_map(|t| updated.get(&t.id.to_string()).copied())
                    .max(),
            }
        })
        .collect();
    out.reverse();
    out
}

/// The New goal dialog, opened from the Goals and Board top bars.
#[component]
pub async fn new_goal(projects: Vec<String>) -> topcoat::Result<impl View> {
    Ok(view! {
        ui::dialog(id: "new-goal", title: "New goal",
            <form data-on:submit="@post('/goals', {contentType: 'form'}); el.closest('[popover]').hidePopover()">
                <div class=(ui::DIALOG_BODY)>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Project"</span>
                        <select name="project" required="" class=(ui::INPUT)>
                            for p in projects { <option value=(p.clone())>(p)</option> }
                        </select>
                    </label>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Goal"</span>
                        <input name="text" required="" class=(ui::INPUT) placeholder="Boss fight at the end of wave 10">
                    </label>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Brief for the producer"</span>
                        <textarea name="detail" class=(ui::TEXTAREA)></textarea>
                        <span class=(ui::HINT)>"The producer splits it into tickets that land on one branch."</span>
                    </label>
                </div>
                <div class=(ui::DIALOG_FOOT)>
                    <span class="spacer"></span>
                    <button type="button" class=(ui::BUTTON) popovertarget="new-goal" popovertargetaction="hide">"Cancel"</button>
                    <button type="submit" class=(ui::PRIMARY)>"Start planning"</button>
                </div>
            </form>
        )
    })
}

/// Columns left out on a phone, whose goal line says the same in short.
const PHONE_HIDDEN: &str = "max-[760px]:hidden";
/// Columns left out below a wide screen, so the table fits a laptop.
const NARROW_HIDDEN: &str = "max-[1180px]:hidden";

#[component]
async fn goal_list(list: Vec<Goal>, open: String) -> topcoat::Result<impl View> {
    let now = ui::now_ms();
    Ok(view! {
        <div id="goal-list">
            if list.is_empty() {
                <p class=(ui::MUTED)>"No goals yet. A goal gets a producer to plan it and file the tickets."</p>
            } else {
                <div class=(ui::TABLE_BOX)>
                    <table class=(ui::TABLE)>
                        <thead>
                            <tr>
                                <th class=(ui::TH)>"Goal"</th>
                                <th class=(format!("{PHONE_HIDDEN} {}", ui::TH))>"Progress"</th>
                                <th class=(format!("text-right {PHONE_HIDDEN} {}", ui::TH))>"Spent"</th>
                                <th class=(ui::TH)>"State"</th>
                                <th class=(format!("text-right {NARROW_HIDDEN} {}", ui::TH))>"Updated"</th>
                                <th class=(ui::TH)><span class="sr-only">"Merge"</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            #[key(goal.plan.id.to_string())]
                            for goal in list {
                                goal_rows(open: goal.plan.id.to_string() == open, goal: goal, now: now)
                            }
                        </tbody>
                    </table>
                </div>
            }
        </div>
    })
}

/// A goal's row, then its tickets as nested rows shown while it is open.
#[component]
async fn goal_rows(goal: Goal, open: bool, now: u64) -> topcoat::Result<impl View> {
    let status = goal.status();
    let merge = goal.merge_ticket();
    let (landed, total) = (goal.landed(), goal.filed.len());
    let plan = goal.plan.clone();
    let id = plan.id.to_string();
    let name = plan.goal.lines().next().unwrap_or_default().to_string();
    let place = if goal.merged {
        format!("merged into {}", plan.base_branch)
    } else {
        Repo::goal_branch_name(&plan.goal)
    };
    let toggle = if open {
        "/goals?open=".to_string()
    } else {
        format!("/goals?open={id}")
    };
    let updated = goal.updated.map(|at| relative(now, at));
    // The progress and spend columns, in short for the goal's line on a phone.
    let short = match (total, goal.spent) {
        (0, _) => "No tickets yet".to_string(),
        (_, Some(spent)) => format!("{landed} of {total} · {}", ui::usd(spent)),
        (_, None) => format!("{landed} of {total}"),
    };
    let tickets: Vec<(Ticket, Option<f64>)> = std::iter::once(plan.clone())
        .chain(goal.filed.iter().cloned())
        .filter(|_| open)
        .map(|t| {
            let usd = goal
                .spend
                .get(&t.id.to_string())
                .copied()
                .filter(|u| *u > 0.0);
            (t, usd)
        })
        .collect();
    let merged = status == "merged";
    Ok(view! {
        <tr class=(ui::TR)>
            // The goal takes the room the other columns leave, and no less
            // than its minimum; long names end in an ellipsis.
            <td class=(format!("w-full min-w-[220px] max-w-0 max-[760px]:min-w-0 {}", ui::TD))>
                <a href=(toggle) class="flex min-w-0 items-center gap-2.5" aria-expanded=(if open { "true" } else { "false" })>
                    <span class="flex text-fg-3">
                        ui::icon(name: if open { "chevron-down" } else { "chevron-right" }, small: true)
                    </span>
                    <span class="flex min-w-0 flex-col gap-0.5">
                        <b class=(if merged { "truncate font-medium text-fg-2" } else { "truncate font-medium" }) title=(plan.goal.clone())>(name)</b>
                        <span class="truncate text-[12px] text-fg-3">
                            (plan.project.to_string()) " · "
                            <span class=(PHONE_HIDDEN)>(place)</span>
                            <span class="hidden max-[760px]:inline">(short)</span>
                        </span>
                    </span>
                </a>
            </td>
            <td class=(format!("{PHONE_HIDDEN} {}", ui::TD))>
                if total == 0 {
                    <span class=(ui::MUTED)>"No tickets yet"</span>
                } else {
                    <span class="flex items-center gap-2">
                        <span class="w-24">ui::meter(value: landed as f64, max: total as f64, tone: if merged { "faint" } else { "ok" })</span>
                        <span class="text-fg-3 tabular-nums">(landed) " of " (total)</span>
                    </span>
                }
            </td>
            <td class=(format!("text-right tabular-nums {PHONE_HIDDEN} {}", ui::TD))>
                if let Some(spent) = goal.spent { (ui::usd(spent)) }
            </td>
            <td class=(ui::TD)>
                if merged {
                    // Merged is history, so it reads quietly.
                    <span class="[&_.st]:text-fg-3">ui::pill(tone: "neutral", label: "Merged", state: "merged")</span>
                } else {
                    ui::state_pill(state: status)
                }
            </td>
            <td class=(format!("text-right text-fg-3 {NARROW_HIDDEN} {}", ui::TD))>
                if let Some(updated) = updated { (updated) }
            </td>
            <td class=(format!("text-right {}", ui::TD))>
                if let Some(ticket) = merge {
                        <button
                            type="button"
                            class=(ui::BUTTON_SM)
                            aria-label="Merge"
                            title=(format!("Merge into {} in your checkout; nothing is pushed", plan.base_branch))
                            data-on:click=(format!("@post('/tickets/{ticket}/merge')"))
                        >
                            ui::icon(name: "git-merge", small: true)
                            <span class=(ui::LABEL)>"Merge"</span>
                        </button>
                }
            </td>
        </tr>
        for (t, usd) in tickets {
            <tr>
                <td colspan="6" class="border-t border-line bg-bg-2 py-2 pl-[42px] pr-4 text-[13px]">
                    <span class="flex min-w-0 items-center gap-2">
                        ui::state_icon(state: ui::shown(&t.state), small: true)
                        <span class="shrink-0 font-mono text-[12.5px] text-fg-3">(t.id.to_string())</span>
                        <a href=(format!("/tickets/{}", t.id)) class="truncate decoration-line-3 underline-offset-3 hover:underline">(t.title.clone())</a>
                        <span class="shrink-0 text-fg-3">
                            "· " (ui::look(ui::shown(&t.state)).label)
                            if let Some(usd) = usd { " · " (ui::usd(usd)) }
                        </span>
                    </span>
                </td>
            </tr>
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use topcoat::context::Cx;

    fn ticket(id: &str, state: &str, parent: Option<&str>) -> Ticket {
        let state = match state {
            "asking" => serde_json::json!({"asking": {"ask": "ASK-1"}}),
            "failed" => serde_json::json!({"failed": {"class": "checks", "detail": ""}}),
            other => serde_json::json!(other),
        };
        serde_json::from_value(serde_json::json!({
            "id": id, "project": "game", "title": format!("Title {id}"), "goal": "Polish the menus",
            "hire": "dev", "state": state, "attempts": 0, "base_branch": "master", "created_at": 0,
            "kind": if parent.is_some() { "work" } else { "plan" }, "parent": parent,
        }))
        .unwrap()
    }

    fn goal(plan: &str, filed: &[&str], merged: bool) -> Goal {
        let mut tickets = vec![ticket("G-1", plan, None)];
        for (i, state) in filed.iter().enumerate() {
            tickets.push(ticket(&format!("G-{}", i + 2), state, Some("G-1")));
        }
        let merged = if merged {
            vec!["G-2".to_string()]
        } else {
            Vec::new()
        };
        let updated = BTreeMap::from([("G-1".to_string(), 5), ("G-2".to_string(), 9)]);
        goals_of(tickets, Some(&BTreeMap::new()), &updated, &merged).remove(0)
    }

    #[test]
    fn a_goal_reads_from_its_tickets() {
        assert_eq!(goal("queued", &[], false).status(), "queued");
        assert_eq!(goal("working", &[], false).status(), "planning");
        assert_eq!(goal("working", &["landed"], false).status(), "in progress");
        assert_eq!(
            goal("landed", &["landed", "working"], false).status(),
            "in progress"
        );
        assert_eq!(
            goal("landed", &["landed", "queued"], false).status(),
            "queued"
        );
        assert_eq!(
            goal("landed", &["ready", "asking"], false).status(),
            "needs attention"
        );
        assert_eq!(
            goal("landed", &["landed", "cancelled"], false).status(),
            "done"
        );
        assert_eq!(goal("landed", &["landed"], true).status(), "merged");
        assert_eq!(goal("failed", &["landed"], false).status(), "failed");
        // A plan that filed nothing has nothing done.
        assert_eq!(goal("landed", &[], false).status(), "needs attention");
    }

    #[test]
    fn only_a_done_unmerged_goal_can_merge() {
        assert_eq!(
            goal("landed", &["landed"], false).merge_ticket().as_deref(),
            Some("G-2")
        );
        assert_eq!(goal("landed", &["landed"], true).merge_ticket(), None);
        assert_eq!(
            goal("landed", &["landed", "ready"], false).merge_ticket(),
            None
        );
    }

    #[test]
    fn only_a_real_goal_opens() {
        let goals = [goal("working", &[], false)];
        assert_eq!(pick_open(Some("G-1"), &goals), "G-1");
        assert_eq!(pick_open(Some(""), &goals), "");
        assert_eq!(pick_open(None, &goals), "G-1");
        assert_eq!(pick_open(Some("x');alert(1);('"), &goals), "G-1");
    }

    #[test]
    fn a_goal_was_updated_when_its_last_ticket_changed() {
        assert_eq!(goal("landed", &["landed"], false).updated, Some(9));
        assert_eq!(goal("landed", &[], false).updated, Some(5));
    }

    #[tokio::test]
    async fn rows_offer_merge_and_nest_their_tickets() {
        let cx = Cx::default();
        let done = goal("landed", &["landed"], false);
        let html = ui::html(
            &cx,
            view! { cx => goal_rows(goal: done, open: false, now: 60_000) },
        )
        .await
        .unwrap();
        assert!(html.contains("@post('/tickets/G-2/merge')"), "{html}");
        assert!(html.contains("studio/goal/polish-the-menus"), "{html}");
        assert!(html.contains("1 of 1") && html.contains("Done"), "{html}");
        // Its tickets show only while it is open.
        assert!(html.contains(r#"href="/goals?open=G-1""#), "{html}");
        assert!(!html.contains(r#"href="/tickets/G-2""#), "{html}");

        let merged = goal("landed", &["landed"], true);
        let html = ui::html(
            &cx,
            view! { cx => goal_rows(goal: merged, open: true, now: 60_000) },
        )
        .await
        .unwrap();
        assert!(!html.contains("/merge"), "{html}");
        assert!(
            html.contains("merged into master") && html.contains("Merged"),
            "{html}"
        );
        // Open: the plan and its ticket nest under it, and the link closes it.
        assert!(html.contains(r#"href="/tickets/G-1""#), "{html}");
        assert!(html.contains(r#"href="/tickets/G-2""#), "{html}");
        assert!(html.contains(r#"href="/goals?open=""#), "{html}");
    }
}
