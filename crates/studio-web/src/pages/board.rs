//! The board: four figures, then one list of what needs the owner, what is
//! running, what is ready to land and what is up next, then what finished
//! in the last day.

use futures_util::Stream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{AskView, DaySpend, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{
        content::{
            Form,
            sse::{Event as SseEvent, Sse},
        },
        page, route,
    },
    view::{View, component, view},
};

use super::ask_card::{DECISION, opened, reasons, summarise};
use super::office::judge_now;
use super::ticket::log_tail;
use super::timeline::relative;
use crate::client::Happened;
use crate::summary::{Action, judge_of, latest_action, truncate};
use crate::{AppConfig, client, diffstat, live, ui, words};

/// The board's "today" is the last 24 hours, like the daily limit.
const DAY_MS: u64 = 86_400_000;
const DONE_SHOWN: usize = 20;
const ACTION_SHOWN: usize = 90;
const CODE_SHOWN: usize = 60;
/// Keeps a long goal's chip from crowding a row's title.
const GOAL: &str = "flex min-w-0 max-w-60";

/// States in which a run is under way and nobody is waiting on the owner.
const RUNNING: [&str; 4] = ["claimed", "working", "checking", "reviewing"];
const FINISHED: [&str; 3] = ["landed", "failed", "cancelled"];
/// The events that can end a ticket: landing, failing or being cancelled,
/// a last failed try, or the owner denying its ask.
pub const ENDINGS: [&str; 6] = [
    "checks_failed",
    "review_failed",
    "ask_resolved",
    "landed",
    "failed",
    "cancelled",
];

/// Everything the board shows, read from studiod.
struct Board {
    tickets: Vec<Ticket>,
    asks: Vec<AskView>,
    /// What each ticket has cost, by id.
    spend: BTreeMap<String, f64>,
    /// Spend in each of the last 14 days, oldest first.
    days: Vec<f64>,
    spent: f64,
    daily: f64,
    /// The studio's budget can't fund another run, so nothing starts.
    out_of_budget: bool,
    /// How many runs may go at once; 0 pauses the studio.
    max_runs: u64,
    projects: Vec<String>,
    hires: Vec<String>,
    /// When each held ticket was held, or each running ticket's run began.
    since: HashMap<String, u64>,
    /// What the run logs of running and ready tickets say.
    logs: HashMap<String, Log>,
    /// Lines added and removed by each ready ticket.
    diffs: HashMap<String, (u64, u64)>,
    /// Tickets that finished in the last day, newest first, and when.
    done: Vec<(Ticket, u64)>,
    now: u64,
}

async fn load(config: &AppConfig) -> topcoat::Result<Board> {
    let socket = &config.socket;
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await?;
    let status: client::Status = client::call(socket, Request::Status).await?;
    let asks: Vec<AskView> = client::call(socket, Request::Asks)
        .await
        .unwrap_or_default();
    let spend = client::call(socket, Request::TicketSpends)
        .await
        .unwrap_or_default();
    let days: Vec<DaySpend> = client::call(socket, Request::SpendByDay { days: 14 })
        .await
        .unwrap_or_default();
    let now = ui::now_ms();
    let since: HashMap<String, u64> = client::call(socket, Request::StateSince)
        .await
        .unwrap_or_default();
    let mut logs = HashMap::new();
    for ticket in &tickets {
        let id = ticket.id.to_string();
        let kind = ticket.state.kind();
        if RUNNING.contains(&kind) || kind == "ready" {
            let mut log = read_log(&log_tail(&config.data_dir, &id).await);
            // A long review can push its judge out of the tail.
            if kind == "reviewing" {
                log.judge = judge_now(&config.data_dir, &id)
                    .await
                    .map(|j| (j.hire, j.gate.is_some()));
            }
            logs.insert(id, log);
        }
    }
    let ready: Vec<&Ticket> = tickets
        .iter()
        .filter(|t| t.state.kind() == "ready")
        .collect();
    let diffs = diffstat::sizes(socket, &ready).await;
    let recent = Request::RecentEvents {
        since_ms: now.saturating_sub(DAY_MS),
        limit: None,
        kinds: ENDINGS.map(String::from).to_vec(),
    };
    let recent: Vec<Happened> = client::call(socket, recent).await.unwrap_or_default();
    Ok(Board {
        done: finished(&recent, &tickets),
        tickets,
        asks,
        spend,
        days: days.iter().map(|d| d.usd).collect(),
        spent: status.spent_24h,
        daily: status.daily_usd,
        out_of_budget: status.out_of_budget,
        max_runs: status.max_runs,
        projects: status.projects,
        hires: status.hires,
        since,
        logs,
        diffs,
        now,
    })
}

/// Tickets that are landed, failed or cancelled, newest first, each with
/// the time of its latest event in `events` (which should hold `ENDINGS`):
/// the one that ended it. Tickets that ended before `events` are left out.
pub fn finished(events: &[Happened], tickets: &[Ticket]) -> Vec<(Ticket, u64)> {
    let mut latest: HashMap<&str, u64> = HashMap::new();
    for e in events {
        latest.insert(&e.ticket_id, e.at_ms);
    }
    let mut done: Vec<(Ticket, u64)> = tickets
        .iter()
        .filter(|t| FINISHED.contains(&t.state.kind()))
        .filter_map(|t| Some((t.clone(), *latest.get(t.id.0.as_str())?)))
        .collect();
    done.sort_by_key(|(_, end)| std::cmp::Reverse(*end));
    done
}

/// What a ticket's run log tells the board.
#[derive(Clone, Debug, Default, PartialEq)]
struct Log {
    /// The agent's latest tool call or message.
    action: Option<Action>,
    /// The reviewer or critic at work, and whether it is a critic.
    judge: Option<(String, bool)>,
    /// What the latest attempt passed: "checks", each gate, "review".
    passed: Vec<String>,
}

/// Reads the studio's notes in a run log: "checks passed" starts an
/// attempt's verdicts, then "review by …", "gate … by …" and their passes.
fn read_log(text: &str) -> Log {
    let mut log = Log {
        action: latest_action(text, ACTION_SHOWN),
        ..Log::default()
    };
    let mut reviewed = false;
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(note) = entry["note"].as_str() else {
            continue;
        };
        let gate = note.strip_prefix("gate ");
        if note == "checks passed" {
            log.passed = vec!["checks".into()];
            log.judge = None;
            reviewed = false;
        } else if note == "review passed" {
            reviewed = true;
        } else if let Some(name) = gate.and_then(|g| {
            g.strip_suffix(" passed")
                .or_else(|| g.strip_suffix(" passed by the owner"))
        }) {
            log.passed.push(name.into());
        } else if let Some(judge) = judge_of(&entry) {
            log.judge = Some((judge.hire, judge.gate.is_some()));
        }
    }
    if reviewed {
        log.passed.push("review".into());
    }
    log
}

#[page("/board")]
async fn board_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let board = load(config).await?;
    let no_projects = board.projects.is_empty();
    let (projects, hires) = (board.projects.clone(), board.hires.clone());
    Ok(view! {
        if no_projects {
            <div class="rounded-[10px] border border-dashed border-line-2 px-4 py-6 text-center">
                <p class="font-medium">"No projects yet"</p>
                <p class="mt-1 text-fg-3">"Add a game project to start filing tickets:"</p>
                <p class="mt-2"><code class=(ui::CODE)>"studio project add <path>"</code></p>
            </div>
        } else {
            new_work(projects: projects, hires: hires)
        }
        <div data-init="@get('/board/stream')">
            board_view(board: board)
        </div>
    })
}

#[route(GET "/board/stream")]
async fn board_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<SseEvent>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    Ok(live::watch(move || {
        let config = config.clone();
        let cx = cx.clone();
        async move {
            let board = load(&config).await?;
            ui::html(&cx, view! { cx => board_view(board: board) }).await
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

/// A form in a dialog posts to `action` and closes; the reply goes to the
/// page's notice.
fn post_and_close(action: &str) -> String {
    format!("@post('{action}', {{contentType: 'form'}}); el.closest('[popover]').hidePopover()")
}

/// The New goal and New ticket buttons in the top bar, and their dialogs.
#[component]
async fn new_work(projects: Vec<String>, hires: Vec<String>) -> topcoat::Result<impl View> {
    let project_options = projects.clone();
    Ok(view! {
        ui::top_actions(
            <button type="button" class=(ui::BUTTON) popovertarget="new-goal">
                ui::icon(name: "target")
                <span class=(ui::LABEL)>"New goal"</span>
            </button>
            <button type="button" class=(ui::PRIMARY) popovertarget="new-ticket">
                ui::icon(name: "plus")
                <span class=(ui::LABEL)>"New ticket"</span>
            </button>
        )
        ui::dialog(id: "new-ticket", title: "New ticket",
            <form data-on:submit=(post_and_close("/tickets"))>
                <div class=(ui::DIALOG_BODY)>
                    <div class="grid grid-cols-2 gap-3 max-[760px]:grid-cols-1">
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Project"</span>
                            <select name="project" required="" class=(ui::INPUT)>
                                for p in project_options { <option value=(p.clone())>(p)</option> }
                            </select>
                        </label>
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Hire"</span>
                            <select name="hire" class=(ui::INPUT)>
                                <option value="">"Let the router pick"</option>
                                for h in hires { <option value=(h.clone())>(h)</option> }
                            </select>
                        </label>
                    </div>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Title"</span>
                        <input name="title" required="" placeholder="What should be done" class=(ui::INPUT)>
                    </label>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Details"</span>
                        <textarea name="detail" class=(ui::TEXTAREA)></textarea>
                        <span class=(ui::HINT)>"What done looks like. Markdown works."</span>
                    </label>
                </div>
                <div class=(ui::DIALOG_FOOT)>
                    <span class="text-[12.5px] text-fg-3 max-[760px]:hidden">"Lands on its own branch. Nothing is pushed."</span>
                    <span class="spacer"></span>
                    <button type="button" class=(ui::BUTTON) popovertarget="new-ticket" popovertargetaction="hide">"Cancel"</button>
                    <button type="submit" class=(ui::PRIMARY)>"Create ticket"</button>
                </div>
            </form>
        )
        super::goals::new_goal(projects: projects)
    })
}

#[component]
async fn board_view(board: Board) -> topcoat::Result<impl View> {
    let Board {
        tickets,
        asks,
        spend,
        days,
        spent,
        daily,
        out_of_budget,
        max_runs,
        since,
        mut logs,
        diffs,
        done,
        now,
        ..
    } = board;
    let cost = |id: &str| spend.get(id).copied().unwrap_or(0.0);
    let in_state = |kinds: &[&str]| -> Vec<Ticket> {
        tickets
            .iter()
            .filter(|t| kinds.contains(&t.state.kind()))
            .cloned()
            .collect()
    };
    let title_of = |id: &str| {
        tickets
            .iter()
            .find(|t| t.id.0 == id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let mut asks = asks;
    asks.sort_by_key(|a| opened(a).unwrap_or(now));
    // Budget and quota holds end on their own, so they wait with the queue.
    let (held_by_money, held): (Vec<Ticket>, Vec<Ticket>) = in_state(&["held"])
        .into_iter()
        .partition(|t| ui::waits_on_its_own(&t.state));
    let running = in_state(&RUNNING);
    let ready = in_state(&["ready"]);
    let queued = in_state(&["queued"]);

    let waiting = asks.len() + held.len();
    let oldest = asks
        .iter()
        .filter_map(opened)
        .chain(held.iter().filter_map(|t| since.get(&t.id.0).copied()))
        .min()
        .map(|at| format!("Oldest waiting {}", ui::elapsed(now.saturating_sub(at))));
    let count = |kinds: &[&str]| {
        running
            .iter()
            .filter(|t| kinds.contains(&t.state.kind()))
            .count()
    };
    let split: Vec<String> = [
        (count(&["claimed", "working"]), "working"),
        (count(&["checking"]), "in checks"),
        (count(&["reviewing"]), "in review"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    let sizes: Vec<(u64, u64)> = ready
        .iter()
        .filter_map(|t| diffs.get(&t.id.0).copied())
        .collect();
    let total_diff =
        (!sizes.is_empty()).then(|| sizes.iter().fold((0, 0), |(a, r), (x, y)| (a + x, r + y)));
    let only_ready = match ready.as_slice() {
        [one] => Some(one.id.to_string()),
        _ => None,
    };

    let ask_rows: Vec<_> = asks
        .into_iter()
        .map(|a| {
            let title = title_of(&a.ticket);
            let at = opened(&a);
            (a, title, at)
        })
        .collect();
    let held_row_of = |t: Ticket| {
        let at = since.get(&t.id.0).copied();
        (t, at)
    };
    let held_rows: Vec<_> = held.into_iter().map(held_row_of).collect();
    let on_hold_rows: Vec<_> = held_by_money.into_iter().map(held_row_of).collect();
    let run_rows: Vec<_> = running
        .into_iter()
        .map(|t| {
            let id = t.id.to_string();
            (
                t,
                logs.remove(&id).unwrap_or_default(),
                since.get(&id).copied(),
                cost(&id),
            )
        })
        .collect();
    let ready_rows: Vec<_> = ready
        .into_iter()
        .map(|t| {
            let id = t.id.to_string();
            let passed = logs.remove(&id).map(|l| l.passed).unwrap_or_default();
            (t, passed, diffs.get(&id).copied(), cost(&id))
        })
        .collect();
    let next_rows: Vec<_> = queued
        .into_iter()
        .map(|t| {
            let waits = ui::Wait::of(&t, &tickets, out_of_budget, max_runs).short();
            (t, waits)
        })
        .collect();
    let done_count = done.len();
    let done_rows: Vec<_> = done
        .into_iter()
        .take(DONE_SHOWN)
        .map(|(t, at)| {
            let id = t.id.to_string();
            (t, cost(&id), relative(now, at))
        })
        .collect();
    let needs_count = ask_rows.len() + held_rows.len();
    let (run_count, ready_count) = (run_rows.len(), ready_rows.len());
    let next_count = next_rows.len() + on_hold_rows.len();
    let empty = needs_count + run_count + ready_count + next_count == 0;

    Ok(view! {
        <div id="board" class="flex flex-col gap-7 max-[760px]:gap-6">
            ui::kpis(
                ui::kpi(label: "Needs you", state: "asking", value: waiting.to_string(), href: "/asks",
                    <span class=(ui::KPI_SUB)>
                        (oldest.unwrap_or_else(|| "Nothing waiting".to_string()))
                    </span>
                )
                ui::kpi(label: "Running", state: "working", value: run_count.to_string(),
                    if split.is_empty() {
                        <span class=(ui::KPI_SUB)>"Nothing running"</span>
                    } else {
                        <span class="block overflow-hidden">
                            <span class=(ui::KPI_PARTS)>
                                for part in split { <span class=(ui::KPI_PART)>(part)</span> }
                            </span>
                        </span>
                    }
                )
                ui::kpi(label: "Ready to land", state: "ready", value: ready_count.to_string(),
                    <span class=(ui::KPI_SUB)>
                        if ready_count == 0 { "Nothing to land" }
                        if let Some(id) = only_ready { (id) if total_diff.is_some() { " · " } }
                        if let Some((added, removed)) = total_diff {
                            ui::diff_size(added: added, removed: removed)
                        }
                    </span>
                )
                ui::kpi(label: "Spent, last 24h", value: ui::usd(spent), unit: format!("of {}", ui::limit(daily)), href: "/budget",
                    ui::sparkline(values: days, label: "Spend on each of the last 14 days", max: daily)
                )
            )
            if empty {
                <p class="rounded-[10px] border border-dashed border-line-2 px-4 py-6 text-center text-fg-3">
                    "Nothing is waiting for you, running or queued."
                </p>
            } else {
                <div class=(ui::LIST)>
                    if needs_count > 0 {
                        ui::group_head(state: "asking", title: "Needs you", count: needs_count, href: "/asks", link: "Open asks")
                        for (ask, title, at) in ask_rows {
                            ask_row(ask: ask, title: title, opened: at, now: now)
                        }
                        for (ticket, at) in held_rows {
                            held_row(ticket: ticket, at: at, now: now)
                        }
                    }
                    if run_count > 0 {
                        ui::group_head(state: "checking", title: "Running", count: run_count)
                        for (ticket, log, started, spent) in run_rows {
                            run_row(ticket: ticket, log: log, started: started, spent: spent, now: now)
                        }
                    }
                    if ready_count > 0 {
                        ui::group_head(state: "ready", title: "Ready to land", count: ready_count)
                        for (ticket, passed, diff, spent) in ready_rows {
                            ready_row(ticket: ticket, passed: passed, diff: diff, spent: spent)
                        }
                    }
                    if next_count > 0 {
                        ui::group_head(state: "queued", title: "Up next", count: next_count, id: "up-next")
                        for (ticket, waits) in next_rows {
                            next_row(ticket: ticket, waits: waits)
                        }
                        for (ticket, at) in on_hold_rows {
                            held_row(ticket: ticket, at: at, now: now)
                        }
                    }
                </div>
            }
            if done_count > 0 {
                done_table(rows: done_rows, count: done_count)
            }
        </div>
    })
}

/// What an ask wants, for its one-line row: words, then the command, path
/// or name it is about, then why it reached the owner.
#[derive(Debug, PartialEq)]
struct Wants {
    words: String,
    code: Option<String>,
    reason: Option<String>,
}

fn wants(ask: &AskView) -> Wants {
    let (words, code) = words::ask_line(ask, CODE_SHOWN);
    // A question's own text says enough.
    let reason = (ask.kind != "question")
        .then(|| reasons(ask, &summarise(ask)).into_iter().next())
        .flatten()
        .map(|r| truncate(r.lines().next().unwrap_or_default().trim(), CODE_SHOWN));
    Wants {
        words,
        code,
        reason,
    }
}

#[component]
async fn ask_row(
    ask: AskView,
    title: String,
    opened: Option<u64>,
    now: u64,
) -> topcoat::Result<impl View> {
    let Wants {
        words,
        code,
        reason,
    } = wants(&ask);
    let age = opened.map(|at| ui::elapsed(now.saturating_sub(at)));
    let question = ask.kind == "question";
    let href = format!("/tickets/{}", ask.ticket);
    let answer = href.clone();
    let post = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    Ok(view! {
        ui::row(state: "asking", id: ask.ticket, href: href, title: title,
            detail: view! {
                (words)
                if let Some(code) = code { " " <span class=(ui::CODE)>(code)</span> }
                if let Some(reason) = reason { <span class=(ui::REASON)>(reason)</span> }
            },
            ui::avatar(hire: ask.hire)
            if let Some(age) = age { <span class=(format!("{} {}", ui::FIGURE, ui::MUTED))>(age)</span> }
            if question {
                <a href=(answer) class=(ui::PRIMARY_SM)>"Answer"</a>
            } else {
                <form class="contents" data-on:submit=(post)>
                    <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON_SM)>"Deny"</button>
                    <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY_SM)>"Allow"</button>
                </form>
            }
        )
    })
}

/// A ticket held for budget, quota, a login, a conflict or a pause.
#[component]
async fn held_row(ticket: Ticket, at: Option<u64>, now: u64) -> topcoat::Result<impl View> {
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    let age = at.map(|at| ui::elapsed(now.saturating_sub(at)));
    // A hold that ends on its own isn't the owner's to clear, so its reason stays grey.
    let reason = if ui::needs_owner(&ticket.state) {
        ui::REASON
    } else {
        "ml-1.5"
    };
    let resume = format!("@post('/tickets/{}/resume')", ticket.id);
    Ok(view! {
        ui::row(state: ui::shown(&ticket.state), id: ticket.id.to_string(), href: format!("/tickets/{}", ticket.id), title: ticket.title,
            detail: view! { "Held" <span class=(reason)>(why)</span> },
            ui::avatar(hire: ticket.hire.to_string())
            if let Some(age) = age { <span class=(format!("{} {}", ui::FIGURE, ui::MUTED))>(age)</span> }
            <button type="button" class=(ui::BUTTON_SM) data-on:click=(resume)>"Resume"</button>
        )
    })
}

#[component]
async fn run_row(
    ticket: Ticket,
    log: Log,
    started: Option<u64>,
    spent: f64,
    now: u64,
) -> topcoat::Result<impl View> {
    let kind = ticket.state.kind();
    let critic = matches!(log.judge, Some((_, true)));
    let step = if kind == "reviewing" && critic {
        Some(ui::CRITICS)
    } else {
        ui::run_step(kind)
    };
    let who = match log.judge {
        Some((hire, _)) if kind == "reviewing" => hire,
        _ => ticket.hire.to_string(),
    };
    // An action logged before this run began belongs to the last one.
    let action = log
        .action
        .filter(|a| started.is_none_or(|at| a.at_ms == 0 || a.at_ms >= at));
    let (words, code) = words::now_line(kind, action);
    let plan = ticket.kind == TicketKind::Plan;
    let elapsed = started.map(|at| ui::elapsed(now.saturating_sub(at)));
    Ok(view! {
        ui::row(state: kind, id: ticket.id.to_string(), href: format!("/tickets/{}", ticket.id), title: ticket.title,
            detail: view! {
                (words)
                if let Some(code) = code { " " <span class=(ui::CODE)>(code)</span> }
            },
            if plan {
                ui::planning()
            }
            if let (false, Some(step)) = (plan, step) {
                ui::steps(step: step, attempt: ticket.attempts + 1)
            }
            ui::avatar(hire: who)
            if let Some(elapsed) = elapsed { <span class=(ui::FIGURE)>(elapsed)</span> }
            <span class=(ui::FIGURE)>(ui::usd(spent))</span>
        )
    })
}

/// "checks", "checks and review", "checks, design and review".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[component]
async fn ready_row(
    ticket: Ticket,
    passed: Vec<String>,
    diff: Option<(u64, u64)>,
    spent: f64,
) -> topcoat::Result<impl View> {
    // Gates read between checks and review, as the design orders them.
    let mut passed = passed;
    if passed.last().is_some_and(|p| p == "review") {
        let review = passed.pop().unwrap_or_default();
        let checks = passed
            .iter()
            .position(|p| p == "checks")
            .map(|i| passed.remove(i));
        passed = checks.into_iter().chain(passed).chain([review]).collect();
    }
    let words = (!passed.is_empty()).then(|| format!("Passed {}", and_list(&passed)));
    let land = format!("@post('/tickets/{}/land')", ticket.id);
    let goal = ticket.goal.clone();
    Ok(view! {
        ui::row(state: "ready", id: ticket.id.to_string(), href: format!("/tickets/{}", ticket.id), title: ticket.title,
            detail: view! {
                if let Some(words) = words { (words) if diff.is_some() { " · " } }
                if let Some((added, removed)) = diff { ui::diff_size(added: added, removed: removed) }
            },
            if !goal.trim().is_empty() { <span class=(GOAL)>ui::goal_chip(goal: goal)</span> }
            <span class=(ui::FIGURE)>(ui::usd(spent))</span>
            <button type="button" class=(ui::BUTTON_SM) data-on:click=(land)>
                ui::icon(name: "git-merge", small: true)
                "Land"
            </button>
        )
    })
}

#[component]
async fn next_row(ticket: Ticket, waits: String) -> topcoat::Result<impl View> {
    let goal = ticket.goal.clone();
    Ok(view! {
        ui::row(state: "queued", id: ticket.id.to_string(), href: format!("/tickets/{}", ticket.id), title: ticket.title, compact: true,
            if !goal.trim().is_empty() { <span class=(GOAL)>ui::goal_chip(goal: goal)</span> }
            <span class=(ui::MUTED)>(waits)</span>
        )
    })
}

#[component]
async fn done_table(rows: Vec<(Ticket, f64, String)>, count: usize) -> topcoat::Result<impl View> {
    let th_right = format!("{} text-right", ui::TH);
    let td_right = format!("{} text-right tabular-nums", ui::TD);
    Ok(view! {
        <section id="done" class="scroll-mt-16">
            ui::section_head(title: "Done, last 24h", count: Some(count as u64), href: String::new(), label: String::new())
            <div class=(ui::TABLE_BOX)>
                <table class=(ui::TABLE)>
                    <thead>
                        <tr>
                            <th class=(format!("{} w-24", ui::TH))>"Ticket"</th>
                            <th class=(ui::TH)>"Title"</th>
                            <th class=(ui::TH)>"Goal"</th>
                            <th class=(ui::TH)>"Hire"</th>
                            <th class=(th_right.clone())>"Cost"</th>
                            <th class=(th_right)>"Finished"</th>
                        </tr>
                    </thead>
                    <tbody>
                        for (ticket, cost, ago) in rows {
                            let kind = ticket.state.kind();
                            let failed = ui::failure(&ticket.state).map(|(headline, _)| headline);
                            let goal = ticket.goal.lines().next().unwrap_or_default().to_string();
                            let title_class = if kind == "cancelled" { format!("{} text-fg-3", ui::TD_TITLE) } else { ui::TD_TITLE.to_string() };
                            <tr class=(ui::TR)>
                                <td class=(ui::TD)>
                                    <a href=(format!("/tickets/{}", ticket.id))>ui::id_cell(state: kind, id: ticket.id.to_string())</a>
                                </td>
                                <td class=(title_class)>
                                    (ticket.title.clone())
                                    if let Some(failed) = failed {
                                        <span class="ml-1.5 align-middle">ui::pill(tone: "bad", label: failed)</span>
                                    }
                                </td>
                                if goal.is_empty() {
                                    <td class=(format!("{} text-fg-3", ui::TD))>"None"</td>
                                } else {
                                    <td class=(format!("{} max-w-60 truncate text-fg-2", ui::TD))>(goal)</td>
                                }
                                <td class=(ui::TD)>ui::who(hire: ticket.hire.to_string())</td>
                                <td class=(td_right.clone())>(ui::usd(cost))</td>
                                <td class=(format!("{} text-fg-3", td_right))>(ago)</td>
                            </tr>
                        }
                    </tbody>
                </table>
            </div>
        </section>
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn ticket(id: &str, state: Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    fn board(tickets: Vec<Ticket>) -> Board {
        Board {
            tickets,
            asks: Vec::new(),
            spend: BTreeMap::new(),
            days: vec![1.0, 2.0],
            spent: 1.0,
            daily: 10.0,
            out_of_budget: false,
            max_runs: 2,
            projects: vec!["game".into()],
            hires: vec!["dev".into()],
            since: HashMap::new(),
            logs: HashMap::new(),
            diffs: HashMap::new(),
            done: Vec::new(),
            now: 10 * 60_000,
        }
    }

    async fn render(board: Board) -> String {
        let cx = Cx::default();
        ui::html(&cx, view! { cx => board_view(board: board) })
            .await
            .unwrap()
    }

    fn ask(id: &str, ticket: &str, tool: &str, input: Value) -> AskView {
        AskView {
            id: id.into(),
            ticket: ticket.into(),
            hire: "dev".into(),
            kind: "permission".into(),
            tool: tool.into(),
            input,
            reasons: vec!["src/player is protected".into()],
            at_ms: 0,
        }
    }

    #[tokio::test]
    async fn groups_come_in_order_and_empty_ones_are_left_out() {
        let mut b = board(vec![
            ticket("G-1", json!("queued")),
            ticket("G-2", json!("working")),
            ticket("G-3", json!("ready")),
        ]);
        b.diffs.insert("G-3".into(), (184, 12));
        let html = render(b).await;
        let running = html.find(">Running<").unwrap();
        let ready = html.find(">Ready to land<").unwrap();
        let next = html.find(">Up next<").unwrap();
        assert!(running < ready && ready < next, "{html}");
        assert!(!html.contains("Open asks"), "{html}");
        assert!(html.contains("G-3 · "), "{html}");
        assert!(html.contains("+184") && html.contains("−12"), "{html}");
        assert!(html.contains("/tickets/G-3/land"), "{html}");
        assert!(html.contains("$1.00") && html.contains("of $10<"), "{html}");
    }

    #[tokio::test]
    async fn an_empty_board_says_so_once() {
        let html = render(board(Vec::new())).await;
        assert_eq!(
            html.matches("Nothing is waiting for you").count(),
            1,
            "{html}"
        );
        assert!(!html.contains("Up next"), "{html}");
        assert!(html.contains("Nothing running"), "{html}");
    }

    #[tokio::test]
    async fn asks_read_as_one_line_with_deny_and_allow() {
        let mut b = board(vec![ticket("G-1", json!({"asking": {"ask": "a1"}}))]);
        b.asks = vec![ask(
            "a1",
            "G-1",
            "Edit",
            json!({"file_path": "/p/.studio/worktrees/G-1/src/player/movement.rs"}),
        )];
        b.asks[0].at_ms = 4 * 60_000;
        let html = render(b).await;
        assert!(html.contains("Wants to edit"), "{html}");
        assert!(html.contains(">src/player/movement.rs<"), "{html}");
        assert!(html.contains(">src/player is protected<"), "{html}");
        assert!(html.contains("/asks/a1"), "{html}");
        assert!(html.contains(r#"value="deny""#) && html.contains(r#"value="approve""#));
        assert!(html.contains("Oldest waiting 6m"), "{html}");
    }

    #[tokio::test]
    async fn running_rows_show_the_action_step_and_try() {
        let mut working = ticket("G-2", json!("working"));
        working.attempts = 1;
        let mut b = board(vec![working, ticket("G-3", json!("reviewing"))]);
        let log = [
            r#"{"at":1,"event":{"type":"tool_call","name":"Bash","input":{"command":"cargo test"}}}"#,
            r#"{"at":2,"note":"checks passed"}"#,
            r#"{"at":3,"note":"review by critic (opus)"}"#,
        ];
        b.logs.insert("G-2".into(), read_log(log[0]));
        b.logs.insert("G-3".into(), read_log(&log.join("\n")));
        b.since.insert("G-2".into(), 0);
        b.spend.insert("G-2".into(), 0.38);
        let html = render(b).await;
        assert!(html.contains("Running <span"), "{html}");
        assert!(html.contains(">cargo test<"), "{html}");
        assert!(html.contains(" · try 2 of 3"), "{html}");
        assert!(html.contains(">10m<") && html.contains("$0.38"), "{html}");
        assert!(
            html.contains(">1 working</span>") && html.contains(">1 in review</span>"),
            "{html}"
        );
        // The reviewer at work shows instead of the author.
        assert!(html.contains(r#"title="critic""#), "{html}");
    }

    #[test]
    fn the_log_says_what_passed_and_who_judges() {
        let log = [
            r#"{"note":"checks passed"}"#,
            r#"{"note":"review by rev (sonnet)"}"#,
            r#"{"note":"review passed"}"#,
            r#"{"note":"gate design by critic (opus)"}"#,
            r#"{"note":"gate design passed"}"#,
            r#"{"note":"gate sound passed by the owner"}"#,
        ]
        .join("\n");
        let read = read_log(&log);
        assert_eq!(read.passed, ["checks", "design", "sound", "review"]);
        assert_eq!(read.judge, Some(("critic".to_string(), true)));
        assert_eq!(and_list(&read.passed), "checks, design, sound and review");
    }

    #[test]
    fn finished_tickets_come_newest_first_by_their_last_event() {
        let tickets = vec![
            ticket("G-1", json!("landed")),
            // Out of tries: the last failed checks end it, with no failed
            // event of its own.
            ticket("G-2", json!({"failed": {"class": "checks", "detail": "x"}})),
            // The owner denied its ask.
            ticket("G-3", json!({"failed": {"class": "denied", "detail": ""}})),
            ticket("G-4", json!("queued")),
            // Finished before the window.
            ticket("G-5", json!("landed")),
        ];
        let recent = |id: &str, at_ms, event: Value| Happened {
            ticket_id: id.into(),
            at_ms,
            event: serde_json::from_value(event).unwrap(),
        };
        let events = vec![
            recent("G-1", 10, json!({"type": "landed"})),
            recent("G-2", 20, json!({"type": "checks_failed", "detail": "x"})),
            recent(
                "G-3",
                30,
                json!({"type": "ask_resolved", "ask": "a1", "approved": false}),
            ),
            recent("G-4", 40, json!({"type": "cancelled"})),
        ];
        let done = finished(&events, &tickets);
        let ids: Vec<(String, u64)> = done.iter().map(|(t, at)| (t.id.to_string(), *at)).collect();
        assert_eq!(
            ids,
            [
                ("G-3".to_string(), 30),
                ("G-2".to_string(), 20),
                ("G-1".to_string(), 10)
            ]
        );
        assert_eq!(
            ui::failure(&tickets[1].state).map(|f| f.0).as_deref(),
            Some("Checks failed")
        );
    }

    #[test]
    fn asks_say_what_they_want() {
        let bash = ask(
            "a1",
            "G-1",
            "Bash",
            json!({"command": "cargo run --example waves\n--release"}),
        );
        assert_eq!(
            wants(&bash),
            Wants {
                words: "Wants to run".into(),
                code: Some("cargo run --example waves --release".into()),
                reason: Some("src/player is protected".into()),
            }
        );
        let mut gate = ask(
            "a2",
            "G-1",
            "design",
            json!({"gate": "design", "rounds": 3}),
        );
        gate.kind = "gate".into();
        assert_eq!(wants(&gate).words, "Wants to pass the design gate");
    }
}
