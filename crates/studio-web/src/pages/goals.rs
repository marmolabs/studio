use futures_util::Stream;
use serde::Deserialize;
use std::collections::BTreeMap;
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
        page, route,
    },
    view::{View, component, view},
};

use crate::{AppConfig, client, live, ui};

#[page("/goals")]
async fn goals_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    let status: client::Status = client::call(&config.socket, Request::Status).await?;
    let goals = goals_of(tickets, &spend_by_ticket(&config.socket).await);
    Ok(view! {
        <h1 class=(format!("mb-3 {}", ui::PAGE_TITLE))>"Goals"</h1>
        new_goal(projects: status.projects, hires: status.hires)
        <div data-init="@get('/goals/stream')">
            goal_list(list: goals)
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
            let tickets: Vec<Ticket> = client::call(&socket, Request::Tickets).await?;
            let goals = goals_of(tickets, &spend_by_ticket(&socket).await);
            ui::html(&cx, view! { cx => goal_list(list: goals) }).await
        }
    }))
}

#[derive(Deserialize)]
struct NewGoal {
    project: String,
    text: String,
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
        detail: String::new(),
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
    /// What the plan and its tickets have cost so far.
    spent: f64,
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
        let plan_open = !matches!(self.plan.state.kind(), "landed" | "failed" | "cancelled");
        let trouble = self
            .filed
            .iter()
            .any(|t| matches!(t.state.kind(), "failed" | "held" | "asking"));
        match self.plan.state.kind() {
            "failed" => "failed",
            "cancelled" => "cancelled",
            _ if trouble => "needs attention",
            _ if plan_open && self.filed.is_empty() => "planning",
            _ if !plan_open && self.landed() == self.filed.len() => "done",
            _ => "in progress",
        }
    }
}

/// Plan tickets, newest first, each with its filed tickets (cancelled ones
/// don't count toward progress, but what they cost does count).
fn goals_of(tickets: Vec<Ticket>, spend: &BTreeMap<String, f64>) -> Vec<Goal> {
    let cost = |t: &Ticket| spend.get(&t.id.to_string()).copied().unwrap_or(0.0);
    let mut out: Vec<Goal> = tickets
        .iter()
        .filter(|t| t.kind == TicketKind::Plan)
        .map(|plan| {
            let family = tickets
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&plan.id));
            Goal {
                plan: plan.clone(),
                filed: family
                    .clone()
                    .filter(|t| t.state.kind() != "cancelled")
                    .cloned()
                    .collect(),
                spent: cost(plan) + family.map(cost).sum::<f64>(),
            }
        })
        .collect();
    out.reverse();
    out
}

/// What each ticket has cost; spend that can't be read counts as 0.
async fn spend_by_ticket(socket: &std::path::Path) -> BTreeMap<String, f64> {
    client::call(socket, Request::TicketSpends)
        .await
        .unwrap_or_default()
}

#[component]
async fn new_goal(projects: Vec<String>, hires: Vec<String>) -> topcoat::Result<impl View> {
    Ok(view! {
        <details class=(format!("group mb-4 {}", ui::CARD))>
            <summary class=(format!("cursor-pointer select-none px-3 py-2 font-medium hover:text-zinc-900 dark:hover:text-zinc-100 {}", ui::SECONDARY))>
                "New goal"
            </summary>
            <form
                data-on:submit="@post('/goals', {contentType: 'form'})"
                class="grid grid-cols-1 gap-2 border-t border-zinc-300 p-3 sm:grid-cols-[10rem_10rem_minmax(0,1fr)] dark:border-zinc-700"
            >
                <select name="project" required="" class=(ui::INPUT)>
                    for p in projects { <option value=(p.clone())>(p)</option> }
                </select>
                <select name="hire" class=(ui::INPUT)>
                    <option value="">"Producer (default)"</option>
                    for h in hires { <option value=(h.clone())>(h)</option> }
                </select>
                <textarea
                    name="text"
                    required=""
                    rows="2"
                    placeholder="What should be achieved. A producer files the tickets."
                    class=(ui::INPUT)
                ></textarea>
                <div class="sm:col-span-3">
                    <button type="submit" class=(ui::PRIMARY)>"Start goal"</button>
                </div>
            </form>
        </details>
    })
}

#[component]
async fn goal_list(list: Vec<Goal>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="goal-list" class="space-y-2">
            if list.is_empty() {
                <p class=(format!("px-1 py-2 {}", ui::MUTED))>"No goals yet."</p>
            }
            #[key(goal.plan.id.to_string())]
            for goal in list {
                goal_card(goal: goal)
            }
        </div>
    })
}

#[component]
async fn goal_card(goal: Goal) -> topcoat::Result<impl View> {
    let landed = goal.landed();
    let total = goal.filed.len();
    let status = goal.status();
    let share = if total == 0 {
        0.0
    } else {
        landed as f64 / total as f64
    };
    // One merge per goal: any landed ticket on it can start it.
    let merge = goal
        .filed
        .iter()
        .find(|t| t.state.kind() == "landed")
        .map(|t| (t.id.to_string(), t.base_branch.clone()));
    let branch = Repo::goal_branch_name(&goal.plan.goal);
    let spent = ui::usd(goal.spent);
    let plan = goal.plan;
    Ok(view! {
        <section class=(format!("p-3 {}", ui::CARD))>
            <div class="flex flex-wrap items-start gap-x-2 gap-y-1">
                <h2 title=(plan.goal.clone()) class="line-clamp-2 min-w-0 flex-1 basis-48 font-medium [overflow-wrap:anywhere]">(plan.goal.clone())</h2>
                ui::state_pill(state: status)
            </div>
            <div class="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[12px]">
                ui::badge(text: plan.project.to_string())
                <code class="max-w-full truncate rounded bg-zinc-100 px-1.5 py-px font-mono text-[11px] dark:bg-zinc-800">(branch)</code>
                <span class=(format!("tabular-nums {}", ui::SECONDARY))>
                    if total == 0 {
                        "No tickets filed yet"
                    } else {
                        (landed) " of " (total) " landed"
                    }
                </span>
                <span class=(format!("tabular-nums {}", ui::SECONDARY))>(spent) " spent"</span>
                <a href=(format!("/tickets/{}", plan.id)) class=(format!("ml-auto underline hover:text-zinc-900 dark:hover:text-zinc-100 {}", ui::SECONDARY))>
                    "Plan " (plan.id.to_string())
                </a>
            </div>
            if total > 0 {
                <div class="mt-2 h-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                    <div class="h-full rounded-full bg-emerald-500" style=(format!("width: {:.1}%", share * 100.0))></div>
                </div>
            }
            if let Some((id, base)) = merge {
                super::ticket::merge_button(ticket: id, base: base)
            }
            if total > 0 {
                <ul class="mt-2 divide-y divide-zinc-100 border-t border-zinc-100 dark:divide-zinc-800 dark:border-zinc-800">
                    #[key(t.id.to_string())]
                    for t in goal.filed {
                        <li class="flex items-center gap-2 py-1">
                            <a href=(format!("/tickets/{}", t.id)) class=(format!("shrink-0 font-mono text-[11px] hover:underline {}", ui::MUTED))>
                                (t.id.to_string())
                            </a>
                            <span class="min-w-0 flex-1 truncate">(t.title)</span>
                            ui::state_pill(state: t.state.kind())
                        </li>
                    }
                </ul>
            }
        </section>
    })
}
