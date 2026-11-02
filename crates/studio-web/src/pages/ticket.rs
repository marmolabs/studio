//! One ticket: what it asks for, what waits for the owner, everything that
//! happened on it, and the facts beside it. Live while it is open.

use futures_util::{Stream, StreamExt, stream};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use studio_config::draft::from_feedback;
use studio_core::machine::MAX_ATTEMPTS;
use studio_core::ticket::{FailureClass, Ticket, TicketKind, TicketState};
use studio_git::Repo;
use studiod::api::{AskView, Request};
use studiod::export::{self, Export};
use topcoat::{
    context::{Cx, app_context},
    datastar::{ElementPatchMode, PatchElements},
    router::{
        Body,
        content::{
            Form,
            sse::{Event, KeepAlive, Sse},
        },
        page, path_param, query_params,
        response::Response,
        route,
    },
    view::{Unescaped, View, component, view},
};

use crate::summary::truncate;
use crate::{AppConfig, client, diffstat, live, markdown, ui};

use super::activity::{self, Filer, Logged};
use super::ask_card::ask_card;
use super::timeline::{exact, relative};
use super::{changes, evidence};

path_param!(id);

#[query_params(error = bad_request)]
struct LogFrom {
    from: Option<u64>,
}

/// How often, in stream ticks, the changed files are looked at again.
const FILES_EVERY: u32 = 10;
/// Changed files listed in the rail before the rest are counted.
const FILES_SHOWN: usize = 12;

/// What the page shows about a ticket, fetched together.
struct Facts {
    ticket: Ticket,
    filer: Filer,
    asks: Vec<AskView>,
    events: Vec<client::Stamped>,
    spent: f64,
    /// The studio's cap on what one ticket may spend. A hire's own cap
    /// covers only that hire's runs on it, so it isn't shown against the
    /// whole ticket's spend.
    cap: Option<f64>,
    /// Whether the ticket's goal branch is already merged into its base.
    merged: bool,
}

async fn facts(config: &AppConfig, id: &str) -> topcoat::Result<Facts> {
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    let ticket = tickets
        .iter()
        .find(|t| t.id.to_string() == id)
        .cloned()
        .ok_or_else(|| client::error(format!("no ticket {id}")))?;
    let asks: Vec<AskView> = client::call(&config.socket, Request::Asks)
        .await
        .unwrap_or_default();
    let budget: Value = client::call(&config.socket, Request::Budget)
        .await
        .unwrap_or_default();
    Ok(Facts {
        filer: activity::filer(&ticket, &tickets),
        asks: asks.into_iter().filter(|a| a.ticket == id).collect(),
        events: ticket_events(&config.socket, id).await,
        spent: spent(&config.socket, id).await,
        cap: budget["limits"]["ticket_usd"].as_f64(),
        merged: mergeable(&ticket) && merged_ids(&config.socket).await.iter().any(|m| m == id),
        ticket,
    })
}

/// When the latest run began: the last time the ticket was claimed.
fn run_started(events: &[client::Stamped]) -> Option<u64> {
    events.iter().rev().find(|e| claimed(e)).map(|e| e.at_ms)
}

/// Every run begins with a claim, whatever became of the one before it.
fn claimed(event: &client::Stamped) -> bool {
    matches!(event.event, studio_core::event::Event::Claimed)
}

/// Whether the ticket has a goal branch to merge: a plan lands without one.
fn mergeable(ticket: &Ticket) -> bool {
    ticket.state.kind() == "landed" && ticket.kind != TicketKind::Plan
}

/// What the ticket changed; nothing once the worktree is gone.
#[derive(Clone, Debug, Default)]
struct Changed {
    /// Each file as (path, added, removed), from the diff, which studiod
    /// cuts at 1 MB.
    files: Vec<(String, u64, u64)>,
    /// Lines added and removed in all, from `git diff --stat`, which is exact.
    added: u64,
    removed: u64,
}

/// The ticket's changes, from `diff` if the caller has it.
async fn changed(socket: &Path, id: &str, diff: Option<&str>) -> Changed {
    let files = diff.map(changes::stats).unwrap_or_default();
    let stat: Option<String> = client::call(socket, Request::Diffstat { ticket: id.into() })
        .await
        .unwrap_or_default();
    let (added, removed) = stat
        .as_deref()
        .and_then(diffstat::totals)
        .unwrap_or_else(|| {
            files
                .iter()
                .fold((0, 0), |(a, r), (_, x, y)| (a + x, r + y))
        });
    Changed {
        files,
        added,
        removed,
    }
}

async fn changed_files(socket: &Path, id: &str) -> Changed {
    let diff: Option<String> = client::call(socket, Request::Diff { ticket: id.into() })
        .await
        .unwrap_or_default();
    changed(socket, id, diff.as_deref()).await
}

/// The page's live parts, each with its own id: the head, the asks, the
/// outcome, the activity, the rail and the top bar's actions.
async fn fragments(
    cx: &Cx,
    facts: &Facts,
    log: &str,
    changes: &Changed,
) -> topcoat::Result<[String; 6]> {
    let ticket = facts.ticket.clone();
    let kind = ticket.state.kind();
    let feed = activity::feed(&ticket, &facts.filer, &facts.events, log);
    let logged = activity::parse_log(log);
    let since = run_started(&facts.events).unwrap_or(0);
    let side = Side {
        model: feed.model.clone(),
        branch: branch(&ticket, &facts.events),
        tries: tries(&ticket, &facts.events),
        time: time_taken(&ticket, &facts.events, ui::now_ms()),
        spent: facts.spent,
        cap: facts.cap,
        stages: stages(&ticket, &logged, since),
        files: changes.files.clone(),
    };
    let unreviewed = logged
        .iter()
        .filter(|(at, _)| *at >= since)
        .any(|(_, l)| matches!(l, Logged::Note(n) if n.starts_with("no reviewer")));
    let (added, removed) = (changes.added, changes.removed);
    let project = ticket.project.to_string();
    let quote = activity::last_words(log);
    Ok([
        ui::html(cx, view! { cx => head(ticket: facts.ticket.clone(), filer: facts.filer.clone()) })
            .await?,
        ui::html(
            cx,
            view! { cx => ticket_asks(asks: facts.asks.clone(), project: project, quote: quote) },
        )
        .await?,
        ui::html(
            cx,
            view! { cx => outcome(ticket: facts.ticket.clone(), merged: facts.merged, reviewed: !unreviewed, added: added, removed: removed) },
        )
        .await?,
        ui::html(
            cx,
            view! { cx => activity::activity(items: feed.items, state: kind, live: ui::run_under_way(kind)) },
        )
        .await?,
        ui::html(cx, view! { cx => rail(ticket: facts.ticket.clone(), side: side) }).await?,
        ui::html(cx, view! { cx => actions(ticket: facts.ticket.clone()) }).await?,
    ])
}

#[page("/tickets/{id}")]
async fn ticket_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let facts = facts(config, &id).await?;
    let (log, end) = read_from(&log_path(&config.data_dir, &id), 0).await;
    let kind = facts.ticket.state.kind();
    // Fetched once: a diff is too big to send again every second.
    let diff: Option<String> = client::call(&config.socket, Request::Diff { ticket: id.clone() })
        .await
        .unwrap_or_default();
    let files = changed(&config.socket, &id, diff.as_deref()).await;
    let captures = evidence::files(&config.socket, &id).await;
    let [
        head_html,
        asks_html,
        outcome_html,
        feed_html,
        rail_html,
        actions_html,
    ] = fragments(cx, &facts, &log, &files).await?;
    let stream = format!("@get('/tickets/{id}/stream?from={end}')");
    Ok(view! {
        <div data-init=(stream) class="grid grid-cols-[minmax(0,1fr)_272px] items-start gap-14 max-[860px]:grid-cols-1 max-[860px]:gap-8">
            <div class="flex min-w-0 flex-col gap-6">
                (Unescaped::new_unchecked(head_html))
                (Unescaped::new_unchecked(asks_html))
                (Unescaped::new_unchecked(outcome_html))
                evidence::art_pairs(ticket: id.clone(), files: captures.clone())
                evidence::screenshots(ticket: id.clone(), files: captures)
                if kind == "ready" {
                    changes::changes(diff: diff)
                }
                (Unescaped::new_unchecked(feed_html))
            </div>
            (Unescaped::new_unchecked(rail_html))
        </div>
        (Unescaped::new_unchecked(actions_html))
    })
}

/// What the stream keeps between ticks.
struct Live {
    /// The fragments last sent.
    sent: [String; 6],
    /// Where the next read of the run log starts, and the log so far.
    pos: u64,
    log: String,
    files: Changed,
    tick: u32,
    /// What the fragments were last built from, so they are built again only
    /// when something changed.
    built_from: String,
}

#[route(GET "/tickets/{id}/stream")]
async fn ticket_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let id = path_param::<Id>(cx).to_owned();
    let from = query_params::<LogFrom>(cx)?.from.unwrap_or(0);
    let cx = cx.clone();
    // The log the page already shows; the feed is built again from all of it
    // whenever it grows, since new lines may join its last group.
    let (log, _) = read_from(&log_path(&config.data_dir, &id), 0).await;
    let log = log.get(..from as usize).unwrap_or_default().to_string();
    let files = changed_files(&config.socket, &id).await;
    let (sent, from_key) = match facts(&config, &id).await {
        Ok(facts) => (
            fragments(&cx, &facts, &log, &files)
                .await
                .unwrap_or_default(),
            built_from(&facts, &files, log.len()),
        ),
        Err(_) => Default::default(),
    };
    let state = Live {
        sent,
        pos: from,
        log,
        files,
        tick: 0,
        built_from: from_key,
    };
    let events = stream::unfold(state, move |mut state| {
        let (config, id, cx) = (config.clone(), id.clone(), cx.clone());
        async move {
            tokio::time::sleep(live::EVERY).await;
            let (text, pos) = read_from(&log_path(&config.data_dir, &id), state.pos).await;
            state.log.push_str(&text);
            state.pos = pos;
            state.tick += 1;
            if state.tick % FILES_EVERY == 0 {
                state.files = changed_files(&config.socket, &id).await;
            }
            let mut events: Vec<Event> = Vec::new();
            let Ok(facts) = facts(&config, &id).await else {
                return Some((events, state));
            };
            // Parsing a long log every second is costly, so the parts are
            // built again only when the log, the facts or the minute moved.
            let key = built_from(&facts, &state.files, state.log.len());
            if key == state.built_from {
                return Some((events, state));
            }
            state.built_from = key;
            if let Ok(parts) = fragments(&cx, &facts, &state.log, &state.files).await {
                for (html, sent) in parts.into_iter().zip(state.sent.iter_mut()) {
                    if html != *sent {
                        sent.clone_from(&html);
                        events.push(PatchElements::new(html).into());
                    }
                }
            }
            Some((events, state))
        }
    })
    .flat_map(|events: Vec<Event>| stream::iter(events.into_iter().map(Ok)));
    Ok(Sse::new(events).keep_alive(KeepAlive::new()))
}

/// What the page's parts depend on: the facts, the changes, how much log
/// there is, and the clock (to the second while a run counts its time, else
/// to the minute, for times such as "3m ago").
fn built_from(facts: &Facts, files: &Changed, log_len: usize) -> String {
    let now = ui::now_ms();
    let clock = if ui::run_under_way(facts.ticket.state.kind()) {
        now / 1000
    } else {
        now / 60_000
    };
    format!(
        "{:?} {:?} {:?} {} {:?} {} {:?} {} {:?} {log_len} {clock}",
        facts.ticket,
        facts.filer,
        facts.asks,
        facts.events.len(),
        facts.events.last().map(|e| e.at_ms),
        facts.spent,
        facts.cap,
        facts.merged,
        files,
    )
}

/// The ticket as Markdown, the same as `studio export` prints.
#[route(GET "/tickets/{id}/export")]
async fn export_markdown(cx: &Cx) -> topcoat::Result<Response> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let tickets: Vec<Value> = client::call(&config.socket, Request::Tickets).await?;
    let Some(ticket) = tickets.iter().find(|t| t["id"] == id) else {
        return Ok(http::Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body(Body::from(format!("no ticket {id}")))?);
    };
    let events: Value = client::call(
        &config.socket,
        Request::Events {
            since: 0,
            limit: 10_000,
            ticket: Some(id.clone()),
        },
    )
    .await?;
    let diffstat: Option<String> =
        client::call(&config.socket, Request::Diffstat { ticket: id.clone() })
            .await
            .unwrap_or_default();
    let evidence: Value = client::call(&config.socket, Request::Evidence { ticket: id.clone() })
        .await
        .unwrap_or_default();
    let (log, _) = read_from(&log_path(&config.data_dir, &id), 0).await;
    let summary = export::summary(&log);
    let text = export::render(&Export {
        ticket,
        spend_usd: spent(&config.socket, &id).await,
        events: &events,
        summary: summary.as_deref(),
        diffstat: diffstat.as_deref(),
        evidence: &evidence,
    });
    let name: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, "text/markdown; charset=utf-8")
        .header(
            http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}.md\""),
        )
        .header("x-content-type-options", "nosniff")
        .body(Body::from(text))?)
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
            <p class=(ui::SECONDARY)>
                "Landed on " <span class="font-mono">(branch)</span> ". Merge it into "
                <span class="font-mono">(base.clone())</span> " when you're happy."
            </p>
            merge_button(ticket: id, base: base, merged: false)
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

/// Ids of the tickets whose goal branch is merged; none if the daemon can't say.
pub async fn merged_ids(socket: &Path) -> Vec<String> {
    client::call(socket, Request::MergedTickets)
        .await
        .unwrap_or_default()
}

/// The button, and what it does, shared by the ticket page and the goals; a
/// quiet label once the goal is merged.
#[component]
pub async fn merge_button(
    ticket: String,
    base: String,
    merged: bool,
) -> topcoat::Result<impl View> {
    let post = format!("@post('/tickets/{ticket}/merge')");
    Ok(view! {
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
            if merged {
                <span class=(format!("text-[12.5px] {}", ui::MUTED))>"Merged into " (base) "."</span>
            } else {
                <button class=(ui::BUTTON) data-on:click=(post)>
                    ui::icon(name: "git-merge")
                    "Merge into " (base)
                </button>
                <span class=(format!("text-[12.5px] {}", ui::MUTED))>"Merges in your checkout; nothing is pushed."</span>
            }
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

#[route(POST "/tickets/{id}/pause")]
async fn pause(cx: &Cx) -> topcoat::Result<PatchElements> {
    let done = "Pausing: the agent stops, and the ticket waits until you resume it.";
    act(cx, |ticket| Request::Pause { ticket }, done).await
}

/// Runs an action and reports it; the stream updates the page.
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

/// The branch the ticket's work is on: its own while open, its goal's once
/// landed. A plan has none, and neither has a ticket that never ran.
fn branch(ticket: &Ticket, events: &[client::Stamped]) -> Option<String> {
    if ticket.kind == TicketKind::Plan {
        return None;
    }
    match ticket.state.kind() {
        "landed" => Some(activity::goal_branch(ticket)),
        "cancelled" => None,
        _ => events
            .iter()
            .any(claimed)
            .then(|| Repo::ticket_branch_name(&ticket.id)),
    }
}

/// "1 of 3": which try this is, once the ticket has run.
fn tries(ticket: &Ticket, events: &[client::Stamped]) -> Option<String> {
    let attempt = (ticket.attempts + 1).min(MAX_ATTEMPTS);
    events
        .iter()
        .any(claimed)
        .then(|| format!("{attempt} of {MAX_ATTEMPTS}"))
}

/// How long the ticket has worked: the current run so far, or from its first
/// run to its last event once it stopped. A landed ticket stopped working
/// when it passed review, not when the owner landed it.
fn time_taken(ticket: &Ticket, events: &[client::Stamped], now: u64) -> Option<String> {
    let kind = ticket.state.kind();
    if ui::run_under_way(kind) {
        return run_started(events).map(|start| ui::elapsed(now.saturating_sub(start)));
    }
    if !matches!(kind, "ready" | "landed" | "failed" | "cancelled") {
        return None;
    }
    let first = events.iter().find(|e| claimed(e))?.at_ms;
    let passed = |e: &&client::Stamped| matches!(e.event, studio_core::event::Event::ReviewPassed);
    let last = match events.iter().rev().find(passed) {
        Some(event) if kind == "landed" => event,
        _ => events.last()?,
    };
    Some(ui::elapsed(last.at_ms.saturating_sub(first)))
}

/// One stage on a ticket's way to landing.
#[derive(Clone, Debug, PartialEq)]
struct Stage {
    name: String,
    mark: Mark,
    /// "waiting for you", why it is held, or "failed".
    note: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    Done,
    Now,
    Next,
    Failed,
}

/// The stages this ticket goes through: work, checks, review, each gate the
/// log shows for it, and landing. They are marked from the ticket's state and
/// from the log since `since`, when its latest run began.
fn stages(ticket: &Ticket, log: &[(u64, Logged)], since: u64) -> Vec<Stage> {
    let notes = |from: u64| {
        log.iter()
            .filter(move |(at, _)| *at >= from)
            .filter_map(|(_, l)| match l {
                Logged::Note(note) => Some(note.as_str()),
                _ => None,
            })
    };
    let mut names: Vec<String> = vec!["Work".into()];
    if ticket.kind == TicketKind::Plan {
        names.push("Done".into());
    } else {
        names.extend(["Checks".into(), "Review".into()]);
        for gate in notes(0).filter_map(activity::gate_of) {
            let name = gate_stage(gate);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.push(match activity::goal_name(ticket) {
            Some(goal) => format!("Land on {}", truncate(&goal, 28)),
            None => "Land".into(),
        });
    }
    let last = names.len() - 1;
    // How far review got in the latest run: the reviewer, then each gate.
    let reviewed_to = || {
        let mut at = 2;
        for note in notes(since) {
            if note.starts_with("review by ") {
                at = 2;
            } else if note == "review passed" || note.starts_with("no reviewer") {
                at = 3;
            } else if let Some(gate) = activity::gate_of(note) {
                let name = gate_stage(gate);
                let index = names.iter().position(|n| *n == name).unwrap_or(2);
                let rest = note.strip_prefix("gate ").unwrap_or_default();
                let rest = rest.strip_prefix(gate).unwrap_or_default();
                let past = rest.starts_with(" passed") || rest.starts_with(" skipped");
                at = if past { index + 1 } else { index };
            }
        }
        at.min(last)
    };
    let place = |kind: &str| match kind {
        "checking" => 1.min(last),
        "reviewing" => reviewed_to(),
        _ => 0,
    };
    let from = ticket
        .held_from
        .as_ref()
        .map_or("working", TicketState::kind);
    let waiting = "waiting for you".to_string();
    let (now, mark, note) = match &ticket.state {
        TicketState::Queued | TicketState::Cancelled => (None, Mark::Next, String::new()),
        TicketState::Landed => (Some(names.len()), Mark::Done, String::new()),
        TicketState::Ready => (Some(last), Mark::Now, waiting),
        TicketState::Asking { .. } => (Some(place(from)), Mark::Now, waiting),
        state @ TicketState::Held { .. } => (
            Some(place(from)),
            Mark::Now,
            ui::state_detail(state).unwrap_or_default(),
        ),
        TicketState::Failed { class, .. } => {
            let at = match class {
                FailureClass::Checks => 1.min(last),
                FailureClass::Review => reviewed_to(),
                _ => 0,
            };
            (Some(at), Mark::Failed, "failed".to_string())
        }
        state => (Some(place(state.kind())), Mark::Now, String::new()),
    };
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let (mark, note) = match now {
                Some(n) if i < n => (Mark::Done, String::new()),
                Some(n) if i == n => (mark, note.clone()),
                _ => (Mark::Next, String::new()),
            };
            Stage { name, mark, note }
        })
        .collect()
}

/// "Design gate" for the gate named `design`.
fn gate_stage(gate: &str) -> String {
    let mut chars = gate.chars();
    let first: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("{first}{} gate", chars.as_str())
}

/// Where a work ticket's goal is in the office: its plan ticket's panel.
fn goal_link(ticket: &Ticket) -> Option<String> {
    let plan = ticket.parent.as_ref()?;
    Some(format!(
        "/?sel={}",
        super::office::key("goals", &plan.to_string())
    ))
}

/// The title, who filed the ticket and from which goal, and what it asks for.
#[component]
async fn head(ticket: Ticket, filer: Filer) -> topcoat::Result<impl View> {
    let goal = activity::goal_name(&ticket)
        .filter(|_| ticket.kind == TicketKind::Work)
        .map(|_| ticket.goal.clone());
    let link = goal_link(&ticket);
    let (hire, words) = match filer {
        Filer::Hire(hire) => (Some(hire), "filed this"),
        Filer::Studio(source) if source.starts_with("ci:") => {
            (None, "The studio filed this after a CI run failed")
        }
        Filer::Studio(source) if source.starts_with("merge:") => (
            None,
            "The studio filed this when merging the goal conflicted",
        ),
        Filer::Studio(_) => (None, "The studio filed this"),
        Filer::Owner if ticket.kind == TicketKind::Plan => (None, "You set this goal"),
        Filer::Owner => (None, "You filed this"),
    };
    let created = ticket.created_at;
    Ok(view! {
        <header id="ticket-head" class="flex flex-col gap-6">
            <div class="flex flex-col gap-2.5">
                <h1 class=(ui::PAGE_TITLE)>(ticket.title)</h1>
                <div class="flex flex-wrap items-center gap-1.5 text-[13px] text-fg-3">
                    if let Some(hire) = hire {
                        <span class="text-fg-2">ui::who(hire: hire)</span>
                    }
                    // The time stays with what comes before it, so no line
                    // starts with its dot.
                    if let Some(goal) = goal {
                        <span>(words) " from"</span>
                        <span class="flex min-w-0 max-w-full items-center gap-1.5 whitespace-nowrap">
                            if let Some(href) = link {
                                <a href=(href) class="min-w-0 hover:[&>span]:border-line-3">ui::goal_chip(goal: goal)</a>
                            } else {
                                <span class="min-w-0">ui::goal_chip(goal: goal)</span>
                            }
                            <span title=(exact(created))>"· " (relative(ui::now_ms(), created))</span>
                        </span>
                    } else {
                        <span>
                            (words) " "
                            <span class="whitespace-nowrap" title=(exact(created))>"· " (relative(ui::now_ms(), created))</span>
                        </span>
                    }
                </div>
            </div>
            if !ticket.detail.trim().is_empty() {
                <div class=(format!("max-w-[68ch] leading-[22px] text-fg-2 {}", markdown::STYLE))>
                    (markdown::render(&ticket.detail))
                </div>
            }
            if let Some(feedback) = ticket.feedback {
                <div class="flex flex-col gap-1.5">
                    <p class="text-[12.5px] font-medium text-fg-3">"Your notes when you sent it back"</p>
                    <div class=(format!("max-w-[68ch] border-l-2 border-line-2 pl-3 leading-[22px] text-fg-2 {}", markdown::STYLE))>
                        (markdown::render(&feedback))
                    </div>
                </div>
            }
        </header>
    })
}

/// The asks waiting on this ticket. The newest gets what the agent said
/// just before it asked.
#[component]
async fn ticket_asks(
    asks: Vec<AskView>,
    project: String,
    quote: Option<String>,
) -> topcoat::Result<impl View> {
    let newest = asks.iter().map(|a| a.at_ms).max();
    Ok(view! {
        <div id="ticket-asks" class="flex flex-col gap-4 empty:hidden">
            #[key(ask.id.clone())]
            for ask in asks {
                let said = quote
                    .clone()
                    .filter(|_| Some(ask.at_ms) == newest && matches!(ask.kind.as_str(), "permission" | "question"));
                ask_card(ask: ask, project: project.clone(), quote: said)
            }
        </div>
    })
}

const OUTCOME: &str = "flex flex-col gap-3.5 rounded-[10px] border border-line bg-bg-2 px-4 py-3.5";
/// A goal branch in a line of text: `ui::CODE`, cut to the room it has.
const BRANCH: &str = "mx-0.5 inline-block min-w-0 max-w-full truncate rounded-[5px] border border-line bg-bg-3 px-[5px] align-bottom font-mono text-[12px] leading-[18px] text-fg";

/// Where a ticket that stopped stands, and what can still be done with it.
#[component]
async fn outcome(
    ticket: Ticket,
    merged: bool,
    reviewed: bool,
    added: u64,
    removed: u64,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let kind = ticket.state.kind();
    let [on_land, on_retry, on_resume] =
        ["land", "retry", "resume"].map(|a| format!("@post('/tickets/{id}/{a}')"));
    let send_back = format!("@post('/tickets/{id}/changes', {{contentType: 'form'}})");
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    let (failed, failed_why) = ui::failure(&ticket.state).unwrap_or_default();
    let plan = ticket.kind == TicketKind::Plan;
    let goal_branch = activity::goal_branch(&ticket);
    let passed = if reviewed {
        "Passed its checks and review."
    } else {
        "Passed its checks. No reviewer runs on another model, so review it yourself."
    };
    Ok(view! {
        <div id="ticket-outcome" class="empty:hidden">
            if kind == "ready" {
                <section class=(OUTCOME) aria-label="Ready to land">
                    <div class="flex flex-wrap items-center gap-2.5">
                        ui::state_icon(state: "ready")
                        <span class="font-medium">"Ready to land"</span>
                        if added + removed > 0 {
                            <span class="text-[12.5px]">ui::diff_size(added: added, removed: removed)</span>
                        }
                    </div>
                    <p class=(ui::SECONDARY)>(passed) " It lands on " <span class=(BRANCH) title=(goal_branch.clone())>(goal_branch.clone())</span> "."</p>
                    <div class=(ui::ASK_ACTIONS)>
                        <button type="button" class=(ui::PRIMARY) data-on:click=(on_land)>
                            ui::icon(name: "git-merge")
                            "Land"
                        </button>
                        <details id="send-back" data-preserve-attr="open" class="group [&[open]]:basis-full">
                            <summary class=(format!("{} list-none [&::-webkit-details-marker]:hidden", ui::BUTTON))>
                                ui::icon(name: "corner-down-right")
                                "Send back"
                            </summary>
                            <form data-on:submit=(send_back) class="mt-3 flex flex-col gap-2.5">
                                <textarea name="feedback" required="" rows="3" aria-label="What should change" placeholder="What should change?" class=(ui::TEXTAREA)></textarea>
                                <div><button type="submit" class=(ui::BUTTON)>"Send back with these notes"</button></div>
                            </form>
                        </details>
                    </div>
                </section>
            } else if kind == "landed" {
                <section class=(OUTCOME) aria-label="Landed">
                    <div class="flex flex-wrap items-center gap-2.5">
                        ui::state_icon(state: "landed")
                        if plan {
                            <span class="font-medium">"Done"</span>
                            <span class=(ui::SECONDARY)>"The plan is filed; its tickets are in the office."</span>
                        } else {
                            <span class="font-medium">"Landed on "</span>
                            <span class=(BRANCH) title=(goal_branch.clone())>(goal_branch)</span>
                        }
                    </div>
                    if !plan {
                        merge_button(ticket: id.clone(), base: ticket.base_branch.clone(), merged: merged)
                    }
                </section>
            } else if kind == "failed" {
                <section class=(OUTCOME) aria-label="Failed">
                    <div class="flex flex-wrap items-center gap-2.5">
                        ui::state_icon(state: "failed")
                        <span class="font-medium">(failed)</span>
                        if !failed_why.is_empty() {
                            <span class="min-w-0 text-fg-2 [overflow-wrap:anywhere]">(failed_why)</span>
                        }
                    </div>
                    <div class=(ui::ASK_ACTIONS)>
                        <button type="button" class=(ui::PRIMARY) data-on:click=(on_retry)>
                            ui::icon(name: "refresh-cw")
                            "Try again"
                        </button>
                    </div>
                </section>
            } else if kind == "held" {
                <section class=(OUTCOME) aria-label="Held">
                    <div class="flex flex-wrap items-center gap-2.5">
                        ui::state_icon(state: ui::shown(&ticket.state))
                        <span class="font-medium">"Held"</span>
                        <span class="text-fg-2">(why)</span>
                    </div>
                    <div class=(ui::ASK_ACTIONS)>
                        <button type="button" class=(ui::PRIMARY) data-on:click=(on_resume)>
                            ui::icon(name: "circle-play")
                            "Resume"
                        </button>
                    </div>
                </section>
            } else if kind == "cancelled" {
                <section class=(OUTCOME) aria-label="Cancelled">
                    <div class="flex flex-wrap items-center gap-2.5">
                        ui::state_icon(state: "cancelled")
                        <span class="font-medium">"Cancelled"</span>
                        <span class=(ui::SECONDARY)>"Nothing more runs on it."</span>
                    </div>
                </section>
            }
        </div>
    })
}

/// What the rail shows besides the ticket itself.
struct Side {
    model: Option<String>,
    branch: Option<String>,
    tries: Option<String>,
    time: Option<String>,
    spent: f64,
    cap: Option<f64>,
    stages: Vec<Stage>,
    files: Vec<(String, u64, u64)>,
}

fn stage_ring(stage: &Stage) -> &'static str {
    match stage.mark {
        Mark::Done => "m-0.5 size-3 rounded-full border-[1.5px] border-ok bg-ok",
        Mark::Failed => "m-0.5 size-3 rounded-full border-[1.5px] border-bad bg-bad",
        Mark::Now if !stage.note.is_empty() => {
            "m-0.5 size-3 rounded-full border-[1.5px] border-attn bg-attn shadow-[0_0_0_3px_color-mix(in_srgb,var(--attn)_22%,transparent)]"
        }
        Mark::Now => {
            "m-0.5 size-3 rounded-full border-[1.5px] border-run bg-run shadow-[0_0_0_3px_color-mix(in_srgb,var(--run)_22%,transparent)]"
        }
        Mark::Next => "m-0.5 size-3 rounded-full border-[1.5px] border-line-3",
    }
}

const STAGE: &str = "grid min-h-[30px] grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2.5";
const RAIL_HEAD: &str = "mb-2 text-[12px] font-medium leading-4 text-fg-3";

/// The facts beside the page: status, who, where, spend and time; the
/// stages; the changed files.
#[component]
async fn rail(ticket: Ticket, side: Side) -> topcoat::Result<impl View> {
    let Side {
        model,
        branch,
        tries,
        time,
        spent,
        cap,
        stages,
        mut files,
    } = side;
    let goal = activity::goal_name(&ticket).map(|_| ticket.goal.clone());
    let link = goal_link(&ticket);
    let more = files.len().saturating_sub(FILES_SHOWN);
    files.truncate(FILES_SHOWN);
    Ok(view! {
        <div id="ticket-rail" class="min-w-0 self-stretch">
            ui::rail(
                <dl aria-label="Ticket details">
                    ui::prop(label: "Status", ui::state_pill(state: ui::shown(&ticket.state)))
                    ui::prop(label: "Hire",
                        ui::who(hire: ticket.hire.to_string())
                        if let Some(model) = model { ui::model_tag(model: model) }
                    )
                    if let Some(goal) = goal {
                        ui::prop(label: "Goal",
                            if let Some(href) = link {
                                <a href=(href) class="min-w-0">ui::goal_chip(goal: goal)</a>
                            } else {
                                <span class="min-w-0">ui::goal_chip(goal: goal)</span>
                            }
                        )
                    }
                    ui::prop(label: "Project", <span class="truncate">(ticket.project.to_string())</span>)
                    if let Some(branch) = branch {
                        ui::prop(label: "Branch",
                            <span class="text-fg-3">ui::icon(name: "git-branch", small: true)</span>
                            <span class=(format!("truncate {}", ui::MONO)) title=(branch.clone())>(branch)</span>
                        )
                    }
                    if let Some(tries) = tries {
                        ui::prop(label: "Try", <span class="tabular-nums">(tries)</span>)
                    }
                    ui::prop(label: "Spent",
                        <div class="flex w-full flex-col gap-1.5">
                            <span class="tabular-nums">
                                (ui::usd(spent))
                                if let Some(cap) = cap { <span class=(ui::MUTED)>" of " (ui::limit(cap))</span> }
                            </span>
                            if let Some(cap) = cap { ui::meter(value: spent, max: cap) }
                        </div>
                    )
                    if let Some(time) = time {
                        ui::prop(label: "Time", <span class="tabular-nums">(time)</span>)
                    }
                </dl>
                <div class="h-px bg-line"></div>
                <section aria-label="Progress">
                    <h3 class=(RAIL_HEAD)>"Progress"</h3>
                    <ol class="flex flex-col">
                        for stage in stages {
                            <li class=(match stage.mark {
                                Mark::Now => format!("{STAGE} font-medium text-fg"),
                                Mark::Done => format!("{STAGE} text-fg-2"),
                                Mark::Failed => format!("{STAGE} font-medium text-bad-fg"),
                                Mark::Next => format!("{STAGE} text-fg-3"),
                            })>
                                <span class=(stage_ring(&stage))></span>
                                <span class="truncate">(stage.name)</span>
                                <span class="text-[12.5px] font-normal text-fg-3">(stage.note)</span>
                            </li>
                        }
                    </ol>
                </section>
                if !files.is_empty() {
                    <div class="h-px bg-line"></div>
                    <section aria-label="Changes">
                        <h3 class=(RAIL_HEAD)>"Changes"</h3>
                        <ul class="flex flex-col gap-1.5">
                            for (path, added, removed) in files {
                                <li class="flex items-center gap-2 text-[12.5px]">
                                    <span class="text-fg-3">ui::icon(name: "file-diff", small: true)</span>
                                    <span class="min-w-0 truncate font-mono text-[12px] text-fg-2" title=(path.clone())>(path)</span>
                                    <span class="ml-auto">ui::diff_size(added: added, removed: removed)</span>
                                </li>
                            }
                            if more > 0 {
                                <li class=(format!("text-[12.5px] {}", ui::MUTED))>"and " (more) " more"</li>
                            }
                        </ul>
                    </section>
                }
            )
        </div>
    })
}

/// The top bar's actions: the Markdown export, and a menu with Pause or
/// Resume and Cancel when they apply.
#[component]
async fn actions(ticket: Ticket) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let kind = ticket.state.kind();
    let can_pause = matches!(kind, "claimed" | "working" | "reviewing");
    let can_resume = kind == "held";
    let can_cancel = !matches!(kind, "landed" | "cancelled");
    let [on_pause, on_resume] = ["pause", "resume"].map(|a| format!("@post('/tickets/{id}/{a}')"));
    let confirm = format!(
        "confirm('Cancel {id}? Its run stops and it will not run again.') && @post('/tickets/{id}/cancel')"
    );
    Ok(view! {
        <div id="ticket-actions">
            ui::top_actions(
                <a href=(format!("/tickets/{id}/export")) download="" class=(ui::GHOST_SM) aria-label="Export as Markdown">
                    ui::icon(name: "file-text", small: true)
                    <span class=(ui::LABEL)>"Export as Markdown"</span>
                </a>
                if can_pause || can_resume || can_cancel {
                    ui::menu(id: "ticket-menu", label: "More actions",
                        if can_pause {
                            <button type="button" class=(ui::MENU_ITEM) popovertarget="ticket-menu" popovertargetaction="hide" data-on:click=(on_pause)>
                                ui::icon(name: "pause")
                                "Pause"
                            </button>
                        }
                        if can_resume {
                            <button type="button" class=(ui::MENU_ITEM) popovertarget="ticket-menu" popovertargetaction="hide" data-on:click=(on_resume)>
                                ui::icon(name: "circle-play")
                                "Resume"
                            </button>
                        }
                        if can_cancel {
                            <button type="button" class=(ui::MENU_ITEM_DANGER) popovertarget="ticket-menu" popovertargetaction="hide" data-on:click=(confirm)>
                                ui::icon(name: "circle-x")
                                "Cancel ticket"
                            </button>
                        }
                    )
                }
            )
        </div>
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
    use studio_core::event::Event;

    fn ticket(state: Value, extra: Value) -> Ticket {
        let mut base = serde_json::json!({
            "id": "GAME-2", "project": "game", "title": "t", "goal": "Enemy waves",
            "hire": "dev", "state": state, "attempts": 0, "base_branch": "main",
            "created_at": 0,
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value(base).unwrap()
    }

    fn marks(stages: &[Stage]) -> Vec<String> {
        stages
            .iter()
            .map(|s| {
                format!("{} {:?} {}", s.name, s.mark, s.note)
                    .trim()
                    .to_string()
            })
            .collect()
    }

    fn notes(texts: &[&str]) -> Vec<(u64, Logged)> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| (10 + i as u64, Logged::Note(t.to_string())))
            .collect()
    }

    #[test]
    fn stages_follow_the_ticket() {
        let none = serde_json::json!({});
        let working = ticket(serde_json::json!("working"), none.clone());
        assert_eq!(
            marks(&stages(&working, &[], 0)),
            [
                "Work Now",
                "Checks Next",
                "Review Next",
                "Land on Enemy waves Next"
            ]
        );
        // A gate that ran on an earlier try still shows, and the one
        // running now is current.
        let log = notes(&[
            "gate design by critic (opus)",
            "gate design failed: too wide",
            "review by reviewer (sonnet)",
            "review passed",
            "gate design: measured 2 file(s)",
            "gate design by critic (opus)",
        ]);
        let reviewing = ticket(serde_json::json!("reviewing"), none.clone());
        assert_eq!(
            marks(&stages(&reviewing, &log, 12)),
            [
                "Work Done",
                "Checks Done",
                "Review Done",
                "Design gate Now",
                "Land on Enemy waves Next"
            ]
        );
        let asking = ticket(
            serde_json::json!({"asking": {"ask": "a1"}}),
            serde_json::json!({"held_from": "reviewing"}),
        );
        assert_eq!(
            marks(&stages(&asking, &log[..3], 0))[2],
            "Review Now waiting for you"
        );
        let ready = ticket(serde_json::json!("ready"), none.clone());
        assert_eq!(
            marks(&stages(&ready, &[], 0))[3],
            "Land on Enemy waves Now waiting for you"
        );
        let failed = ticket(
            serde_json::json!({"failed": {"class": "checks", "detail": "boom"}}),
            none.clone(),
        );
        assert_eq!(
            marks(&stages(&failed, &[], 0))[..2],
            ["Work Done", "Checks Failed failed"]
        );
        let held = ticket(
            serde_json::json!({"held": {"reason": "paused"}}),
            serde_json::json!({"held_from": "checking"}),
        );
        assert_eq!(marks(&stages(&held, &[], 0))[1], "Checks Now paused");
        let landed = ticket(serde_json::json!("landed"), none.clone());
        assert!(stages(&landed, &[], 0).iter().all(|s| s.mark == Mark::Done));
        let plan = ticket(
            serde_json::json!("working"),
            serde_json::json!({"kind": "plan"}),
        );
        assert_eq!(marks(&stages(&plan, &[], 0)), ["Work Now", "Done Next"]);
    }

    fn stamped(at_ms: u64, event: Event) -> client::Stamped {
        client::Stamped { at_ms, event }
    }

    #[test]
    fn the_rail_counts_tries_and_time() {
        let never = ticket(serde_json::json!("queued"), serde_json::json!({}));
        assert_eq!(tries(&never, &[]), None);
        assert_eq!(branch(&never, &[]), None);
        let events = [
            stamped(0, Event::Claimed),
            stamped(240_000, Event::ReviewPassed),
            stamped(3_600_000, Event::Landed),
        ];
        let landed = ticket(serde_json::json!("landed"), serde_json::json!({}));
        assert_eq!(tries(&landed, &events).as_deref(), Some("1 of 3"));
        // The wait for the owner to land it isn't work.
        assert_eq!(
            time_taken(&landed, &events, 9_000_000).as_deref(),
            Some("4m")
        );
        assert_eq!(
            branch(&landed, &events).as_deref(),
            Some("studio/goal/enemy-waves")
        );
        // The counter stops one short on the last try.
        let failed = ticket(
            serde_json::json!({"failed": {"class": "checks", "detail": ""}}),
            serde_json::json!({"attempts": 2}),
        );
        assert_eq!(tries(&failed, &events).as_deref(), Some("3 of 3"));
        let working = ticket(serde_json::json!("working"), serde_json::json!({}));
        let events = [stamped(60_000, Event::Claimed)];
        assert_eq!(
            time_taken(&working, &events, 180_000).as_deref(),
            Some("2m")
        );
        assert_eq!(branch(&working, &events).as_deref(), Some("studio/game-2"));
    }

    #[tokio::test]
    async fn detail_and_feedback_render_as_markdown() {
        let t = ticket(
            serde_json::json!("queued"),
            serde_json::json!({
                "detail": "Waves come from `rules.rs`",
                "feedback": "**Quit** needs a confirm\nkeep `Esc`",
                "parent": "GAME-1",
            }),
        );
        let cx = Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => head(ticket: t, filer: Filer::Hire("producer".into())) },
        )
        .await
        .unwrap();
        assert!(html.contains("<code>rules.rs</code>"), "{html}");
        assert!(html.contains("<strong>Quit</strong>"), "{html}");
        assert!(html.contains("<br />"), "{html}");
        assert!(html.contains(">producer"), "{html}");
        assert!(html.contains("filed this"), "{html}");
        // The goal leads to its plan's panel in the office.
        assert!(html.contains(r#"href="/?sel=goals/GAME-1""#), "{html}");
    }

    #[tokio::test]
    async fn a_goal_without_a_plan_ticket_is_not_a_link() {
        let t = ticket(serde_json::json!("queued"), serde_json::json!({}));
        let cx = Cx::default();
        let html = ui::html(&cx, view! { cx => head(ticket: t, filer: Filer::Owner) })
            .await
            .unwrap();
        assert!(html.contains("Enemy waves"), "{html}");
        assert!(!html.contains("sel=goals"), "{html}");
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
