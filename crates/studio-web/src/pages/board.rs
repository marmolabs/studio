use futures_util::Stream;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{AskView, Estimate, Request};
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

use super::ask_card::ask_card;
use super::ticket::log_tail;
use crate::summary::latest_activity;
use crate::{AppConfig, client, live, ui};

/// A group of tickets on the board: its anchor id, title and the states it holds.
struct Lane {
    id: &'static str,
    title: &'static str,
    kinds: &'static [&'static str],
}

/// Lanes top to bottom. The first two are the owner's; empty ones are not drawn.
const LANES: [Lane; 5] = [
    Lane {
        id: "needs-you",
        title: "Needs you",
        kinds: &["asking", "held"],
    },
    Lane {
        id: "ready",
        title: "Ready to land",
        kinds: &["ready"],
    },
    Lane {
        id: "working",
        title: "Working",
        kinds: &["claimed", "working", "checking", "reviewing"],
    },
    Lane {
        id: "queued",
        title: "Queued",
        kinds: &["queued"],
    },
    Lane {
        id: "landed",
        title: "Landed",
        kinds: &["landed"],
    },
];
const LANDED_SHOWN: usize = 20;
const NOW_SHOWN: usize = 90;

/// What the columns show besides the tickets themselves.
struct Extras {
    etas: Vec<Estimate>,
    asks: Vec<AskView>,
    /// The latest activity of each running ticket, by id.
    now: HashMap<String, String>,
}

async fn extras(socket: &Path, data_dir: &Path, tickets: &[Ticket]) -> Extras {
    let asks = client::call(socket, Request::Asks)
        .await
        .unwrap_or_default();
    let mut now = HashMap::new();
    for ticket in tickets.iter().filter(|t| t.state.kind() == "working") {
        let id = ticket.id.to_string();
        let tail = log_tail(data_dir, &id).await;
        if let Some(line) = latest_activity(&tail, NOW_SHOWN) {
            now.insert(id, line);
        }
    }
    Extras {
        etas: client::estimates(socket).await,
        asks,
        now,
    }
}

#[page("/")]
async fn board(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    let status: client::Status = client::call(&config.socket, Request::Status).await?;
    let Extras { etas, asks, now } = extras(&config.socket, &config.data_dir, &tickets).await;
    let no_projects = status.projects.is_empty();
    Ok(view! {
        <div class="mb-3 flex items-center justify-between">
            <h1 class=(ui::PAGE_TITLE)>"Board"</h1>
        </div>
        if no_projects {
            <div class="mb-5 rounded-lg border border-dashed border-zinc-300 bg-white px-4 py-6 text-center dark:border-zinc-700 dark:bg-zinc-900">
                <p class="font-medium">"No projects yet"</p>
                <p class="mt-1 text-zinc-500 dark:text-zinc-400">
                    "Add a game project to start filing tickets:"
                </p>
                <p class="mt-2">
                    <code class="rounded-md bg-zinc-100 px-2 py-1 font-mono text-[12px] dark:bg-zinc-800">"studio project add <path>"</code>
                </p>
            </div>
        } else {
            new_ticket(projects: status.projects, hires: status.hires)
        }
        <div data-init="@get('/board/stream')">
            columns(tickets: tickets, etas: etas, asks: asks, now: now, spent: status.spent_24h, daily: status.daily_usd)
        </div>
    })
}

#[route(GET "/board/stream")]
async fn board_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    Ok(live::watch(move || {
        let config = config.clone();
        let cx = cx.clone();
        async move {
            let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
            let status: client::Status = client::call(&config.socket, Request::Status).await?;
            let Extras { etas, asks, now } =
                extras(&config.socket, &config.data_dir, &tickets).await;
            let (spent, daily) = (status.spent_24h, status.daily_usd);
            ui::html(
                &cx,
                view! { cx => columns(tickets: tickets, etas: etas, asks: asks, now: now, spent: spent, daily: daily) },
            )
            .await
        }
    }))
}

#[derive(Deserialize)]
struct NewTicket {
    project: String,
    /// Empty means the router picks.
    #[serde(default)]
    hire: String,
    title: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    detail: String,
}

#[route(POST "/tickets")]
async fn create_ticket(cx: &Cx, Form(form): Form<NewTicket>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::CreateTicket {
        project: form.project,
        title: form.title,
        goal: form.goal,
        hire: form.hire,
        base: None,
        detail: form.detail.replace("\r\n", "\n"),
    };
    match client::call::<Ticket>(&config.socket, request).await {
        Ok(ticket) => ui::notice(cx, format!("Queued {}.", ticket.id), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[component]
async fn new_ticket(projects: Vec<String>, hires: Vec<String>) -> topcoat::Result<impl View> {
    Ok(view! {
        <details class="group mb-4 rounded-lg border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
            <summary class="cursor-pointer select-none px-3 py-1.5 text-[13px] text-zinc-600 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100">
                "New ticket"
            </summary>
            <form
                data-on:submit="@post('/tickets', {contentType: 'form'})"
                class="grid grid-cols-1 gap-2 border-t border-zinc-300 p-3 sm:grid-cols-[10rem_10rem_minmax(0,1fr)] dark:border-zinc-700"
            >
                <select name="project" required="" class=(ui::INPUT)>
                    for p in projects { <option value=(p.clone())>(p)</option> }
                </select>
                <select name="hire" class=(ui::INPUT)>
                    <option value="">"Pick for me"</option>
                    for h in hires { <option value=(h.clone())>(h)</option> }
                </select>
                <input name="title" required="" placeholder="What should be done" class=(ui::INPUT)>
                <textarea
                    name="detail"
                    rows="4"
                    placeholder="Details (optional): context, steps, what done looks like."
                    class=(format!("{} sm:col-span-3", ui::INPUT))
                ></textarea>
                <textarea
                    name="goal"
                    rows="2"
                    placeholder="Goal (optional). Tickets with the same goal land on one branch."
                    class=(format!("{} sm:col-span-3", ui::INPUT))
                ></textarea>
                <div class="sm:col-span-3">
                    <button type="submit" class=(ui::PRIMARY)>"Create ticket"</button>
                </div>
            </form>
        </details>
    })
}

/// What the summary strip counts.
#[derive(Debug, PartialEq)]
struct Counts {
    running: usize,
    /// Pending asks plus held tickets.
    needs_you: usize,
    ready: usize,
}

fn counts(tickets: &[Ticket], asks: &[AskView]) -> Counts {
    let in_state = |kinds: &[&str]| {
        tickets
            .iter()
            .filter(|t| kinds.contains(&t.state.kind()))
            .count()
    };
    Counts {
        running: in_state(LANES[2].kinds),
        needs_you: asks.len() + in_state(&["held"]),
        ready: in_state(&["ready"]),
    }
}

// Class strings are literal so Tailwind finds them. The owner's lanes span
// the row and carry an accent; the others share the grid.
fn lane_class(id: &str) -> &'static str {
    match id {
        "needs-you" => {
            "min-w-0 scroll-mt-4 rounded-lg border border-l-4 border-amber-300 border-l-amber-500 bg-amber-50 p-3 md:col-span-2 xl:col-span-3 dark:border-amber-800 dark:border-l-amber-500 dark:bg-amber-950/30"
        }
        "ready" => {
            "min-w-0 scroll-mt-4 rounded-lg border border-l-4 border-emerald-300 border-l-emerald-500 bg-emerald-50 p-3 md:col-span-2 xl:col-span-3 dark:border-emerald-800 dark:border-l-emerald-500 dark:bg-emerald-950/30"
        }
        _ => "min-w-0 scroll-mt-4",
    }
}

fn lane_title(id: &str) -> &'static str {
    match id {
        "needs-you" => {
            "mb-2 flex items-center gap-2 px-1 text-[13px] font-semibold text-amber-900 dark:text-amber-200"
        }
        "ready" => {
            "mb-2 flex items-center gap-2 px-1 text-[13px] font-semibold text-emerald-900 dark:text-emerald-200"
        }
        _ => {
            "mb-2 flex items-center gap-2 px-1 text-[12px] font-medium text-zinc-600 dark:text-zinc-400"
        }
    }
}

fn lane_cards(id: &str) -> &'static str {
    match id {
        "needs-you" => "grid grid-cols-1 gap-2",
        "ready" => "grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-3",
        _ => "space-y-2",
    }
}

#[component]
async fn columns(
    tickets: Vec<Ticket>,
    etas: Vec<Estimate>,
    asks: Vec<AskView>,
    now: HashMap<String, String>,
    spent: f64,
    daily: f64,
) -> topcoat::Result<impl View> {
    let Counts {
        running,
        needs_you,
        ready,
    } = counts(&tickets, &asks);
    let money = format!("{} of {} today", ui::usd(spent), ui::usd(daily));
    let link = "rounded-md px-2 py-1 hover:bg-zinc-100 dark:hover:bg-zinc-800";
    let need_tone = if needs_you > 0 {
        "font-medium text-amber-800 dark:text-amber-300"
    } else {
        ui::MUTED
    };
    let ready_tone = if ready > 0 {
        "font-medium text-emerald-800 dark:text-emerald-300"
    } else {
        ui::MUTED
    };
    // Empty lanes are dropped, so they take no room.
    let groups: Vec<(&Lane, Vec<Ticket>)> = LANES
        .iter()
        .map(|lane| {
            let mut list: Vec<Ticket> = tickets
                .iter()
                .filter(|t| lane.kinds.contains(&t.state.kind()))
                .cloned()
                .collect();
            if lane.id == "landed" {
                list.reverse();
                list.truncate(LANDED_SHOWN);
            }
            (lane, list)
        })
        .filter(|(_, list)| !list.is_empty())
        .collect();
    let nothing = groups.is_empty();
    let stopped: Vec<Ticket> = tickets
        .into_iter()
        .filter(|t| matches!(t.state.kind(), "failed" | "cancelled"))
        .rev()
        .collect();
    let stopped_count = stopped.len();

    Ok(view! {
        <div id="board-columns">
            <nav id="board-summary" aria-label="Summary" class=(format!("mb-4 flex flex-wrap items-center gap-x-1 gap-y-1 p-1 text-[13px] {}", ui::CARD))>
                <a href="#lane-working" class=(format!("{link} {}", ui::SECONDARY))>
                    <span class="font-semibold tabular-nums" data-count="running">(running)</span> " running"
                </a>
                <a href="/asks" class=(format!("{link} {need_tone}"))>
                    <span class="font-semibold tabular-nums" data-count="needs-you">(needs_you)</span> " need you"
                </a>
                <a href="#lane-ready" class=(format!("{link} {ready_tone}"))>
                    <span class="font-semibold tabular-nums" data-count="ready">(ready)</span> " ready"
                </a>
                <a href="/budget" class=(format!("{link} tabular-nums {}", ui::SECONDARY))>(money)</a>
            </nav>
            if nothing {
                <p class="px-1 text-zinc-600 dark:text-zinc-400">"Nothing queued"</p>
            }
            <div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
                for (lane, list) in groups {
                    <section id=(format!("lane-{}", lane.id)) class=(lane_class(lane.id))>
                        <h2 class=(lane_title(lane.id))>
                            (lane.title)
                            <span class="tabular-nums">(list.len())</span>
                        </h2>
                        <div class=(lane_cards(lane.id))>
                            #[key(ticket.id.to_string())]
                            for ticket in list {
                                let id = ticket.id.to_string();
                                card(
                                    eta: eta_for(&etas, &ticket),
                                    now: now.get(&id).cloned(),
                                    asks: asks.iter().filter(|a| a.ticket == id).cloned().collect(),
                                    ticket: ticket,
                                )
                            }
                        </div>
                    </section>
                }
            </div>
            if stopped_count > 0 {
                <details class="mt-6">
                    <summary class="cursor-pointer select-none px-1 text-[12px] font-medium text-zinc-600 dark:text-zinc-400">
                        "Failed and cancelled " <span class="tabular-nums">(stopped_count)</span>
                    </summary>
                    <div class="mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
                        #[key(ticket.id.to_string())]
                        for ticket in stopped {
                            card(eta: None, now: None, asks: Vec::new(), ticket: ticket)
                        }
                    </div>
                </details>
            }
        </div>
    })
}

/// "~12m" for a queued or running ticket whose finish time is known.
fn eta_for(etas: &[Estimate], ticket: &Ticket) -> Option<String> {
    if !matches!(
        ticket.state.kind(),
        "queued" | "claimed" | "working" | "checking" | "reviewing"
    ) {
        return None;
    }
    let id = ticket.id.to_string();
    ui::eta(etas.iter().find(|e| e.ticket == id)?.eta_ms)
}

#[component]
async fn card(
    ticket: Ticket,
    eta: Option<String>,
    now: Option<String>,
    asks: Vec<AskView>,
) -> topcoat::Result<impl View> {
    let detail = ui::state_detail(&ticket.state);
    let plan = ticket.kind == TicketKind::Plan;
    let land = format!("@post('/tickets/{}/land')", ticket.id);
    let ready = ticket.state.kind() == "ready";
    let project = ticket.project.to_string();
    // The "from" link sits outside the card's own link: anchors can't nest.
    Ok(view! {
        <div class="rounded-lg border border-zinc-200 bg-white hover:border-zinc-300 hover:shadow-sm dark:border-zinc-800 dark:bg-zinc-900 dark:hover:border-zinc-700">
            <a href=(format!("/tickets/{}", ticket.id)) class="block p-2.5">
                <div class="flex items-center justify-between gap-2">
                    <span class="flex items-center gap-1.5">
                        <span class="font-mono text-[11px] text-zinc-600 dark:text-zinc-400">(ticket.id.to_string())</span>
                        if plan {
                            <span class="rounded bg-indigo-50 px-1 py-px text-[10px] font-medium text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300">"plan"</span>
                        }
                    </span>
                    ui::state_pill(state: ticket.state.kind())
                </div>
                <p class="mt-1 line-clamp-2 break-words font-medium leading-snug" title=(ticket.title.clone())>(ticket.title.clone())</p>
                <div class="mt-1 flex min-w-0 flex-wrap items-center gap-1">
                    ui::badge(text: ticket.project.to_string())
                    ui::badge(text: ticket.hire.to_string())
                    if let Some(eta) = eta {
                        <span class="text-[12px] tabular-nums text-zinc-600 dark:text-zinc-400">(eta)</span>
                    }
                </div>
                if let Some(detail) = detail {
                    <p class="mt-1 truncate text-[12px] text-amber-800 dark:text-amber-300">(detail)</p>
                }
                if let Some(now) = now {
                    <p class="mt-1 flex gap-1 text-[12px]" title=(now.clone())>
                        <span class="shrink-0 text-zinc-600 dark:text-zinc-400">"now:"</span>
                        <span class=(format!("min-w-0 truncate font-mono {}", ui::SECONDARY))>(now)</span>
                    </p>
                }
            </a>
            for ask in asks {
                <div class="px-2 pb-2">
                    ask_card(ask: ask, title: String::new(), project: project.clone(), compact: true)
                </div>
            }
            if ready {
                <div class="px-2.5 pb-2.5">
                    <button class=(format!("{} min-h-10 w-full sm:min-h-0", ui::PRIMARY)) data-on:click=(land)>"Land"</button>
                </div>
            }
            if let Some(parent) = ticket.parent {
                <p class="-mt-1 px-2.5 pb-2 text-[12px]">
                    <a href=(format!("/tickets/{parent}")) class="text-zinc-600 hover:underline dark:text-zinc-400">
                        "from " (parent.to_string())
                    </a>
                </p>
            }
        </div>
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn ticket(id: &str, state: serde_json::Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    async fn render(tickets: Vec<Ticket>) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => columns(tickets: tickets, etas: Vec::new(), asks: Vec::new(), now: HashMap::new(), spent: 1.0, daily: 10.0) },
        )
        .await
        .unwrap()
    }

    #[test]
    fn counts_running_needs_you_and_ready() {
        let tickets = vec![
            ticket("G-1", json!("working")),
            ticket("G-2", json!("checking")),
            ticket("G-3", json!({"held": {"reason": "budget"}})),
            ticket("G-4", json!("ready")),
            ticket("G-5", json!("queued")),
        ];
        let ask = AskView {
            id: "ASK-1".into(),
            ticket: "G-3".into(),
            hire: "dev".into(),
            kind: "permission".into(),
            tool: "Bash".into(),
            input: json!({}),
            reasons: Vec::new(),
        };
        assert_eq!(
            counts(&tickets, &[ask]),
            Counts {
                running: 2,
                needs_you: 2,
                ready: 1
            }
        );
    }

    #[tokio::test]
    async fn empty_lanes_are_not_drawn() {
        let html = render(vec![ticket("G-1", json!("queued"))]).await;
        assert!(html.contains(r#"id="lane-queued""#), "{html}");
        for gone in ["needs-you", "ready", "working", "landed"] {
            assert!(!html.contains(&format!(r#"id="lane-{gone}""#)), "{html}");
        }
        assert!(!html.contains("Nothing here"), "{html}");
        assert!(!html.contains("Nothing queued"), "{html}");
        assert!(html.contains("$1.00 of $10.00 today"), "{html}");
    }

    #[tokio::test]
    async fn an_empty_board_says_so_once() {
        let html = render(Vec::new()).await;
        assert_eq!(html.matches("Nothing queued").count(), 1, "{html}");
        assert!(!html.contains(r#"id="lane-"#), "{html}");
    }
}
