use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use studio_config::draft::from_feedback;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{AskView, Estimate, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::{ElementPatchMode, PatchElements},
    router::{
        content::{
            Form,
            sse::{Event, KeepAlive, Sse},
        },
        page, path_param, query_params, route,
    },
    view::{View, component, view},
};

use crate::summary::{latest_activity, tool_arg, truncate};
use crate::{AppConfig, client, live, ui};

use super::ask_card::ask_card;
use super::changes;
use super::evidence;
use super::timeline;

path_param!(id);

#[query_params(error = bad_request)]
struct LogFrom {
    from: Option<u64>,
}

/// What the top of a ticket page shows, fetched together.
struct Top {
    ticket: Ticket,
    spent: f64,
    estimate: Option<Estimate>,
    asks: Vec<AskView>,
    now: Option<String>,
    /// When the current run started, for a ticket that is running.
    started: Option<u64>,
    events: Vec<client::Stamped>,
}

/// Whether a run is under way on the ticket, even if paused on an ask.
fn is_live(kind: &str) -> bool {
    matches!(
        kind,
        "claimed" | "working" | "asking" | "checking" | "reviewing"
    )
}

/// When the latest run began: the last time the ticket was claimed.
fn run_started(events: &[client::Stamped]) -> Option<u64> {
    let claimed = |e: &&client::Stamped| matches!(e.event, studio_core::event::Event::Claimed);
    events.iter().rev().find(claimed).map(|e| e.at_ms)
}

async fn top(config: &AppConfig, id: &str) -> topcoat::Result<Top> {
    let ticket = find(&config.socket, id).await?;
    let asks: Vec<AskView> = client::call(&config.socket, Request::Asks)
        .await
        .unwrap_or_default();
    let now = now_text(&ticket, &config.data_dir).await;
    let events = ticket_events(&config.socket, id).await;
    let started = if is_live(ticket.state.kind()) {
        run_started(&events)
    } else {
        None
    };
    Ok(Top {
        spent: spent(&config.socket, id).await,
        estimate: estimate(&config.socket, id).await,
        asks: asks.into_iter().filter(|a| a.ticket == id).collect(),
        now,
        started,
        events,
        ticket,
    })
}

/// The fragments of the page's top, in the order they are shown.
async fn top_parts(cx: &Cx, top: Top) -> topcoat::Result<[String; 4]> {
    let project = top.ticket.project.to_string();
    let Top {
        ticket,
        spent,
        estimate,
        asks,
        now,
        started,
        events: _,
    } = top;
    let titled = ticket.clone();
    Ok([
        ui::html(cx, view! { cx => head(ticket: titled) }).await?,
        ui::html(
            cx,
            view! { cx => ticket_asks(asks: asks, project: project) },
        )
        .await?,
        ui::html(cx, view! { cx => now_line(now: now) }).await?,
        ui::html(
            cx,
            view! { cx => details(ticket: ticket, spent: spent, estimate: estimate, started: started) },
        )
        .await?,
    ])
}

#[page("/tickets/{id}")]
async fn ticket_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let top = top(config, &id).await?;
    let project = top.ticket.project.to_string();
    let Top {
        ticket,
        spent,
        estimate,
        asks,
        now,
        started,
        events,
    } = top;
    let (text, end) = read_from(&log_path(&config.data_dir, &id), 0).await;
    let entries = group_lines(log_lines(&text));
    let timeline_rows = timeline::rows(&events, ui::now_ms());
    let stream = format!("@get('/tickets/{id}/stream?from={end}')");
    // Fetched once: a diff is too big to re-send every second.
    let review = matches!(ticket.state.kind(), "ready" | "working" | "reviewing");
    let (stat, diff) = if review {
        let stat: Option<String> =
            client::call(&config.socket, Request::Diffstat { ticket: id.clone() })
                .await
                .unwrap_or_default();
        let diff: Option<String> =
            client::call(&config.socket, Request::Diff { ticket: id.clone() })
                .await
                .unwrap_or_default();
        (stat, diff)
    } else {
        (None, None)
    };
    let captures = if review {
        evidence::files(&config.socket, &id).await
    } else {
        Vec::new()
    };

    Ok(view! {
        <a href="/" class="text-[12px] text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100">
            "← Board"
        </a>
        <div data-init=(stream)>
            head(ticket: ticket.clone())
            ticket_asks(asks: asks, project: project)
            now_line(now: now)
            details(ticket: ticket, spent: spent, estimate: estimate, started: started)
        </div>
        timeline::timeline(rows: timeline_rows)
        evidence::art_pairs(ticket: id.clone(), files: captures.clone())
        evidence::screenshots(ticket: id.clone(), files: captures)
        if review {
            changes::changes(stat: stat, diff: diff)
        }
        <section class="mt-6">
            <h2 class="mb-2 text-[12px] font-medium text-zinc-500 dark:text-zinc-400">"Run log"</h2>
            // Reversed, so the box opens scrolled to the newest entry and stays there.
            <div
                id="log"
                class="flex max-h-[70vh] flex-col-reverse overflow-y-auto rounded-lg border border-zinc-300 bg-white p-3 text-[13px] leading-relaxed dark:border-zinc-700 dark:bg-zinc-900"
            >
                <div id="log-body" class="min-w-0">
                    log(entries: entries)
                </div>
            </div>
        </section>
    })
}

#[route(GET "/tickets/{id}/stream")]
async fn ticket_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let id = path_param::<Id>(cx).to_owned();
    let from = query_params::<LogFrom>(cx)?.from.unwrap_or(0);
    let cx = cx.clone();
    // The log the page already shows: new lines may join its last group, so
    // the whole log is grouped again whenever it grows.
    let (seen, _) = read_from(&log_path(&config.data_dir, &id), 0).await;
    let seen = seen.get(..from as usize).unwrap_or_default().to_string();
    // What was last sent for the four top fragments and the timeline.
    let state = (vec![String::new(); 5], from, seen);
    let events = stream::unfold(state, move |(mut last, pos, mut seen)| {
        let (config, id, cx) = (config.clone(), id.clone(), cx.clone());
        async move {
            tokio::time::sleep(live::EVERY).await;
            let mut events = Vec::new();
            let mut parts: Vec<String> = Vec::new();
            if let Ok(top) = top(&config, &id).await {
                let rows = timeline::rows(&top.events, ui::now_ms());
                if let Ok(rendered) = top_parts(&cx, top).await {
                    parts.extend(rendered);
                    if let Ok(html) =
                        ui::html(&cx, view! { cx => timeline::timeline(rows: rows) }).await
                    {
                        parts.push(html);
                    }
                }
            }
            if parts.len() == last.len() {
                for (html, seen) in parts.into_iter().zip(last.iter_mut()) {
                    if html != *seen {
                        seen.clone_from(&html);
                        events.push(PatchElements::new(html).into());
                    }
                }
            }
            let (text, pos) = read_from(&log_path(&config.data_dir, &id), pos).await;
            if !text.is_empty() {
                seen.push_str(&text);
                let entries = group_lines(log_lines(&seen));
                if let Ok(html) = ui::html(&cx, view! { cx => log(entries: entries) }).await {
                    events.push(
                        PatchElements::new(html)
                            .selector("#log-body")
                            .mode(ElementPatchMode::Inner)
                            .into(),
                    );
                }
            }
            Some((events, (last, pos, seen)))
        }
    })
    .flat_map(|events: Vec<Event>| stream::iter(events.into_iter().map(Ok)));
    Ok(Sse::new(events).keep_alive(KeepAlive::new()))
}

#[derive(Deserialize)]
struct Feedback {
    feedback: String,
}

#[route(POST "/tickets/{id}/land")]
async fn land(cx: &Cx) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let request = Request::Land { ticket: id.clone() };
    let reply = match client::call::<Value>(&config.socket, request).await {
        Ok(reply) => reply,
        Err(err) => return ui::notice(cx, err.to_string(), true).await,
    };
    let branch = reply["branch"].as_str().unwrap_or_default().to_string();
    let base = find(&config.socket, &id)
        .await
        .map(|t| t.base_branch)
        .unwrap_or_default();
    let text = ui::html(
        cx,
        view! { cx =>
            <p class="text-zinc-600 dark:text-zinc-400">
                "Landed on " <span class="font-mono">(branch)</span> ". Merge it into "
                <span class="font-mono">(base.clone())</span> " when you're happy."
            </p>
            merge_button(ticket: id, base: base)
        },
    )
    .await?;
    Ok(PatchElements::new(text)
        .selector("#notice")
        .mode(ElementPatchMode::Inner))
}

/// Merges the goal branch a landed ticket is on into its base branch.
#[route(POST "/tickets/{id}/merge")]
async fn merge(cx: &Cx) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    match client::call::<Value>(&config.socket, Request::Merge { ticket: id }).await {
        Ok(reply) => {
            let text = |key: &str| reply[key].as_str().unwrap_or_default().to_string();
            let sha = text("sha");
            let open = reply["open"].as_array().map_or(0, Vec::len);
            let mut done = format!(
                "Merged {} into {} ({}).",
                text("branch"),
                text("into"),
                sha.get(..7).unwrap_or(&sha)
            );
            if open > 0 {
                done.push_str(&format!(" {open} ticket(s) on this goal are still open."));
            }
            ui::notice(cx, done, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The button, and what it does, shared by the ticket page and the goals.
#[component]
pub async fn merge_button(ticket: String, base: String) -> topcoat::Result<impl View> {
    let post = format!("@post('/tickets/{ticket}/merge')");
    Ok(view! {
        <div class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
            <button class=(format!("{} min-h-10 sm:min-h-0", ui::BUTTON)) data-on:click=(post)>"Merge into " (base)</button>
            <span class=(format!("text-[12px] {}", ui::MUTED))>"Merges in your checkout; nothing is pushed."</span>
        </div>
    })
}

#[route(POST "/tickets/{id}/changes")]
async fn request_changes(cx: &Cx, Form(form): Form<Feedback>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let project = find(&config.socket, &id)
        .await
        .map(|t| t.project.to_string());
    let request = Request::RequestChanges {
        ticket: id,
        feedback: form.feedback.clone(),
    };
    let done = "Sent back with your feedback.".to_string();
    match client::call::<Value>(&config.socket, request).await {
        Ok(_) => match project {
            Ok(project) => {
                let draft = from_feedback(&project, &form.feedback);
                let label = format!("Make it a rule for every ticket in {project}");
                ui::rule_notice(cx, done, label, &draft).await
            }
            Err(_) => ui::notice(cx, done, false).await,
        },
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[route(POST "/tickets/{id}/cancel")]
async fn cancel(cx: &Cx) -> topcoat::Result<PatchElements> {
    act(cx, |ticket| Request::Cancel { ticket }, "Cancelled.").await
}

#[route(POST "/tickets/{id}/retry")]
async fn retry(cx: &Cx) -> topcoat::Result<PatchElements> {
    act(cx, |ticket| Request::Retry { ticket }, "Queued again.").await
}

#[route(POST "/tickets/{id}/resume")]
async fn resume(cx: &Cx) -> topcoat::Result<PatchElements> {
    act(cx, |ticket| Request::Resume { ticket }, "Resumed.").await
}

/// Runs an action and reports it; the stream updates the header.
async fn act(
    cx: &Cx,
    request: impl FnOnce(String) -> Request,
    done: &str,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    match client::call::<Value>(&config.socket, request(id)).await {
        Ok(_) => ui::notice(cx, done.to_string(), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

async fn find(socket: &Path, id: &str) -> topcoat::Result<Ticket> {
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await?;
    tickets
        .into_iter()
        .find(|t| t.id.to_string() == id)
        .ok_or_else(|| client::error(format!("no ticket {id}")))
}

/// The ticket's history; empty if studiod can't say.
async fn ticket_events(socket: &Path, id: &str) -> Vec<client::Stamped> {
    let request = Request::Events {
        since: 0,
        // All of them: the daemon keeps the oldest, and the newest say when
        // the current run began.
        limit: usize::MAX,
        ticket: Some(id.into()),
    };
    client::call(socket, request).await.unwrap_or_default()
}

async fn spent(socket: &Path, id: &str) -> f64 {
    client::call(socket, Request::TicketSpend { ticket: id.into() })
        .await
        .unwrap_or(0.0)
}

async fn estimate(socket: &Path, id: &str) -> Option<Estimate> {
    let all = client::estimates(socket).await;
    all.into_iter().find(|e| e.ticket == id)
}

const GOAL_SHOWN: usize = 48;

/// The facts under the title as (label, value, tooltip). While a run is live
/// and nothing is booked yet, the spend is the quote, labelled as an estimate.
fn facts(
    ticket: &Ticket,
    spent: f64,
    estimate: Option<&Estimate>,
    started: Option<u64>,
    now: u64,
) -> Vec<(&'static str, String, String)> {
    let fact = |label, value: String| (label, value.clone(), value);
    let mut quote = estimate.and_then(|e| e.quote_usd);
    let live = is_live(ticket.state.kind());
    let mut facts = vec![
        fact("Project", ticket.project.to_string()),
        fact("Hire", ticket.hire.to_string()),
        fact("Base", ticket.base_branch.clone()),
        fact("Attempts", ticket.attempts.to_string()),
    ];
    match quote {
        Some(usd) if live && spent == 0.0 => {
            facts.push(fact("Est. spend", format!("~{}", ui::usd(usd))));
            quote = None;
        }
        _ => facts.push(fact("Spent", ui::usd(spent))),
    }
    if let Some(started) = started {
        facts.push(fact("Elapsed", ui::elapsed(now.saturating_sub(started))));
    }
    if let Some(usd) = quote {
        facts.push(fact("Quote", format!("~{}", ui::usd(usd))));
    }
    if let Some(eta) = estimate.and_then(|e| ui::eta(e.eta_ms)) {
        facts.push(fact("ETA", eta));
    }
    if let Some(name) = ticket.goal.lines().next().filter(|l| !l.is_empty()) {
        facts.push(("Goal", truncate(name, GOAL_SHOWN), ticket.goal.clone()));
    }
    facts
}

/// The last stretch of a run log, for what the agent is doing now.
pub async fn log_tail(data_dir: &Path, id: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    const TAIL: u64 = 32 * 1024;
    let Ok(mut file) = tokio::fs::File::open(log_path(data_dir, id)).await else {
        return String::new();
    };
    let len = file.metadata().await.map_or(0, |m| m.len());
    let start = len.saturating_sub(TAIL);
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).await.is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A read that starts mid-file begins in the middle of a line.
    match (start, text.split_once('\n')) {
        (0, _) => text,
        (_, Some((_, rest))) => rest.to_string(),
        (_, None) => String::new(),
    }
}

const NOW_SHOWN: usize = 160;

/// What a live ticket is doing right now; `None` for one that isn't running.
async fn now_text(ticket: &Ticket, data_dir: &Path) -> Option<String> {
    match ticket.state.kind() {
        "working" | "claimed" => {
            let tail = log_tail(data_dir, &ticket.id.to_string()).await;
            Some(latest_activity(&tail, NOW_SHOWN).unwrap_or_else(|| "Starting up".to_string()))
        }
        "checking" => Some("Running the project's checks".to_string()),
        "reviewing" => Some("Being reviewed".to_string()),
        _ => None,
    }
}

#[component]
async fn head(ticket: Ticket) -> topcoat::Result<impl View> {
    let kind = ticket.state.kind();
    let detail = ui::state_detail(&ticket.state);
    Ok(view! {
        <header id="ticket-head" class="mt-3 space-y-3">
            <div class="flex flex-wrap items-center gap-2">
                <span class="font-mono text-[12px] text-zinc-500 dark:text-zinc-400">(ticket.id.to_string())</span>
                ui::state_pill(state: kind)
                if let Some(detail) = detail {
                    <span class="text-[12px] text-amber-700 dark:text-amber-400">(detail)</span>
                }
            </div>
            <h1 class=(format!("leading-tight {}", ui::PAGE_TITLE))>(ticket.title)</h1>
        </header>
    })
}

/// The asks waiting on this ticket, right under its title.
#[component]
async fn ticket_asks(asks: Vec<AskView>, project: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="ticket-asks" class="space-y-3 empty:hidden [&:not(:empty)]:mt-3">
            #[key(ask.id.clone())]
            for ask in asks {
                ask_card(ask: ask, title: String::new(), project: project.clone(), compact: false)
            }
        </div>
    })
}

#[component]
async fn now_line(now: Option<String>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="ticket-now" class="empty:hidden [&:not(:empty)]:mt-3">
            if let Some(now) = now {
                <p class="flex items-baseline gap-2 text-[12px]">
                    <span class="relative top-px inline-block size-1.5 shrink-0 animate-pulse rounded-full bg-sky-500"></span>
                    <span class="shrink-0 font-medium text-zinc-500 dark:text-zinc-400">"Now"</span>
                    <span class="min-w-0 truncate font-mono text-zinc-700 dark:text-zinc-300" title=(now.clone())>(now)</span>
                </p>
            }
        </div>
    })
}

#[component]
async fn details(
    ticket: Ticket,
    spent: f64,
    estimate: Option<Estimate>,
    started: Option<u64>,
) -> topcoat::Result<impl View> {
    let kind = ticket.state.kind();
    let id = ticket.id.to_string();
    let [on_land, on_resume, on_retry, on_cancel] =
        ["land", "resume", "retry", "cancel"].map(|a| format!("@post('/tickets/{id}/{a}')"));
    let facts = facts(&ticket, spent, estimate.as_ref(), started, ui::now_ms());
    // A plan lands without a branch of its own to merge.
    let mergeable = kind == "landed" && ticket.kind != TicketKind::Plan;
    let send_back = format!("@post('/tickets/{id}/changes', {{contentType: 'form'}})");
    Ok(view! {
        <div id="ticket-details" class="mt-3 space-y-3">
            <div class="flex flex-wrap gap-1.5">
                for (label, value, title) in facts {
                    chip(label: label, value: value, title: title)
                }
            </div>
            if let Some(feedback) = ticket.feedback {
                <p class="max-w-3xl whitespace-pre-wrap rounded-md bg-zinc-100 px-2 py-1 text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">
                    "Feedback: " (feedback)
                </p>
            }
            <div class="flex flex-wrap items-start gap-2">
                if kind == "ready" {
                    <button class=(ui::PRIMARY) data-on:click=(on_land)>"Land"</button>
                }
                if kind == "held" {
                    <button class=(ui::PRIMARY) data-on:click=(on_resume)>"Resume"</button>
                }
                if kind == "failed" {
                    <button class=(ui::PRIMARY) data-on:click=(on_retry)>"Retry"</button>
                }
                if !matches!(kind, "landed" | "cancelled") {
                    <button class=(ui::QUIET) data-on:click=(on_cancel)>"Cancel"</button>
                }
            </div>
            if mergeable {
                merge_button(ticket: id.clone(), base: ticket.base_branch.clone())
            }
            if kind == "ready" {
                <form data-on:submit=(send_back) class="flex max-w-3xl flex-col gap-2 sm:flex-row sm:items-start">
                    <textarea name="feedback" required="" rows="2" placeholder="What should change?" class=(ui::INPUT)></textarea>
                    <button type="submit" class=(ui::BUTTON)>"Request changes"</button>
                </form>
            }
        </div>
    })
}

/// A labelled fact: the label muted, the value beside it, `title` on hover.
#[component]
async fn chip(label: &'static str, value: String, title: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span title=(title) class="inline-flex max-w-full items-baseline gap-1.5 rounded-md bg-zinc-100 px-2 py-1 text-[12px] ring-1 ring-inset ring-zinc-300 dark:bg-zinc-800 dark:ring-zinc-600">
            <span class=(format!("shrink-0 {}", ui::MUTED))>(label)</span>
            <span class="min-w-0 truncate font-medium">(value)</span>
        </span>
    })
}

/// One rendered line of the run log.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    kind: LineKind,
    text: String,
    /// A tool call's tool, and its raw input for a tooltip.
    tool: String,
    raw: String,
}

impl Line {
    fn new(kind: LineKind, text: String) -> Line {
        Line {
            kind,
            text,
            tool: String::new(),
            raw: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LineKind {
    Text,
    Tool,
    Ask,
    Error,
    Note,
    End,
}

const TOOL_INPUT_SHOWN: usize = 300;
const TOOL_ARG_SHOWN: usize = 200;

/// The same lines `studio log` prints, from the run log's JSON lines.
fn log_lines(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        if let Some(note) = entry["note"].as_str() {
            // A rule answered the ask the line before: show only the answer.
            if note.starts_with("allowed ")
                && lines.last().is_some_and(|l: &Line| l.kind == LineKind::Ask)
            {
                lines.pop();
            }
            lines.push(Line::new(LineKind::Note, note.to_string()));
            continue;
        }
        let e = &entry["event"];
        let str_of = |key: &str| e[key].as_str().unwrap_or_default().to_string();
        let line = match e["type"].as_str().unwrap_or_default() {
            "text" => Line::new(LineKind::Text, str_of("text")),
            "tool_call" => {
                let name = str_of("name");
                let arg = truncate(&tool_arg(&name, &e["input"]), TOOL_ARG_SHOWN);
                Line {
                    tool: name,
                    raw: truncate(&e["input"].to_string(), TOOL_INPUT_SHOWN),
                    ..Line::new(LineKind::Tool, arg)
                }
            }
            "tool_result" if e["is_error"] == true => Line::new(LineKind::Error, str_of("output")),
            "permission" => Line::new(LineKind::Ask, format!("asked to use {}", str_of("tool"))),
            "question" => Line::new(LineKind::Ask, "asked a question".to_string()),
            "finished" => {
                let outcome = e["outcome"]["outcome"].as_str().unwrap_or("unknown");
                let cost = e["cost"]["usd"].as_f64().unwrap_or(0.0);
                Line::new(
                    LineKind::End,
                    format!("finished: {outcome} · {}", ui::usd(cost)),
                )
            }
            _ => continue,
        };
        lines.push(line);
    }
    lines
}

/// Consecutive calls of one tool, with the policy rules that allowed them.
#[derive(Clone, Debug, PartialEq)]
struct Group {
    tool: String,
    calls: Vec<Line>,
    rules: Vec<String>,
}

/// What the run log shows: a line, a group of tool calls, or rule lines that
/// belong to no call.
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Line(Line),
    Group(Group),
    Rules(Vec<String>),
}

/// The tool an "allowed ..." policy line is about.
fn ruled_tool(line: &Line) -> Option<&str> {
    if line.kind != LineKind::Note {
        return None;
    }
    line.text.strip_prefix("allowed ")?.split(':').next()
}

/// Folds runs of the same tool into one entry. The log writes a rule line
/// after its call, so it joins the nearest group of that tool; a rule with no
/// such group becomes one entry with a count.
fn group_lines(lines: Vec<Line>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in lines {
        if let Some(tool) = ruled_tool(&line).map(str::to_string) {
            let group = entries
                .iter_mut()
                .rev()
                .take_while(|e| !matches!(e, Entry::Line(_)))
                .find_map(|e| match e {
                    Entry::Group(group) if group.tool == tool => Some(group),
                    _ => None,
                });
            if let Some(group) = group {
                group.rules.push(line.text);
            } else if let Some(Entry::Rules(rules)) = entries.last_mut() {
                rules.push(line.text);
            } else {
                entries.push(Entry::Rules(vec![line.text]));
            }
            continue;
        }
        if line.kind != LineKind::Tool {
            entries.push(Entry::Line(line));
            continue;
        }
        match entries.last_mut() {
            Some(Entry::Group(group)) if group.tool == line.tool => group.calls.push(line),
            _ => entries.push(Entry::Group(Group {
                tool: line.tool.clone(),
                calls: vec![line],
                rules: Vec::new(),
            })),
        }
    }
    entries
}

/// Entries only grow at the end, so an entry's place is its id: that and
/// `data-preserve-attr` keep a group open when the stream re-sends the log.
#[component]
async fn log(entries: Vec<Entry>) -> topcoat::Result<impl View> {
    Ok(view! {
        if entries.is_empty() {
            <p class=(ui::MUTED)>"Nothing yet."</p>
        }
        for (index, entry) in entries.into_iter().enumerate() {
            match entry {
                Entry::Line(line) => log_line(line: line),
                Entry::Group(group) => tool_group(group: group, id: format!("log-entry-{index}")),
                Entry::Rules(rules) => rule_lines(rules: rules, id: format!("log-entry-{index}")),
            }
        }
    })
}

#[component]
async fn log_line(line: Line) -> topcoat::Result<impl View> {
    Ok(view! {
        match line.kind {
            LineKind::Text => <p class="my-2 max-w-prose whitespace-pre-wrap text-[15px] leading-relaxed text-zinc-900 dark:text-zinc-100">(line.text)</p>,
            LineKind::Tool => tool_row(line: line),
            LineKind::Ask => <p class="text-[12px] text-amber-700 dark:text-amber-400">"? " (line.text)</p>,
            LineKind::Error => <p class="whitespace-pre-wrap font-mono text-[12px] text-rose-700 dark:text-rose-400">"! " (line.text)</p>,
            LineKind::Note => <p class=(format!("whitespace-pre-wrap text-[12px] {}", ui::MUTED))>"· " (line.text)</p>,
            LineKind::End => <p class="my-2 border-t border-zinc-200 pt-2 text-[12px] text-zinc-500 dark:border-zinc-800 dark:text-zinc-400">"= " (line.text)</p>,
        }
    })
}

/// One tool call: the tool, then its argument, which scrolls if it is long.
#[component]
async fn tool_row(line: Line) -> topcoat::Result<impl View> {
    Ok(view! {
        <p class="flex items-baseline gap-2 py-px" title=(line.raw)>
            <span class="shrink-0 rounded bg-sky-50 px-1.5 py-px text-[11px] font-medium text-sky-700 dark:bg-sky-950 dark:text-sky-300">(line.tool)</span>
            <span class=(format!("min-w-0 overflow-x-auto whitespace-nowrap font-mono text-[12px] {}", ui::MUTED))>(line.text)</span>
        </p>
    })
}

/// "Read ×6" over the newest call; it opens to every call and rule line. A
/// lone call with no rule is just its row.
#[component]
async fn tool_group(group: Group, id: String) -> topcoat::Result<impl View> {
    let Group { tool, calls, rules } = group;
    let plain = calls.len() == 1 && rules.is_empty();
    let last = calls.last().map(|c| c.text.clone()).unwrap_or_default();
    let mut label = format!("{tool} ×{}", calls.len());
    if !rules.is_empty() {
        label.push_str(&format!(" · {} allowed", rules.len()));
    }
    let first = calls.first().cloned();
    Ok(view! {
        if plain {
            if let Some(call) = first {
                tool_row(line: call)
            }
        } else {
            <details id=(id) data-preserve-attr="open" class="py-px">
                <summary class="flex cursor-pointer select-none items-baseline gap-2 marker:content-none">
                    <span class="shrink-0 rounded bg-sky-50 px-1.5 py-px text-[11px] font-medium text-sky-700 dark:bg-sky-950 dark:text-sky-300">(label)</span>
                    <span class=(format!("min-w-0 truncate font-mono text-[12px] {}", ui::MUTED))>(last)</span>
                </summary>
                <div class="ml-2 mt-1 space-y-px border-l border-zinc-300 pl-3 dark:border-zinc-700">
                    for rule in rules {
                        <p class=(format!("overflow-x-auto whitespace-nowrap font-mono text-[11px] {}", ui::MUTED))>"· " (rule)</p>
                    }
                    for call in calls {
                        tool_row(line: call)
                    }
                </div>
            </details>
        }
    })
}

/// Rule lines that follow no tool call: one quiet line, or a count that opens
/// to them.
#[component]
async fn rule_lines(rules: Vec<String>, id: String) -> topcoat::Result<impl View> {
    let label = format!("· {} rules applied", rules.len());
    Ok(view! {
        if rules.len() == 1 {
            for rule in rules {
                <p class=(format!("text-[12px] {}", ui::MUTED))>"· " (rule)</p>
            }
        } else {
            <details id=(id) data-preserve-attr="open" class="text-[12px]">
                <summary class=(format!("cursor-pointer select-none {}", ui::MUTED))>(label)</summary>
                <div class="ml-2 mt-1 border-l border-zinc-300 pl-3 dark:border-zinc-700">
                    for rule in rules {
                        <p class=(format!("overflow-x-auto whitespace-nowrap font-mono text-[11px] {}", ui::MUTED))>(rule)</p>
                    }
                </div>
            </details>
        }
    })
}

fn log_path(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("runs").join(format!("{id}.jsonl"))
}

/// Complete lines from byte `pos` on, and where the next read starts. A
/// partly written last line is left for the next read.
async fn read_from(path: &Path, pos: u64) -> (String, u64) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return (String::new(), pos);
    };
    if file.seek(std::io::SeekFrom::Start(pos)).await.is_err() {
        return (String::new(), pos);
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).await.is_err() {
        return (String::new(), pos);
    }
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(complete);
    let next = pos + complete as u64;
    (String::from_utf8_lossy(&bytes).into_owned(), next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_follow_the_cli() {
        let text = [
            r#"{"event":{"type":"started","session":"s"}}"#,
            r#"{"event":{"type":"tool_call","name":"Bash","input":{"command":"ls"}}}"#,
            r#"{"event":{"type":"permission","tool":"Bash"}}"#,
            r#"{"note":"`cargo test` failed"}"#,
            "not json",
            r#"{"event":{"type":"finished","outcome":{"outcome":"success"},"cost":{"usd":0.0019}}}"#,
        ]
        .join("\n");
        let lines = log_lines(&text);
        let kinds: Vec<LineKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [LineKind::Tool, LineKind::Ask, LineKind::Note, LineKind::End]
        );
        assert_eq!(lines[0].tool, "Bash");
        assert_eq!(lines[0].text, "ls");
        assert_eq!(lines[0].raw, r#"{"command":"ls"}"#);
        assert_eq!(lines[3].text, "finished: success · $0.0019");
    }

    #[test]
    fn long_tool_input_is_cut_on_a_char_boundary() {
        let input = "é".repeat(400);
        let raw =
            serde_json::json!({"event": {"type": "tool_call", "name": "Write", "input": input}});
        let lines = log_lines(&raw.to_string());
        assert!(lines[0].text.ends_with('…'));
    }

    fn call(tool: &str, arg: &str) -> String {
        let input = if tool == "Bash" {
            serde_json::json!({"command": arg})
        } else {
            serde_json::json!({"file_path": arg})
        };
        serde_json::json!({"event": {"type": "tool_call", "name": tool, "input": input}})
            .to_string()
    }

    /// A call that a rule let through, in the order the worker logs them.
    fn allowed(tool: &str, arg: &str) -> Vec<String> {
        vec![
            call(tool, arg),
            format!(r#"{{"event":{{"type":"permission","tool":"{tool}"}}}}"#),
            format!(r#"{{"note":"allowed {tool}: inside the worktree"}}"#),
        ]
    }

    fn shape(text: &str) -> Vec<String> {
        group_lines(log_lines(text))
            .iter()
            .map(|e| match e {
                Entry::Line(l) => format!("{:?}", l.kind),
                Entry::Group(g) => format!("{} x{} rules {}", g.tool, g.calls.len(), g.rules.len()),
                Entry::Rules(r) => format!("rules {}", r.len()),
            })
            .collect()
    }

    #[test]
    fn runs_of_one_tool_fold_into_groups_with_their_rules() {
        let text = [
            vec![
                r#"{"event":{"type":"text","text":"Looking around"}}"#.to_string(),
                call("Read", "a.rs"),
                call("Read", "b.rs"),
                call("Read", "c.rs"),
            ],
            allowed("Edit", "a.rs"),
            allowed("Edit", "b.rs"),
            allowed("Bash", "cargo test"),
            vec![
                r#"{"event":{"type":"text","text":"Done"}}"#.to_string(),
                r#"{"note":"checks passed"}"#.to_string(),
            ],
        ]
        .concat()
        .join("\n");
        assert_eq!(
            shape(&text),
            [
                "Text",
                "Read x3 rules 0",
                "Edit x2 rules 2",
                "Bash x1 rules 1",
                "Text",
                "Note"
            ]
        );
    }

    #[test]
    fn a_rule_stays_with_its_own_tool() {
        // The next tool's group gets nothing from the call before it.
        let text = [allowed("Edit", "a.rs"), vec![call("Read", "b.rs")]]
            .concat()
            .join("\n");
        assert_eq!(shape(&text), ["Edit x1 rules 1", "Read x1 rules 0"]);
        // Calls made together, then their rules together.
        let text = [
            call("Edit", "a.rs"),
            call("Bash", "ls"),
            r#"{"note":"allowed Edit: inside the worktree"}"#.to_string(),
            r#"{"note":"allowed Bash: ls"}"#.to_string(),
        ]
        .join("\n");
        assert_eq!(shape(&text), ["Edit x1 rules 1", "Bash x1 rules 1"]);
        // Rules with no call of their tool.
        let text = [
            r#"{"note":"allowed Edit: inside the worktree"}"#,
            r#"{"note":"allowed Bash: ls"}"#,
        ]
        .join("\n");
        assert_eq!(shape(&text), ["rules 2"]);
    }

    #[tokio::test]
    async fn a_group_keeps_its_id_and_stays_open_as_the_log_grows() {
        let cx = Cx::default();
        let two = [call("Read", "a.rs"), call("Read", "b.rs")].join("\n");
        let three = [two.clone(), call("Read", "c.rs")].join("\n");
        for text in [two, three] {
            let entries = group_lines(log_lines(&text));
            let html = ui::html(&cx, view! { cx => log(entries: entries) })
                .await
                .unwrap();
            assert!(html.contains(r#"id="log-entry-0""#), "{html}");
            assert!(html.contains(r#"data-preserve-attr="open""#), "{html}");
        }
    }

    #[test]
    fn a_run_of_calls_stays_one_group_when_it_grows() {
        let first = group_lines(log_lines(
            &[call("Read", "a.rs"), call("Read", "b.rs")].join("\n"),
        ));
        let all = group_lines(log_lines(
            &[
                call("Read", "a.rs"),
                call("Read", "b.rs"),
                call("Read", "c.rs"),
            ]
            .join("\n"),
        ));
        assert!(matches!(&first[..], [Entry::Group(g)] if g.calls.len() == 2));
        assert!(matches!(&all[..], [Entry::Group(g)] if g.calls.len() == 3));
    }

    #[test]
    fn facts_show_the_estimate_while_a_run_is_live() {
        let mut ticket: Ticket = serde_json::from_value(serde_json::json!({
            "id": "GAME-2", "project": "game", "title": "t", "goal": "Ship the menus\nand more",
            "hire": "dev", "state": "working", "attempts": 1, "base_branch": "main",
            "created_at": 0,
        }))
        .unwrap();
        let estimate = Estimate {
            ticket: "GAME-2".into(),
            quote_usd: Some(0.42),
            eta_ms: None,
        };
        let shown = |facts: Vec<(&str, String, String)>| -> Vec<(String, String)> {
            facts.into_iter().map(|(l, v, _)| (l.into(), v)).collect()
        };
        let live = shown(facts(&ticket, 0.0, Some(&estimate), Some(1_000), 301_000));
        assert!(
            live.contains(&("Est. spend".into(), "~$0.42".into())),
            "{live:?}"
        );
        assert!(live.contains(&("Elapsed".into(), "5m".into())), "{live:?}");
        assert!(
            live.contains(&("Goal".into(), "Ship the menus".into())),
            "{live:?}"
        );
        assert!(!live.iter().any(|(l, _)| l == "Quote"), "{live:?}");
        // A run paused on an ask is still the current run.
        assert!(is_live("asking"));
        ticket.state = studio_core::ticket::TicketState::Queued;
        let queued = shown(facts(&ticket, 0.0, Some(&estimate), None, 0));
        assert!(
            queued.contains(&("Spent".into(), "$0.00".into())),
            "{queued:?}"
        );
        assert!(
            queued.contains(&("Quote".into(), "~$0.42".into())),
            "{queued:?}"
        );
    }

    #[tokio::test]
    async fn agent_text_is_escaped() {
        let entries = group_lines(vec![Line::new(
            LineKind::Text,
            "<script>alert(1)</script>".into(),
        )]);
        let cx = Cx::default();
        let html = ui::html(&cx, view! { cx => log(entries: entries) })
            .await
            .unwrap();
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[tokio::test]
    async fn reads_only_complete_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "one\ntwo\npart").unwrap();
        let (text, next) = read_from(&path, 0).await;
        assert_eq!(text, "one\ntwo\n");
        assert_eq!(next, 8);
        std::fs::write(&path, "one\ntwo\npartial\n").unwrap();
        assert_eq!(read_from(&path, next).await, ("partial\n".into(), 16));
    }
}
