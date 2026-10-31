//! The inspector beside the floor: what needs the owner, or the panel of
//! whatever is picked on the floor.

use serde_json::Value;
use std::collections::BTreeMap;
use studio_core::event::Event as TicketEvent;
use studio_core::machine::MAX_ATTEMPTS;
use studio_core::ticket::{Ticket, TicketKind};
use studio_git::Repo;
use studiod::api::{ApproverCall, AskView, Request};
use topcoat::context::Cx;
use topcoat::view::{BoxView, Child, View, ViewExt, component, view};

use super::floor::{self, Goal, Room, checks_since, room};
use super::{Office, ago, key, select};
use crate::pages::ask_card::{
    ALWAYS, DECISION, MESSAGE, always_prefix, opened, questions, reasons, summarise,
};
use crate::pages::{changes, hires};
use crate::summary::{Action, latest_action, mcp_words, tool_arg, truncate};
use crate::{AppConfig, client, diffstat, ui, words};

/// Changed files, each with its lines added and removed.
type Files = Vec<(String, u64, u64)>;

/// What a panel needs beyond the floor's own data, read for the selection.
#[derive(Clone, Debug, Default)]
pub struct Detail {
    /// The selected ticket's whole history.
    pub events: Vec<client::Stamped>,
    /// The selected ticket's run log, or its tail.
    pub log: String,
    /// What one ticket may spend, from the budget.
    pub cap: Option<f64>,
    /// The selected ticket's lines added and removed, and files changed.
    pub stat: Option<(u64, u64, u64)>,
    /// Each changed file of a ready ticket, with lines added and removed.
    pub files: Files,
    /// Spend over the last 24 hours by hire.
    pub by_hire: BTreeMap<String, f64>,
    /// No reviewer on another model read the selected ready ticket.
    pub owner_reviews: bool,
    /// The approver's latest calls, newest first, when it is picked.
    pub calls: Vec<ApproverCall>,
}

/// What a panel reads once and keeps while its stream runs: the budget's cap
/// on a ticket, and what a ready ticket changed, which stays put while it
/// waits. The latter is kept with when its checks passed, so a ticket sent
/// back and ready again is read again.
#[derive(Clone, Debug, Default)]
pub struct Memo {
    cap: Option<Option<f64>>,
    ready: Option<(u64, Files, bool)>,
    /// The approver's latest calls, with when they were read.
    calls: Option<(u64, Vec<ApproverCall>)>,
}

const SAID_SHOWN: usize = 280;
const STEPS_SHOWN: usize = 3;
const RECENT_SHOWN: usize = 5;
const ARG_SHOWN: usize = 120;
/// The approver's calls its panel lists. studiod reads every fresh run log
/// for them, so they are read again at most every `CALLS_EVERY` ms.
const CALLS_SHOWN: usize = 8;
const CALLS_EVERY: u64 = 30_000;
/// How much of a call's command or path shows; its title has the rest.
const CALL_WHAT: usize = 60;

pub async fn detail(config: &AppConfig, office: &Office, sel: &str, memo: &mut Memo) -> Detail {
    let socket = &config.socket;
    let budget = || async {
        client::call::<Value>(socket, Request::Budget)
            .await
            .unwrap_or_default()
    };
    match sel.split_once('/') {
        Some(("tickets", id)) if office.ticket(id).is_some() => {
            let events: Vec<client::Stamped> = client::call(
                socket,
                Request::Events {
                    since: 0,
                    limit: usize::MAX,
                    ticket: Some(id.into()),
                },
            )
            .await
            .unwrap_or_default();
            let cap = match memo.cap {
                Some(cap) => cap,
                None => *memo
                    .cap
                    .insert(budget().await["limits"]["ticket_usd"].as_f64()),
            };
            let ready = office.ticket(id).is_some_and(|t| room(t) == Room::Ready);
            let passed = events
                .iter()
                .rev()
                .find(|e: &&client::Stamped| matches!(e.event, TicketEvent::ChecksPassed))
                .map_or(0, |e| e.at_ms);
            let (stat, files, owner_reviews) = if ready {
                let (files, owner_reviews) = match memo.ready.clone() {
                    Some((at, files, owner_reviews)) if at == passed => (files, owner_reviews),
                    _ => {
                        let (files, owner_reviews) = ready_detail(config, id).await;
                        memo.ready = Some((passed, files.clone(), owner_reviews));
                        (files, owner_reviews)
                    }
                };
                let stat = office
                    .sizes
                    .get(id)
                    .map(|(added, removed)| (*added, *removed, files.len() as u64));
                (stat, files, owner_reviews)
            } else {
                memo.ready = None;
                let stat: Option<String> =
                    client::call(socket, Request::Diffstat { ticket: id.into() })
                        .await
                        .unwrap_or_default();
                let stat = stat.as_deref().and_then(|s| {
                    let (added, removed) = diffstat::totals(s)?;
                    Some((added, removed, diffstat::files(s)?))
                });
                (stat, Vec::new(), false)
            };
            Detail {
                events,
                log: super::log_tail(&config.data_dir, id).await,
                cap,
                stat,
                files,
                owner_reviews,
                ..Detail::default()
            }
        }
        Some(("hires", id)) => {
            let approver = office.member(id).is_some_and(|m| m.role == "approver");
            let calls = match &memo.calls {
                _ if !approver => Vec::new(),
                Some((at, calls)) if office.now.saturating_sub(*at) < CALLS_EVERY => calls.clone(),
                _ => {
                    let limit = CALLS_SHOWN;
                    let calls: Vec<ApproverCall> =
                        client::call(socket, Request::ApproverCalls { limit })
                            .await
                            .unwrap_or_default();
                    memo.calls = Some((office.now, calls.clone()));
                    calls
                }
            };
            Detail {
                by_hire: serde_json::from_value(budget().await["spent_24h"]["by_hire"].clone())
                    .unwrap_or_default(),
                calls,
                ..Detail::default()
            }
        }
        _ => Detail::default(),
    }
}

/// A ready ticket's changed files, and whether no reviewer on another model
/// read it: the run log's latest word on that since its checks passed.
async fn ready_detail(config: &AppConfig, id: &str) -> (Files, bool) {
    let diff: Option<String> = client::call(&config.socket, Request::Diff { ticket: id.into() })
        .await
        .unwrap_or_default();
    let owner_reviews = super::find_back(&config.data_dir, id, 0..u64::MAX, |raw| {
        let entry: Value = serde_json::from_str(raw).ok()?;
        let note = entry["note"].as_str()?;
        match note {
            "checks passed" => Some(false),
            _ if note.starts_with("no reviewer on a different model") => Some(true),
            _ => None,
        }
    })
    .await
    .unwrap_or(false);
    (
        diff.as_deref().map(changes::stats).unwrap_or_default(),
        owner_reviews,
    )
}

/// What the inspector shows for a selection.
enum Panel {
    Overview,
    Ticket(Box<Ticket>),
    Hire(client::Member),
    Goal(Box<Goal>),
}

fn panel(office: &Office, sel: &str) -> Panel {
    match sel.split_once('/') {
        Some(("tickets", id)) => office
            .ticket(id)
            .map_or(Panel::Overview, |t| Panel::Ticket(Box::new(t.clone()))),
        Some(("hires", id)) => office
            .member(id)
            .map_or(Panel::Overview, |m| Panel::Hire(m.clone())),
        Some(("goals", id)) => floor::goals(office)
            .into_iter()
            .find(|g| g.plan.id.to_string() == id)
            .map_or(Panel::Overview, |g| Panel::Goal(Box::new(g))),
        _ => Panel::Overview,
    }
}

/// As tall as it needs: beside the floor, the floor's row grows to match.
const INSPECTOR: &str = "flex min-w-0 flex-col border-l border-line bg-bg max-[1180px]:border-l-0 max-[1180px]:border-t";
const HEAD: &str = "sticky top-0 z-[1] flex min-h-[52px] items-center gap-2 border-b border-line bg-bg px-4 py-2.5";
const BODY: &str = "flex flex-col gap-[18px] p-4";
const SECTION_LABEL: &str = "mb-2 text-[12px] font-medium leading-4 text-fg-3";
const KV: &str = "m-0 grid grid-cols-[84px_minmax(0,1fr)] gap-x-2.5 gap-y-2 text-[13px] [&_dd]:m-0 [&_dd]:min-w-0 [&_dt]:text-fg-3";
const ACTIONS: &str = "flex flex-wrap gap-2";
const SUBTITLE: &str = "block truncate font-mono text-[12px] text-fg-3";
const PANEL_TITLE: &str =
    "text-[16px] font-semibold leading-[22px] tracking-[-0.01em] [text-wrap:balance]";
const DOING: &str = "flex min-w-0 items-center gap-2 rounded-[7px] border border-line bg-bg-2 px-2.5 py-[7px] text-[13px] leading-[18px] text-fg-2 [&>.i]:size-3.5 [&>.i]:text-fg-3";
const TOOL_ROW: &str = "grid grid-cols-[16px_auto_minmax(0,1fr)_auto] items-center gap-2.5 border-t border-line px-3 py-[7px] text-[12.5px] first:border-t-0 [&>.i]:size-3.5 [&>.i]:text-fg-3";
const TICKET_LINK: &str = "font-medium text-fg hover:underline";
const EVENT_ROW: &str = "grid grid-cols-[36px_16px_minmax(0,1fr)] items-start gap-2.5 border-t border-line py-2.5 text-[13px] leading-5 first:border-t-0 first:pt-0.5 [&>.st]:mt-0.5 [&>.prog]:mt-0.5";
/// A ticket in a list: its id, glyph and title.
const TICKET_ROW: &str = "grid grid-cols-[64px_16px_minmax(0,1fr)] items-start gap-2.5 border-t border-line py-2.5 text-[13px] leading-5 first:border-t-0 first:pt-0.5 hover:bg-bg-2 [&>.st]:mt-0.5 [&>.prog]:mt-0.5";
const BACK: &str = "$sel = ''; @get('/office/panel')";
/// A command or path on one line, for a small ask card.
const CMD_LINE: &str = "truncate rounded-lg border border-line bg-bg-2 px-2.5 py-[7px] font-mono text-[12px] leading-5 text-fg";

#[component]
pub async fn inspector(office: Office, detail: Detail, sel: String) -> topcoat::Result<impl View> {
    // Boxed, so the panels' futures live on the heap, not the stack.
    let body: BoxView = match panel(&office, &sel) {
        Panel::Overview => view! { overview(office: office) }.boxed(),
        Panel::Ticket(ticket) => ticket_panel(__cx, *ticket, office, detail),
        Panel::Hire(member) => {
            view! { hire_panel(member: member, office: office, detail: detail) }.boxed()
        }
        Panel::Goal(goal) => view! { goal_panel(goal: *goal, office: office) }.boxed(),
    };
    Ok(view! {
        <aside id="inspector" class=(INSPECTOR) aria-label="Inspector" aria-live="polite">
            (body)
        </aside>
    })
}

/// The panel's header: a way back, then `child`.
#[component]
async fn head(child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class=(HEAD)>
            <button type="button" class=(ui::ICON_BUTTON_SM) aria-label="Back to what needs you" data-on:click=(BACK)>
                ui::icon(name: "chevron-left")
            </button>
            (child)
        </div>
    })
}

/// A hire's avatar, name and model, for a header. A `plain` avatar is grey,
/// as the floor draws hires who are not at work.
#[component]
async fn who(
    #[into] hire: String,
    #[into] model: String,
    #[default] plain: bool,
) -> topcoat::Result<impl View> {
    Ok(view! {
        if plain {
            <span class="av" aria-hidden="true">(ui::initials(&hire))</span>
        } else {
            ui::avatar(hire: hire.clone())
        }
        <b class="truncate font-medium">(hire)</b>
        if !model.is_empty() {
            ui::model_tag(model: model)
        }
    })
}

/// A ticket's title, and under it its id and goal.
#[component]
async fn titled(ticket: Ticket) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let under = match ticket.goal.lines().next().filter(|g| !g.is_empty()) {
        Some(goal) => format!("{id} · {goal}"),
        None => id,
    };
    Ok(view! {
        <div>
            <h2 class=(PANEL_TITLE)>(ticket.title.clone())</h2>
            subtitle(text: under)
        </div>
    })
}

/// The quiet mono line under a panel's title, kept to one line.
#[component]
async fn subtitle(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class=(SUBTITLE) title=(text.clone())>(text)</span>
    })
}

/// Reasons as sentences: "No rule allows it."
fn sentences(reasons: &[String]) -> String {
    reasons
        .iter()
        .map(|r| {
            let r = r.trim();
            let mut chars = r.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            let end = if r.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            format!("{}{}{end}", first.unwrap_or_default(), chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[component]
async fn open_ticket(#[into] id: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <a href=(format!("/tickets/{id}")) class=(ui::BUTTON)>
            "Open ticket"
            ui::icon(name: "arrow-right", small: true)
        </a>
    })
}

#[component]
async fn cancel(#[into] id: String) -> topcoat::Result<impl View> {
    let post = format!("confirm('Cancel {id}?') && @post('/tickets/{id}/cancel')");
    Ok(view! {
        <button type="button" class=(ui::DANGER) data-on:click=(post)>"Cancel"</button>
    })
}

/// "$0.42 of $8.00", or just what was spent when the cap is unknown.
#[component]
async fn spend_of(spent: f64, cap: Option<f64>) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="tabular-nums">
            (ui::usd(spent))
            if let Some(cap) = cap {
                <span class="text-fg-3">" of " (ui::limit(cap))</span>
            }
        </span>
    })
}

/// The four-step bar across the panel, with its own words under it.
#[component]
async fn steps_wide(step: usize, child: Child<'_>) -> topcoat::Result<impl View> {
    let bars = (0..ui::STEPS.len()).map(move |i| match i.cmp(&step) {
        std::cmp::Ordering::Less => "done",
        std::cmp::Ordering::Equal => "now",
        std::cmp::Ordering::Greater => "",
    });
    Ok(view! {
        <div class="flex flex-col gap-[5px]">
            <div class="steps-bar grid grid-cols-4 gap-[3px]">
                for class in bars { <i class=(class)></i> }
            </div>
            <span class="text-[12px] text-fg-3">(child)</span>
        </div>
    })
}

/// The icon for a tool call, or for a message.
fn tool_icon(tool: Option<&str>) -> &'static str {
    match tool {
        Some("Read") => "file-text",
        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => "file-pen-line",
        Some("Bash") => "terminal",
        Some("Grep" | "Glob" | "WebSearch") => "search",
        Some(_) => "file-code",
        None => "message-square-text",
    }
}

/// What the agent is doing now and for how long.
#[component]
async fn doing(action: Action, now: u64) -> topcoat::Result<impl View> {
    let since = (action.at_ms > 0).then(|| ui::elapsed(now.saturating_sub(action.at_ms)));
    let text = words::action_text(&action);
    let (said, code) = words::action_line(&action);
    Ok(view! {
        <div class=(DOING) title=(text)>
            ui::icon(name: tool_icon(action.tool.as_deref()))
            <span class="truncate">
                (said)
                if let Some(code) = code {
                    " " <span class="font-mono text-[12.5px] text-fg">(code)</span>
                }
            </span>
            if let Some(since) = since {
                <span class="ml-auto shrink-0 font-sans text-[12px] tabular-nums text-fg-3">(since)</span>
            }
        </div>
    })
}

/// The latest tool calls in a run log: (tool, what, when), oldest first.
fn latest_calls(log: &str, count: usize) -> Vec<(String, String, u64)> {
    let mut calls: Vec<(String, String, u64)> = log
        .lines()
        .rev()
        .filter_map(|raw| {
            let entry: Value = serde_json::from_str(raw).ok()?;
            let e = &entry["event"];
            (e["type"] == "tool_call").then_some(())?;
            let tool = e["name"].as_str()?.to_string();
            // The studio's own tools say what they did in words.
            let what = match mcp_words(&tool, &e["input"]) {
                Some(words) => words
                    .split_once(": ")
                    .map(|(_, what)| what.to_string())
                    .unwrap_or_default(),
                None => tool_arg(&tool, &e["input"]),
            };
            let what = truncate(&what, ARG_SHOWN);
            Some((tool, what, entry["at"].as_u64().unwrap_or(0)))
        })
        .take(count)
        .collect();
    calls.reverse();
    calls
}

/// The agent's latest words in a run log.
fn last_said(log: &str) -> Option<String> {
    log.lines().rev().find_map(|raw| {
        let entry: Value = serde_json::from_str(raw).ok()?;
        let e = &entry["event"];
        (e["type"] == "text").then_some(())?;
        let words: Vec<&str> = e["text"].as_str()?.split_whitespace().collect();
        (!words.is_empty()).then(|| truncate(&words.join(" "), SAID_SHOWN))
    })
}

/// Markdown cut down to one plain line for a quote: list items lose their
/// markers and are joined with "; ".
fn plain(text: &str) -> String {
    let mut line = String::new();
    let mut in_list = false;
    for raw in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = list_item(raw);
        if !line.is_empty() {
            line.push_str(if item.is_some() && in_list { "; " } else { " " });
        }
        let words: Vec<&str> = item.unwrap_or(raw).split_whitespace().collect();
        line.push_str(&words.join(" "));
        in_list = item.is_some();
    }
    truncate(&line.replace(['*', '`'], ""), SAID_SHOWN)
}

/// A Markdown list item's text, without its "- ", "* " or "1. " marker.
fn list_item(line: &str) -> Option<&str> {
    if let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| line.strip_prefix(m)) {
        return Some(rest);
    }
    let digits = line.find(|c: char| !c.is_ascii_digit())?;
    let rest = line[digits..]
        .strip_prefix(". ")
        .or_else(|| line[digits..].strip_prefix(") "))?;
    (digits > 0).then_some(rest)
}

/// When the current try began: the ticket's latest claim, or the checks or
/// review sending it back for another try.
fn try_started(events: &[client::Stamped]) -> Option<u64> {
    events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e.event,
                TicketEvent::Claimed
                    | TicketEvent::ChecksFailed { .. }
                    | TicketEvent::ReviewFailed { .. }
            )
        })
        .map(|e| e.at_ms)
}

/// Why the work before this try was sent back, and by what: the latest
/// failed checks, failed review, conflict or the owner's request for changes
/// between the previous claim and the current one.
fn sent_back(events: &[client::Stamped]) -> Option<(&'static str, String)> {
    let claimed = |e: &client::Stamped| matches!(e.event, TicketEvent::Claimed);
    let current = events.iter().rposition(claimed)?;
    events[..current]
        .iter()
        .rev()
        .take_while(|e| !claimed(e))
        .find_map(|e| match &e.event {
            TicketEvent::ChecksFailed { detail } => Some(("the checks", detail.clone())),
            TicketEvent::ReviewFailed { detail } => Some(("review", detail.clone())),
            TicketEvent::ChangesRequested { detail } => Some(("you", detail.clone())),
            TicketEvent::Conflicted { branch, files } => Some((
                "a conflict",
                format!("newer work on {branch} conflicts in {}", files.join(", ")),
            )),
            _ => None,
        })
}

/// A ticket's panel, for where it is on the floor.
fn ticket_panel(cx: &Cx, ticket: Ticket, office: Office, detail: Detail) -> BoxView<'_> {
    let id = ticket.id.to_string();
    let asks: Vec<AskView> = office
        .asks
        .iter()
        .filter(|a| a.ticket == id)
        .cloned()
        .collect();
    match room(&ticket) {
        _ if !asks.is_empty() => {
            view! { cx => ask_panel(ticket: ticket, asks: asks, office: office, detail: detail) }
                .boxed()
        }
        _ if ticket.state.kind() == "held" => {
            view! { cx => held_panel(ticket: ticket, office: office) }.boxed()
        }
        Room::Checks => {
            view! { cx => checks_panel(ticket: ticket, office: office, detail: detail) }.boxed()
        }
        Room::Review => {
            view! { cx => review_panel(ticket: ticket, office: office, detail: detail) }.boxed()
        }
        Room::Ready => view! { cx => ready_panel(ticket: ticket, detail: detail) }.boxed(),
        Room::UpNext => view! { cx => queued_panel(ticket: ticket, office: office) }.boxed(),
        Room::Done => view! { cx => done_panel(ticket: ticket) }.boxed(),
        Room::Desk | Room::Planning => {
            view! { cx => work_panel(ticket: ticket, office: office, detail: detail) }.boxed()
        }
    }
}

#[component]
async fn work_panel(ticket: Ticket, office: Office, detail: Detail) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let hire = ticket.hire.to_string();
    let model = office
        .member(&hire)
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let state = ticket.state.kind();
    let action = latest_action(&detail.log, ARG_SHOWN);
    let calls = latest_calls(&detail.log, STEPS_SHOWN);
    let try_n = ticket.attempts + 1;
    // A plan is never checked, reviewed or tried again.
    let plan = ticket.kind == TicketKind::Plan;
    let back = sent_back(&detail.events);
    let after = back
        .as_ref()
        .map(|(by, _)| format!(", after {by} sent it back"))
        .unwrap_or_default();
    // Failed checks or review use up a try; the owner's request and a
    // conflict don't.
    let back_line = back.map(|(by, why)| {
        let on = match by {
            "you" | "a conflict" => String::new(),
            _ => format!(" on try {}", ticket.attempts),
        };
        format!("Sent back by {by}{on}: “{}”", plain(&why))
    });
    let started = try_started(&detail.events);
    let now = office.now;
    let spent = office.spent(&id);
    Ok(view! {
        head(
            who(hire: hire, model: model)
            <span class="flex-1"></span>
            ui::state_pill(state: state)
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            if let Some(action) = action {
                doing(action: action, now: now)
            }
            if !plan {
                steps_wide(step: 0,
                    <b class="font-medium text-run-fg">(ui::look(state).label)</b>
                    " · try " (try_n) " of " (MAX_ATTEMPTS) (after)
                )
            }
            if !calls.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"Latest steps"</h3>
                    <div class="overflow-hidden rounded-lg border border-line">
                        for (tool, what, at) in calls {
                            <div class=(TOOL_ROW)>
                                ui::icon(name: tool_icon(Some(&tool)))
                                <span class="min-w-0 max-w-28 truncate font-medium text-fg-2" title=(tool.clone())>(words::tool_name(&tool))</span>
                                <span class="truncate font-mono text-[12px] leading-[18px] text-fg-2" title=(what.clone())>(what)</span>
                                <span class="whitespace-nowrap tabular-nums text-fg-3">(if at > 0 { ago(now.saturating_sub(at)) } else { String::new() })</span>
                            </div>
                        }
                    </div>
                </section>
            }
            if let Some(line) = back_line {
                <p class="flex gap-2 text-[13px] text-fg-2">
                    <span class="mt-[3px] text-bad-fg">ui::icon(name: "circle-x", small: true)</span>
                    <span>(line)</span>
                </p>
            }
            <dl class=(KV)>
                <dt>"Spent"</dt>
                <dd>spend_of(spent: spent, cap: detail.cap)</dd>
                if let Some(started) = started {
                    <dt>"Time"</dt>
                    <dd>(ui::elapsed(now.saturating_sub(started))) (if plan { "" } else { " on this try" })</dd>
                }
                if let Some((added, removed, files)) = detail.stat {
                    <dt>"Changes"</dt>
                    <dd>ui::diff_size(added: added, removed: removed) " in " (files) (if files == 1 { " file" } else { " files" })</dd>
                }
            </dl>
            <div class=(ACTIONS)>
                open_ticket(id: id.clone())
                cancel(id: id)
            </div>
        </div>
    })
}

#[component]
async fn ask_panel(
    ticket: Ticket,
    asks: Vec<AskView>,
    office: Office,
    detail: Detail,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    // Who asks: the author, or a critic in review.
    let hire = asks
        .first()
        .map_or_else(|| ticket.hire.to_string(), |a| a.hire.clone());
    let model = office
        .member(&hire)
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let said = last_said(&detail.log);
    let started = try_started(&detail.events);
    let now = office.now;
    let spent = office.spent(&id);
    let project = ticket.project.to_string();
    let waited: Vec<Option<u64>> = asks
        .iter()
        .map(|a| opened(a).map(|at| now.saturating_sub(at)))
        .collect();
    Ok(view! {
        head(
            who(hire: hire.clone(), model: model)
            <span class="flex-1"></span>
            ui::state_pill(state: "asking")
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            for (ask, waited) in asks.into_iter().zip(waited) {
                ask_full(ask: ask, waited: waited, said: said.clone(), project: project.clone())
            }
            <dl class=(KV)>
                <dt>"Spent"</dt>
                <dd>spend_of(spent: spent, cap: detail.cap)</dd>
                if let Some(started) = started {
                    <dt>"Time"</dt>
                    <dd>(ui::elapsed(now.saturating_sub(started)))</dd>
                }
                if let Some((added, removed, files)) = detail.stat {
                    <dt>"Changes"</dt>
                    <dd>ui::diff_size(added: added, removed: removed) " in " (files) (if files == 1 { " file" } else { " files" })</dd>
                }
            </dl>
            <div class=(ACTIONS)>
                open_ticket(id: id)
            </div>
        </div>
    })
}

/// The command, file, change or question an ask is about.
#[component]
async fn ask_target(ask: AskView, #[default] small: bool) -> topcoat::Result<impl View> {
    let summary = summarise(&ask);
    // A small card keeps a command or path to one line, cut at its end.
    let line = if small { CMD_LINE } else { ui::CMD };
    // The full card asks its questions in its form; a small one names the first.
    let question = questions(&ask.input)
        .into_iter()
        .next()
        .filter(|_| small)
        .map(|(text, _)| text);
    Ok(view! {
        if let Some(command) = summary.command.clone() {
            <div class=(line) title=(command.clone())>(command)</div>
        } else if let Some((old, new)) = summary.change.clone() {
            <div class=(format!("overflow-hidden rounded-lg border border-line font-mono text-[12px] leading-[18px] {}", if small { "" } else { "text-[12.5px]" }))>
                <div class="border-b border-line bg-bg-2 px-3 py-1.5 text-fg-2">(summary.path.clone().unwrap_or_default())</div>
                <div class="whitespace-pre-wrap break-all bg-bad-bg px-3 py-0.5 text-bad-fg">"- " (old)</div>
                <div class="whitespace-pre-wrap break-all bg-ok-bg px-3 py-0.5 text-ok-fg">"+ " (new)</div>
            </div>
        } else if let Some(path) = summary.path.clone() {
            <div class=(line) title=(path.clone())>(path)</div>
        } else if let Some(text) = question {
            <p class="text-[13px]">(text)</p>
        } else if let Some(json) = summary.json.clone() {
            <div class=(format!("{line} max-h-40 overflow-auto"))>(json)</div>
        }
    })
}

/// An ask in full, as on its ticket: what, the agent's words, why, and the
/// answers.
#[component]
async fn ask_full(
    ask: AskView,
    waited: Option<u64>,
    said: Option<String>,
    project: String,
) -> topcoat::Result<impl View> {
    let summary = summarise(&ask);
    let why = sentences(&reasons(&ask, &summary));
    let question = ask.kind == "question";
    let questions = questions(&ask.input);
    let always = always_prefix(&ask);
    let submit = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    let ago_text = waited
        .map(|w| format!("{} ago", ago(w)))
        .unwrap_or_default();
    let hire = ask.hire.clone();
    let head_text = words::ask_head(&ask);
    Ok(view! {
        ui::ask_frame(head: view! { (head_text) }, ago: ago_text,
            ask_target(ask: ask.clone())
            if let Some(said) = said {
                <div class="flex items-start gap-2.5 text-[13.5px] leading-5 text-fg-2">
                    ui::avatar(hire: hire)
                    <span>"“" (said) "”"</span>
                </div>
            }
            if !question {
                <p class=(format!("flex gap-1.5 {}", ui::WHY))>
                    <span class="mt-[3px]">ui::icon(name: "shield-check", small: true)</span>
                    <span>(why)</span>
                </p>
            }
            <form class="flex flex-col gap-3" data-on:submit=(submit)>
                for (text, options) in questions {
                    <fieldset class="flex flex-col gap-1.5">
                        <legend class="mb-1 text-[13px]">(text.clone())</legend>
                        for option in options {
                            <label class="flex min-h-7 items-center gap-2 text-[13px]">
                                <input type="radio" name=(text.clone()) value=(option.clone())>
                                (option)
                            </label>
                        }
                    </fieldset>
                }
                <input name=(MESSAGE) placeholder="Note for the agent (optional)" title="A deny with a note lets the agent carry on another way." class=(format!("{} text-[13px]", ui::INPUT))>
                <div class=(ui::ASK_ACTIONS)>
                    <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY)>
                        ui::icon(name: "check")
                        (if question { "Answer" } else { "Allow" })
                    </button>
                    <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON)>
                        ui::icon(name: "x")
                        "Deny"
                    </button>
                    if let Some(prefix) = always {
                        <button type="submit" name=(DECISION) value=(ALWAYS) class=(ui::GHOST) title=(format!("Allow and save a rule for {prefix} in {project}"))>
                            "Always allow " <span class=(ui::CODE)>(prefix)</span>
                        </button>
                    }
                </div>
            </form>
        )
    })
}

#[component]
async fn held_panel(ticket: Ticket, office: Office) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let hire = ticket.hire.to_string();
    let model = office
        .member(&hire)
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    // Work held for budget starts again by itself once a run is funded.
    let then = if why == "out of budget" {
        "It starts again by itself once the budget allows, or when you resume it."
    } else {
        "It waits until you resume it."
    };
    let resume = format!("@post('/tickets/{id}/resume')");
    Ok(view! {
        head(
            who(hire: hire, model: model)
            <span class="flex-1"></span>
            ui::state_pill(state: ui::shown(&ticket.state))
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            <p class="text-[13px] text-fg-2">"Held: " (why) ". " (then)</p>
            <div class=(ACTIONS)>
                <button type="button" class=(ui::PRIMARY) data-on:click=(resume)>
                    ui::icon(name: "circle-play")
                    "Resume"
                </button>
                open_ticket(id: id.clone())
                cancel(id: id)
            </div>
        </div>
    })
}

#[component]
async fn checks_panel(
    ticket: Ticket,
    office: Office,
    detail: Detail,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let now = office.now;
    let started = detail
        .events
        .iter()
        .rev()
        .find(|e| matches!(e.event, TicketEvent::ChecksStarted))
        .map(|e| e.at_ms);
    let run = started
        .map(|since| checks_since(&detail.log, since))
        .unwrap_or_default();
    // Each check runs until the next one starts; the last is running now.
    let rows: Vec<(String, u64, bool)> = run
        .iter()
        .enumerate()
        .map(|(i, (command, at))| match run.get(i + 1) {
            Some((_, next)) => (command.clone(), next.saturating_sub(*at), true),
            None => (command.clone(), now.saturating_sub(*at), false),
        })
        .collect();
    let finished = format!("{} finished try {}", ticket.hire, ticket.attempts + 1);
    Ok(view! {
        head(
            ui::icon(name: "flask-conical")
            <b class="font-medium">"Checks"</b>
            <span class="flex-1"></span>
            ui::pill(tone: "run", label: "Running", state: "checking")
        )
        <div class=(BODY)>
            <div>
                <h2 class=(PANEL_TITLE)>(ticket.title.clone())</h2>
                subtitle(text: format!("{id} · {finished}"))
            </div>
            if rows.is_empty() {
                <p class="text-[13px] text-fg-2">
                    "Running the project's checks"
                    if let Some(started) = started {
                        " for " (ui::elapsed(now.saturating_sub(started)))
                    }
                    "."
                </p>
            } else {
                <section>
                    <h3 class=(SECTION_LABEL)>"Project checks"</h3>
                    <div class="overflow-hidden rounded-lg border border-line">
                        for (command, took, done) in rows {
                            <div class="grid grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-2.5 border-t border-line px-3 py-[7px] text-[12.5px] first:border-t-0">
                                if done {
                                    <span class="text-ok-fg">ui::icon(name: "check", small: true)</span>
                                } else {
                                    ui::ring(percent: 50, small: true)
                                }
                                <span class="truncate font-mono text-[12px] leading-[18px] text-fg-2" title=(command.clone())>(command)</span>
                                <span class=(if done { "whitespace-nowrap tabular-nums text-ok-fg" } else { "whitespace-nowrap tabular-nums text-fg-3" })>
                                    (ui::elapsed(took))
                                </span>
                            </div>
                        }
                    </div>
                </section>
            }
            <p class="text-[13px] text-fg-3">"Then it goes to review."</p>
            <div class=(ACTIONS)>
                open_ticket(id: id.clone())
                cancel(id: id)
            </div>
        </div>
    })
}

#[component]
async fn review_panel(
    ticket: Ticket,
    office: Office,
    detail: Detail,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let author = ticket.hire.to_string();
    let author_model = office
        .member(&author)
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let judge = office.judges.get(&id).cloned();
    let action = judge
        .as_ref()
        .and_then(|j| j.action(&detail.log, ARG_SHOWN));
    let step = match judge.as_ref().map(|j| j.gate.clone()) {
        Some(Some(gate)) => format!("the {gate} critic is judging it"),
        Some(None) => "checks passed".to_string(),
        None => "checks passed; no reviewer has started".to_string(),
    };
    let pill = if judge.as_ref().is_some_and(|j| j.gate.is_some()) {
        "Critic"
    } else {
        "Reviewing"
    };
    let now = office.now;
    let spent = office.spent(&id);
    Ok(view! {
        head(
            if let Some(judge) = judge.clone() {
                who(hire: judge.hire, model: judge.model)
            } else {
                <b class="font-medium">"Review"</b>
            }
            <span class="flex-1"></span>
            ui::pill(tone: "run", label: pill, state: "reviewing")
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            if let Some(action) = action {
                doing(action: action, now: now)
            }
            steps_wide(step: 3,
                <b class="font-medium text-run-fg">"Review"</b> " · " (step)
            )
            <dl class=(KV)>
                <dt>"Written by"</dt>
                <dd class="flex items-center gap-[7px]">
                    ui::avatar(hire: author.clone())
                    <span>
                        (author)
                        if !author_model.is_empty() {
                            <span class="text-fg-3">", " (author_model)</span>
                        }
                    </span>
                </dd>
                if let Some((added, removed, files)) = detail.stat {
                    <dt>"Changes"</dt>
                    <dd>ui::diff_size(added: added, removed: removed) " in " (files) (if files == 1 { " file" } else { " files" })</dd>
                }
                <dt>"Spent"</dt>
                <dd>spend_of(spent: spent, cap: detail.cap)</dd>
            </dl>
            <div class=(ACTIONS)>
                open_ticket(id: id.clone())
                cancel(id: id)
            </div>
        </div>
    })
}

#[component]
async fn ready_panel(ticket: Ticket, detail: Detail) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let tries = match ticket.attempts {
        0 => "on the first try".to_string(),
        n => format!("on try {}", n + 1),
    };
    let passed = if detail.owner_reviews {
        format!(
            "Passed checks {tries}. No reviewer on another model read it, so it is yours to review."
        )
    } else {
        format!("Passed checks and review {tries}.")
    };
    let goal = ticket.goal.lines().next().unwrap_or_default().to_string();
    let land_label = if goal.is_empty() {
        "Land".to_string()
    } else {
        format!("Land on {}", truncate(&goal, 32))
    };
    let land = format!("@post('/tickets/{id}/land')");
    Ok(view! {
        head(
            ui::state_icon(state: "ready")
            <b class="font-medium">"Ready to land"</b>
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            <p class="text-[13px] text-fg-2">(passed)</p>
            if !detail.files.is_empty() {
                <div class="flex flex-col gap-1.5">
                    for (path, added, removed) in detail.files {
                        <div class="flex items-center gap-2 text-[12.5px]">
                            <span class="text-fg-3">ui::icon(name: "file-diff", small: true)</span>
                            <span class="min-w-0 flex-1 truncate font-mono text-[12px]" title=(path.clone())>(path)</span>
                            ui::diff_size(added: added, removed: removed)
                        </div>
                    }
                </div>
            }
            <div class=(ACTIONS)>
                <button type="button" class=(ui::PRIMARY) data-on:click=(land)>
                    ui::icon(name: "git-merge")
                    (land_label)
                </button>
                open_ticket(id: id)
            </div>
        </div>
    })
}

#[component]
async fn queued_panel(ticket: Ticket, office: Office) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let why = ui::Wait::of(
        &ticket,
        &office.tickets,
        office.status.out_of_budget,
        office.status.max_runs,
    )
    .sentence();
    Ok(view! {
        head(
            ui::state_icon(state: "queued")
            <b class="font-medium">"Up next"</b>
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            <p class="text-[13px] text-fg-2">(why)</p>
            <div class=(ACTIONS)>
                open_ticket(id: id.clone())
                cancel(id: id)
            </div>
        </div>
    })
}

#[component]
async fn done_panel(ticket: Ticket) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let state = ticket.state.kind();
    let why = ui::state_detail(&ticket.state);
    Ok(view! {
        head(
            ui::state_icon(state: state)
            <b class="font-medium">(ui::look(state).label)</b>
        )
        <div class=(BODY)>
            titled(ticket: ticket.clone())
            if let Some(why) = why {
                <p class="text-[13px] text-fg-2">(why)</p>
            }
            <div class=(ACTIONS)>
                open_ticket(id: id)
            </div>
        </div>
    })
}

#[component]
async fn goal_panel(goal: Goal, office: Office) -> topcoat::Result<impl View> {
    let state = goal.state();
    let landed = goal.landed();
    let total = goal.filed.len();
    let plan_id = goal.plan.id.clone();
    let spent: f64 = office
        .tickets
        .iter()
        .filter(|t| t.id == plan_id || t.parent.as_ref() == Some(&plan_id))
        .map(|t| office.spent(&t.id.to_string()))
        .sum();
    let branch = Repo::goal_branch_name(&goal.plan.goal);
    let asking = std::iter::once(&goal.plan)
        .chain(&goal.filed)
        .find(|t| t.state.kind() == "asking")
        .map(|t| key("tickets", &t.id.to_string()));
    let merge = (state == "done")
        .then(|| goal.filed.iter().find(|t| t.state.kind() == "landed"))
        .flatten()
        .map(|t| {
            (
                format!("@post('/tickets/{}/merge')", t.id),
                t.base_branch.clone(),
            )
        });
    let summary = format!("{landed} of {total} landed · {}", ui::usd(spent));
    let project = goal.plan.project.to_string();
    let name = goal.name();
    let filed = goal.filed.clone();
    let open = format!("/goals?open={}", super::safe(&plan_id.to_string()));
    Ok(view! {
        head(
            ui::icon(name: "target")
            <b class="font-medium">"Goal"</b>
            <span class="flex-1"></span>
            ui::state_pill(state: state)
        )
        <div class=(BODY)>
            <div>
                <h2 class=(PANEL_TITLE) title=(goal.plan.goal.clone())>(name)</h2>
                subtitle(text: format!("{project} · {branch}"))
            </div>
            <div class="flex flex-col gap-1.5">
                <span class="text-[13px] tabular-nums">(summary)</span>
                if total > 0 {
                    ui::meter(value: landed as f64, max: total as f64, tone: "ok")
                }
            </div>
            if filed.is_empty() {
                <p class="text-[13px] text-fg-2">"The producer has not filed tickets yet."</p>
            } else {
                <div class="flex flex-col">
                    for t in filed {
                        <a href=(format!("/tickets/{}", t.id)) class=(TICKET_ROW)>
                            <span class="font-mono text-[12px] text-fg-3">(t.id.to_string())</span>
                            ui::state_icon(state: ui::shown(&t.state))
                            <span class="min-w-0 text-fg-2"><b class="font-medium text-fg">(t.title.clone())</b> " · " (ui::look(ui::shown(&t.state)).label.to_lowercase())</span>
                        </a>
                    }
                </div>
            }
            <div class=(ACTIONS)>
                if let Some(k) = asking {
                    <button type="button" class=(ui::PRIMARY) data-on:click=(select(&k))>"See the ask"</button>
                }
                if let Some((post, base)) = merge {
                    <button type="button" class=(ui::PRIMARY) data-on:click=(post)>
                        ui::icon(name: "git-merge")
                        "Merge into " (base)
                    </button>
                }
                <a href=(open) class=(ui::BUTTON)>
                    "Open goal"
                    ui::icon(name: "arrow-right", small: true)
                </a>
            </div>
        </div>
    })
}

/// What a front-desk hire is for.
fn role_words(role: &str) -> Option<&'static str> {
    match role {
        "approver" => {
            Some("Decides the tool calls no rule covers, so only the risky ones reach you.")
        }
        "router" => Some("Picks the hire for each new ticket."),
        "producer" => Some("Plans goals into tickets."),
        "reviewer" => Some("Reads finished work before it is ready to land."),
        _ => None,
    }
}

#[component]
async fn hire_panel(
    member: client::Member,
    office: Office,
    detail: Detail,
) -> topcoat::Result<impl View> {
    let id = member.id.clone();
    let front = matches!(member.role.as_str(), "approver" | "router");
    let open: Vec<Ticket> = office
        .tickets
        .iter()
        .filter(|t| t.hire.to_string() == id && !matches!(room(t), Room::Done | Room::UpNext))
        .cloned()
        .collect();
    let resting = !front && floor::resting(&office).iter().any(|m| m.id == id);
    let window = office.window();
    let idle = resting
        .then(|| floor::idle_since(&office, &id))
        .flatten()
        .map(|at| ui::elapsed(office.now.saturating_sub(at)));
    let landed = floor::shipped(&office)
        .iter()
        .filter(|(t, _)| t.hire.to_string() == id && t.state.kind() == "landed")
        .count();
    let spent = detail.by_hire.get(&id).copied().unwrap_or(0.0);
    let pick = format!("$pick = '{}'", super::safe(&id));
    let producer = member.role == "producer";
    let words = role_words(&member.role);
    let did = office.today.get(&id).map(hires::did).unwrap_or_default();
    let calls = detail.calls.clone();
    let now = office.now;
    Ok(view! {
        head(
            who(hire: id.clone(), model: member.model.clone(), plain: front || resting)
            <span class="flex-1"></span>
            if resting {
                ui::pill(tone: "neutral", label: "Resting")
            }
        )
        <div class=(BODY)>
            if let Some(words) = words {
                <p class="text-[13px] text-fg-2">(words)</p>
            }
            <dl class=(KV)>
                <dt>"Role"</dt>
                <dd>(member.role.clone())</dd>
                if !did.is_empty() {
                    <dt>"Today"</dt>
                    <dd>(did)</dd>
                }
                if let Some(idle) = idle {
                    <dt>"Resting"</dt>
                    <dd>"for " (idle)</dd>
                }
                if !front {
                    <dt>"Landed"</dt>
                    <dd>(landed) " in " (window)</dd>
                }
                <dt>"Spent"</dt>
                <dd><span class="tabular-nums">(ui::usd(spent))</span> <span class="text-fg-3">" in the last 24 hours"</span></dd>
            </dl>
            if !calls.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"Latest"</h3>
                    <div class="flex flex-col">
                        for call in calls {
                            call_row(call: call, now: now)
                        }
                    </div>
                </section>
            }
            if !open.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"On now"</h3>
                    <div class="flex flex-col">
                        for t in open {
                            <button type="button" class=(format!("{TICKET_ROW} cursor-pointer text-left")) data-on:click=(select(&key("tickets", &t.id.to_string())))>
                                <span class="font-mono text-[12px] text-fg-3">(t.id.to_string())</span>
                                ui::state_icon(state: ui::shown(&t.state))
                                <span class="min-w-0 truncate">(t.title.clone())</span>
                            </button>
                        }
                    </div>
                </section>
            }
            <div class=(ACTIONS)>
                if member.role == "approver" {
                    <a href="/rules" class=(ui::GHOST)>"Rules"</a>
                } else if member.role == "router" {
                    <button type="button" class=(ui::PRIMARY) popovertarget="office-new-ticket" data-on:click="$pick = ''">
                        ui::icon(name: "plus")
                        "New ticket"
                    </button>
                } else if producer {
                    <button type="button" class=(ui::PRIMARY) popovertarget="office-new-goal" data-on:click=(pick.clone())>
                        ui::icon(name: "target")
                        "Give it a goal"
                    </button>
                } else {
                    <button type="button" class=(ui::PRIMARY) popovertarget="office-new-ticket" data-on:click=(pick.clone())>
                        ui::icon(name: "plus")
                        "Give it a ticket"
                    </button>
                }
                <a href=(format!("/hires?file={id}")) class=(ui::GHOST)>"Edit hire"</a>
            </div>
        </div>
    })
}

/// One of the approver's calls, worded as the design has them: "Allowed
/// `cargo test` in SHOP-12", "Denied `curl` in SHOP-15: not needed", "Sent
/// you SHOP-14: `cargo run`". The approver's reason shows on hover.
#[component]
async fn call_row(call: ApproverCall, now: u64) -> topcoat::Result<impl View> {
    let what = if call.what.is_empty() {
        words::tool_name(&call.tool)
    } else {
        truncate(&call.what, CALL_WHAT)
    };
    let code = format!("{} inline-block max-w-full truncate align-bottom", ui::CODE);
    let link = format!("/tickets/{}", call.ticket);
    let ticket = call.ticket.clone();
    let full = call.what.clone();
    Ok(view! {
        <div class=(EVENT_ROW) title=(call.reason.clone())>
            <span class="text-[12px] tabular-nums text-fg-3">(ago(now.saturating_sub(call.at)))</span>
            if call.verdict == "allowed" {
                <span class="flex h-5 items-center text-ok-fg">ui::icon(name: "check", small: true)</span>
                <span class="min-w-0 text-fg-2">
                    "Allowed " <code class=(code.clone()) title=(full.clone())>(what.clone())</code>
                    " in " <a href=(link.clone()) class=(TICKET_LINK)>(ticket.clone())</a>
                </span>
            } else if call.verdict == "denied" {
                <span class="flex h-5 items-center text-bad-fg">ui::icon(name: "x", small: true)</span>
                <span class="min-w-0 text-fg-2">
                    "Denied " <code class=(code.clone()) title=(full.clone())>(what.clone())</code>
                    " in " <a href=(link.clone()) class=(TICKET_LINK)>(ticket.clone())</a>
                    ": " (call.reason.clone())
                </span>
            } else {
                ui::state_icon(state: "asking")
                <span class="min-w-0 text-fg-2">
                    "Sent you " <a href=(link) class=(TICKET_LINK)>(ticket)</a>
                    ": " <code class=(code) title=(full)>(what)</code>
                </span>
            }
        </div>
    })
}

/// What needs the owner, what is ready, the last day, and the latest events.
#[component]
async fn overview(office: Office) -> topcoat::Result<impl View> {
    // Budget and quota holds start again by themselves, so they wait apart.
    let (held, on_hold): (Vec<Ticket>, Vec<Ticket>) = office
        .tickets
        .iter()
        .filter(|t| t.state.kind() == "held")
        .cloned()
        .partition(|t| ui::needs_owner(&t.state));
    let needs = office.asks.len() + held.len();
    let ready: Vec<Ticket> = office
        .tickets
        .iter()
        .filter(|t| room(t) == Room::Ready)
        .cloned()
        .collect();
    let shipped = floor::shipped(&office);
    let count = |state: &str| {
        shipped
            .iter()
            .filter(|(t, _)| t.state.kind() == state)
            .count()
    };
    let mut done: Vec<String> = Vec::new();
    for (state, words) in [
        ("landed", "landed"),
        ("failed", "failed"),
        ("cancelled", "cancelled"),
    ] {
        let n = count(state);
        if n > 0 {
            done.push(format!("{n} {words}"));
        }
    }
    let recent: Vec<(client::Happened, &str, String)> = office
        .recent
        .iter()
        .rev()
        .filter(|e| !matches!(e.event, TicketEvent::RunStarted { .. }))
        .take(RECENT_SHOWN)
        .map(|e| {
            let (state, said) = event_words(&e.event, &office.asks);
            (e.clone(), state, said)
        })
        .collect();
    // Spend always covers the 24 hours; the counts say so when studiod cut
    // their window shorter.
    let mut done = done.join(", ");
    if office.cut_at.is_some() && !done.is_empty() {
        done = format!("{done} in {}", office.window());
    }
    // The approver's calls, from whichever hire holds the role.
    let approver = office
        .status
        .team
        .iter()
        .filter(|m| m.role == "approver")
        .find_map(|m| office.today.get(&m.id))
        .map(|d| (d.allowed + d.denied + d.to_owner, d.to_owner))
        .filter(|(calls, _)| *calls > 0)
        .map(|(calls, sent)| format!("{calls} calls, {sent} sent to you"));
    let asks = office.asks.clone();
    let now = office.now;
    let (spent, daily) = (office.status.spent_24h, office.status.daily_usd);
    Ok(view! {
        <div class=(HEAD)>
            <h2 class="text-[14px] font-semibold leading-5">"Needs you"</h2>
            <span class="text-fg-3 tabular-nums">(needs)</span>
            <span class="flex-1"></span>
            <a href="/asks" class=(ui::GHOST_SM)>"All asks"</a>
        </div>
        <div class=(BODY)>
            if needs == 0 {
                <p class="text-[13px] text-fg-3">"Nothing needs you right now."</p>
            }
            for ask in asks {
                mini_ask(waited: opened(&ask).map(|at| now.saturating_sub(at)), ask: ask)
            }
            for ticket in held {
                held_row(ticket: ticket)
            }
            if !ready.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"Ready to land"</h3>
                    <div class="flex flex-col gap-2">
                        for ticket in ready {
                            ready_row(size: office.sizes.get(&ticket.id.to_string()).copied(), ticket: ticket)
                        }
                    </div>
                </section>
            }
            if !on_hold.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"On hold, starts by itself"</h3>
                    <div class="flex flex-col gap-2">
                        for ticket in on_hold {
                            held_row(ticket: ticket)
                        }
                    </div>
                </section>
            }
            <section>
                <h3 class=(SECTION_LABEL)>"Last 24 hours"</h3>
                <dl class=(KV)>
                    <dt>"Spent"</dt>
                    <dd class="flex flex-col gap-1.5">
                        <span class="tabular-nums">(ui::usd(spent)) <span class="text-fg-3">" of " (ui::limit(daily))</span></span>
                        ui::meter(value: spent, max: daily)
                    </dd>
                    if !done.is_empty() {
                        <dt>"Shipped"</dt>
                        <dd>(done)</dd>
                    }
                    if let Some(calls) = approver {
                        <dt>"Approver"</dt>
                        <dd>(calls)</dd>
                    }
                </dl>
            </section>
            if !recent.is_empty() {
                <section>
                    <h3 class=(SECTION_LABEL)>"Recent"</h3>
                    <div class="flex flex-col">
                        for (e, state, said) in recent {
                            event_row(happened: e, state: state, said: said, now: now)
                        }
                    </div>
                </section>
            }
            <p class="text-[12.5px] text-fg-3">"Pick anyone on the floor to see their work."</p>
        </div>
    })
}

/// A compact ask: what it wants, and Allow, Deny or Details.
#[component]
async fn mini_ask(ask: AskView, waited: Option<u64>) -> topcoat::Result<impl View> {
    let summary = summarise(&ask);
    let why = sentences(&reasons(&ask, &summary));
    let question = ask.kind == "question";
    let submit = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    let details = select(&key("tickets", &ask.ticket));
    Ok(view! {
        <div id=(format!("ask-{}", ask.id)) class="overflow-hidden rounded-[10px] border border-attn-line">
            <div class="flex items-center gap-2 bg-attn-bg px-3 py-2 text-[12.5px] font-medium text-attn-fg">
                ui::icon(name: "shield-alert", small: true)
                <span class="font-mono">(ask.ticket.clone())</span>
                <span class="truncate">(words::ask_title(&ask))</span>
                if let Some(waited) = waited {
                    <span class="ml-auto shrink-0 font-normal">(ago(waited))</span>
                }
            </div>
            <form class="flex flex-col gap-2.5 px-3 pb-3 pt-2.5" data-on:submit=(submit)>
                ask_target(ask: ask.clone(), small: true)
                if !question {
                    <p class="text-[12.5px] leading-[18px] text-fg-3">(why)</p>
                }
                <div class="flex flex-wrap items-center gap-2">
                    if !question {
                        <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY_SM)>"Allow"</button>
                        <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON_SM)>"Deny"</button>
                    }
                    <button type="button" class=(if question { ui::BUTTON_SM } else { ui::GHOST_SM }) data-on:click=(details)>
                        (if question { "Answer" } else { "Details" })
                    </button>
                </div>
            </form>
        </div>
    })
}

/// A held ticket: its title, id and why, and its panel.
#[component]
async fn held_row(ticket: Ticket) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    Ok(view! {
        <div class="grid grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-3 rounded-[10px] border border-line px-3 py-2">
            ui::state_icon(state: ui::shown(&ticket.state))
            <div class="flex min-w-0 flex-col">
                <span class="truncate font-medium">(ticket.title.clone())</span>
                <span class="truncate text-[12.5px] text-fg-3">(id.clone()) " · " (why)</span>
            </div>
            <button type="button" class=(ui::GHOST_SM) data-on:click=(select(&key("tickets", &id)))>"Details"</button>
        </div>
    })
}

#[component]
async fn ready_row(ticket: Ticket, size: Option<(u64, u64)>) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let land = format!("@post('/tickets/{id}/land')");
    Ok(view! {
        <div class="grid grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-3 rounded-[10px] border border-line px-3 py-2">
            ui::state_icon(state: "ready")
            <button type="button" class="flex min-w-0 cursor-pointer flex-col text-left" data-on:click=(select(&key("tickets", &id)))>
                <span class="truncate font-medium" title=(ticket.title.clone())>(ticket.title.clone())</span>
                <span class="truncate text-[12.5px] text-fg-3">
                    (id.clone())
                    if let Some((added, removed)) = size {
                        " · " ui::diff_size(added: added, removed: removed)
                    }
                </span>
            </button>
            <button type="button" class=(ui::BUTTON_SM) data-on:click=(land) aria-label=(format!("Land {id}"))>"Land"</button>
        </div>
    })
}

/// How an event reads in the Recent list: the glyph's state, then the words
/// after the ticket's id. An ask still open says what it wants.
fn event_words(event: &TicketEvent, asks: &[AskView]) -> (&'static str, String) {
    let words = |state, text: &str| (state, text.to_string());
    match event {
        TicketEvent::Created { .. } => words("queued", "was filed"),
        TicketEvent::Claimed => words("working", "started"),
        TicketEvent::RunStarted { .. } => words("working", "started a run"),
        TicketEvent::AskOpened { ask } => match asks.iter().find(|a| a.id == ask.to_string()) {
            Some(open) => ("asking", words::asked(open)),
            None => words("asking", "asked you"),
        },
        TicketEvent::AskResolved { approved: true, .. } => words("ready", "was allowed"),
        TicketEvent::AskResolved { .. } => words("failed", "was denied"),
        TicketEvent::ChecksStarted => words("checking", "checks started"),
        TicketEvent::ChecksPassed => words("ready", "passed checks"),
        TicketEvent::ChecksFailed { .. } => words("failed", "failed checks"),
        TicketEvent::ReviewPassed => words("ready", "passed review"),
        TicketEvent::ReviewFailed { .. } => words("failed", "was sent back by review"),
        TicketEvent::Held { .. } => words("held", "was held"),
        TicketEvent::Resumed => words("working", "resumed"),
        TicketEvent::Landed => words("landed", "landed"),
        TicketEvent::Cancelled => words("cancelled", "was cancelled"),
        TicketEvent::Failed { .. } => words("failed", "failed"),
        TicketEvent::Retried => words("queued", "was queued again"),
        TicketEvent::ChangesRequested { .. } => words("failed", "was sent back by you"),
        TicketEvent::Conflicted { .. } => words("queued", "went back to resolve a conflict"),
    }
}

#[component]
async fn event_row(
    happened: client::Happened,
    state: &'static str,
    said: String,
    now: u64,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class=(EVENT_ROW)>
            <span class="text-[12px] tabular-nums text-fg-3">(ago(now.saturating_sub(happened.at_ms)))</span>
            ui::state_icon(state: state)
            <span class="min-w-0 text-fg-2">
                <a href=(format!("/tickets/{}", happened.ticket_id)) class=(TICKET_LINK)>(happened.ticket_id.clone())</a>
                " " (said)
            </span>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_log_gives_the_latest_calls_and_words() {
        let log = [
            r#"{"at":1,"event":{"type":"text","text":"I'll  look\nfirst"}}"#,
            r#"{"at":2,"event":{"type":"tool_call","name":"Read","input":{"file_path":"a.rs"}}}"#,
            r#"{"at":3,"event":{"type":"tool_call","name":"Bash","input":{"command":"cargo test"}}}"#,
        ]
        .join("\n");
        assert_eq!(
            latest_calls(&log, 1),
            [("Bash".to_string(), "cargo test".to_string(), 3)]
        );
        assert_eq!(latest_calls(&log, 5).len(), 2);
        assert_eq!(last_said(&log).as_deref(), Some("I'll look first"));
    }

    #[tokio::test]
    async fn an_asks_command_is_escaped() {
        let ask = AskView {
            id: "a1".into(),
            ticket: "G-1".into(),
            hire: "dev".into(),
            kind: "permission".into(),
            tool: "Bash".into(),
            input: serde_json::json!({"command": "echo '<script>x</script>'"}),
            reasons: vec!["<b>no rule</b>".into()],
            at_ms: 0,
        };
        let cx = Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => mini_ask(ask: ask, waited: Some(120_000)) },
        )
        .await
        .unwrap();
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"), "{html}");
        assert!(html.contains("&lt;b&gt;no rule"), "{html}");
        assert!(!html.contains("<b>no"), "{html}");
        // Raw only in the tooltip, as an attribute's text.
        assert_eq!(html.matches("<script>").count(), 1, "{html}");
        assert!(html.contains("x</script>'\""), "{html}");
        assert!(
            html.contains("@post('/asks/a1', {contentType: 'form'})"),
            "{html}"
        );
        assert!(html.contains("$sel = 'tickets/G-1'"), "{html}");
        assert!(html.contains(">2m<"), "{html}");
    }
}
