//! Saves a screenshot of every web page, served against a stand-in studiod
//! full of sample data.
//!
//! `cargo run -q -p studio-web --example gallery -- <out-dir>`
//!
//! Writes `<name>.png` at 1024px wide, `<name>-1440.png` and `<name>-phone.png`
//! (390px) for every page, plus a few in the light theme at 1024px and 1440px.
//! Four stand-in studiods serve them: a busy one, an idle one, a new studio
//! with nothing in it yet, and the busy one with its office closed.
//!
//! Uses a Chromium browser (`$STUDIO_BROWSER`, Chrome, Brave, Chromium). When
//! there is none, or it can't run (as in an agent's sandbox), macOS Quick Look
//! renders the pages instead, the way the design references were made: the
//! page goes in an iframe of the target width, scaled to fit 1024px. With
//! neither it prints a line and exits 0.

use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TOKEN: &str = "0123abcd";
/// Page widths, by the suffix of their file name.
const WIDTHS: [(&str, u32); 3] = [("", 1024), ("-1440", 1440), ("-phone", 390)];
/// Pages also shown in the light theme (Quick Look only).
const LIGHT: [&str; 3] = ["board", "ticket-asking", "office"];
/// What stands on the office floor, by the key that selects it.
const ELEMENTS: [&str; 16] = [
    "board",
    "corkboard",
    "posters",
    "racks",
    "add-desk",
    "lounge",
    "coffee",
    "trophies",
    "review",
    "crates",
    "returns",
    "counter",
    "queue",
    "safe",
    "add-project",
    "hire",
];
const PAGE_LIMIT: Duration = Duration::from_secs(30);
const DIFFSTAT: &str = " src/landing.rs | 6 +++++-\n src/main.rs    | 1 +\n 2 files changed, 6 insertions(+), 1 deletion(-)\n";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A ticket with the fields every ticket has, plus any `extra` ones.
fn ticket(id: &str, title: &str, hire: &str, state: Value, extra: Value) -> Value {
    let project = if id.starts_with("EDIT") {
        "editor"
    } else {
        "shop"
    };
    let mut ticket = json!({
        "id": id, "project": project, "title": title, "goal": "", "hire": hire,
        "state": state, "attempts": 1, "base_branch": "main",
        "created_at": now_ms() - 3_600_000,
    });
    if let (Some(ticket), Some(extra)) = (ticket.as_object_mut(), extra.as_object()) {
        ticket.extend(extra.clone());
    }
    ticket
}

fn tickets() -> Value {
    // Filed tickets share their plan's goal.
    let child = json!({"parent": "EDIT-1",
                       "goal": "Ship a product editor: add items, undo mistakes, save drafts"});
    json!([
        ticket(
            "SHOP-1",
            "Add a contact page",
            "dev",
            json!("queued"),
            json!({"attempts": 0})
        ),
        ticket(
            "SHOP-2",
            "Fix the search cache",
            "dev",
            json!("working"),
            json!({"goal": "Speed up search results"})
        ),
        ticket(
            "SHOP-3",
            "Add a cart drawer",
            "dev",
            json!({"asking": {"ask": "ASK-1"}}),
            json!({"goal": "Checkout flow", "parent": "SHOP-21", "attempts": 0, "created_at": now_ms() - 21 * 60_000,
                   "detail": "Adding an item opens a drawer with the cart's lines and total, so the shopper stays on the page. The drawer closes when they click outside it or go to checkout. The total comes from `cart::total`, so the drawer and the checkout page always agree.\n\n**Done when** the drawer lists every line with its quantity and price, its total matches the checkout page, and `cart::total` has tests."})
        ),
        ticket(
            "SHOP-4",
            "Retry payments that time out",
            "dev",
            json!({"asking": {"ask": "ASK-2"}}),
            json!({})
        ),
        ticket(
            "SHOP-5",
            "Resize product images",
            "dev",
            json!("checking"),
            json!({})
        ),
        ticket(
            "SHOP-6",
            "Add an order history page",
            "dev",
            json!("reviewing"),
            json!({})
        ),
        ticket(
            "SHOP-7",
            "Add a landing page",
            "dev",
            json!("ready"),
            json!({"goal": "Polish the navigation", "feedback": "The **Sign out** button needs a confirm step:\n\n- ask before signing out\n- keep `Esc` as cancel"})
        ),
        ticket(
            "SHOP-8",
            "Fix the broken footer links",
            "dev",
            json!("landed"),
            json!({})
        ),
        ticket(
            "SHOP-9",
            "Load the product list faster",
            "dev",
            json!({"failed": {"class": "checks", "detail": "cargo clippy: 2 warnings"}}),
            json!({"attempts": 3, "created_at": now_ms() - 110 * 60_000})
        ),
        ticket(
            "SHOP-10",
            "Add gift card support",
            "dev",
            json!({"held": {"reason": "budget"}}),
            json!({})
        ),
        ticket(
            "EDIT-1",
            "Plan: ship the product editor",
            "producer",
            json!("working"),
            json!({"goal": "Ship a product editor: add items, undo mistakes, save drafts",
                   "kind": "plan"})
        ),
        ticket(
            "EDIT-2",
            "Add a form for new items",
            "dev",
            json!("landed"),
            child.clone()
        ),
        ticket(
            "EDIT-3",
            "Add undo and redo",
            "dev",
            json!("queued"),
            child.clone()
        ),
        ticket(
            "EDIT-4",
            "Save drafts as JSON files",
            "dev",
            json!("queued"),
            child
        ),
        ticket(
            "SHOP-11",
            "Draw the store logo",
            "designer",
            json!("landed"),
            json!({"parent": "SHOP-14", "attempts": 0})
        ),
        ticket(
            "SHOP-12",
            "Design the product cards",
            "designer",
            json!({"failed": {"class": "review", "detail": "palette too wide"}}),
            json!({"parent": "SHOP-14"})
        ),
        ticket(
            "SHOP-13",
            "Write the launch email",
            "writer",
            json!({"asking": {"ask": "ASK-3"}}),
            json!({"parent": "SHOP-14"})
        ),
        ticket(
            "SHOP-14",
            "Plan: give the store its look",
            "producer",
            json!("landed"),
            json!({"goal": "Give the store its look: logo, product cards, launch email",
                   "kind": "plan"})
        ),
        // A goal whose tickets all landed and whose branch is merged.
        ticket(
            "SHOP-16",
            "Plan: polish the navigation",
            "producer",
            json!("landed"),
            json!({"goal": "Polish the navigation", "kind": "plan"})
        ),
        ticket(
            "SHOP-17",
            "Landing page with Shop now and Sign out",
            "dev",
            json!("landed"),
            json!({"parent": "SHOP-16"})
        ),
        ticket(
            "SHOP-18",
            "Breadcrumbs on product pages",
            "dev",
            json!("landed"),
            json!({"parent": "SHOP-16"})
        ),
        // A goal that is done but not merged yet.
        ticket(
            "SHOP-19",
            "Plan: save carts",
            "producer",
            json!("landed"),
            json!({"goal": "Save carts between sessions", "kind": "plan"})
        ),
        ticket(
            "SHOP-20",
            "Store carts in the database",
            "dev",
            json!("landed"),
            json!({"parent": "SHOP-19"})
        ),
        // A goal whose next tickets wait for a slot.
        ticket(
            "SHOP-21",
            "Plan: checkout flow",
            "producer",
            json!("landed"),
            json!({"goal": "Checkout flow", "kind": "plan"})
        ),
        ticket(
            "SHOP-22",
            "Count the items in the cart",
            "dev",
            json!("landed"),
            json!({"parent": "SHOP-21"})
        ),
        ticket(
            "SHOP-23",
            "Validate the payment form",
            "dev",
            json!("queued"),
            json!({"parent": "SHOP-21", "attempts": 0})
        ),
        ticket(
            "SHOP-24",
            "Design the order confirmation page",
            "designer",
            json!("queued"),
            json!({"parent": "SHOP-21", "attempts": 0})
        ),
        ticket(
            "SHOP-15",
            "Plan: add a settings page",
            "producer",
            json!("working"),
            json!({"goal": "Add a settings page for email and notification preferences", "kind": "plan"})
        ),
    ])
}

fn hire_file(name: &str, role: &str, model: &str, escalate: &str, about: &str) -> Value {
    json!({
        "name": name, "problem": null,
        "text": format!(
            "---\nname: {name}\nrole: {role}\nruntime: claude-code\nmodel: {model}\nescalate: [{escalate}]\neffort: medium\n---\n{about}\n"
        ),
    })
}

/// A rule file as `RuleFiles` lists it.
fn rule_file(origin: &str, owner: &str, name: &str, head: &str, why: &str) -> Value {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = match origin {
        "project" => format!("{home}/code/{owner}/.studio/rules/{name}.md"),
        _ => format!("{home}/Library/Application Support/studio/rules/{name}.md"),
    };
    json!({
        "origin": origin, "owner": owner, "name": name, "path": path, "problem": null,
        "text": format!("---\n{head}\n---\n{why}\n"),
    })
}

/// What spend history looks like: 14 days to today, across a new month.
fn spend_by_day(days: u32, today: f64) -> Value {
    const PAST: [f64; 13] = [
        3.1, 4.8, 2.2, 6.9, 5.3, 8.4, 4.1, 0.0, 1.2, 7.6, 6.2, 9.1, 5.8,
    ];
    let days = days.min(14) as usize;
    let start = now_ms() - 14 * 86_400_000;
    let usd = PAST.iter().copied().chain([today]).collect::<Vec<_>>();
    let all: Vec<Value> = usd
        .iter()
        .enumerate()
        .map(|(i, usd)| {
            let day = 22 + i;
            let date = if day > 31 {
                format!("2026-11-{:02}", day - 31)
            } else {
                format!("2026-10-{day}")
            };
            json!({"date": date, "start_ms": start + i as u64 * 86_400_000, "usd": usd})
        })
        .collect();
    json!(all[all.len() - days..])
}

/// The last hour or so of the busy studio: each busy
/// or recently busy ticket's history, in an order the state machine accepts.
fn activity() -> Vec<Value> {
    let histories: [(&str, &[(u64, Value)]); 12] = [
        (
            "SHOP-8",
            &[
                (100, json!({"type": "claimed"})),
                (99, json!({"type": "run_started", "run": "r1"})),
                (62, json!({"type": "checks_started"})),
                (60, json!({"type": "checks_passed"})),
                (50, json!({"type": "review_passed"})),
                (33, json!({"type": "landed"})),
            ],
        ),
        (
            "SHOP-7",
            &[
                (70, json!({"type": "claimed"})),
                (70, json!({"type": "run_started", "run": "r2"})),
                (38, json!({"type": "checks_started"})),
                (35, json!({"type": "checks_passed"})),
                (24, json!({"type": "review_passed"})),
            ],
        ),
        (
            "SHOP-12",
            &[
                (58, json!({"type": "claimed"})),
                (58, json!({"type": "run_started", "run": "r3"})),
                (45, json!({"type": "checks_started"})),
                (43, json!({"type": "checks_passed"})),
                (
                    40,
                    json!({"type": "review_failed", "detail": "design gate: the palette is too wide for the product cards"}),
                ),
                (25, json!({"type": "checks_started"})),
                (24, json!({"type": "checks_passed"})),
                (
                    20,
                    json!({"type": "review_failed", "detail": "design gate: the palette is still too wide"}),
                ),
                (
                    18,
                    json!({"type": "failed", "class": "review", "detail": "palette too wide"}),
                ),
            ],
        ),
        (
            "SHOP-2",
            &[
                (50, json!({"type": "claimed"})),
                (50, json!({"type": "run_started", "run": "r4"})),
                (30, json!({"type": "checks_started"})),
                (
                    27,
                    json!({"type": "checks_failed", "detail": "cargo test: 2 failed"}),
                ),
            ],
        ),
        (
            "EDIT-1",
            &[
                (40, json!({"type": "claimed"})),
                (40, json!({"type": "run_started", "run": "r5"})),
            ],
        ),
        (
            "SHOP-6",
            &[
                (31, json!({"type": "claimed"})),
                (31, json!({"type": "run_started", "run": "r6"})),
                (15, json!({"type": "checks_started"})),
                (9, json!({"type": "checks_passed"})),
            ],
        ),
        (
            "SHOP-5",
            &[
                (24, json!({"type": "claimed"})),
                (24, json!({"type": "run_started", "run": "r7"})),
                (2, json!({"type": "checks_started"})),
            ],
        ),
        (
            "SHOP-3",
            &[
                (18, json!({"type": "claimed"})),
                (18, json!({"type": "run_started", "run": "r8"})),
                (2, json!({"type": "ask_opened", "ask": "ASK-1"})),
            ],
        ),
        (
            "SHOP-13",
            &[
                (14, json!({"type": "claimed"})),
                (14, json!({"type": "run_started", "run": "r9"})),
                (9, json!({"type": "ask_opened", "ask": "ASK-3"})),
            ],
        ),
        (
            "SHOP-4",
            &[
                (9, json!({"type": "claimed"})),
                (9, json!({"type": "run_started", "run": "r10"})),
                (6, json!({"type": "ask_opened", "ask": "ASK-2"})),
            ],
        ),
        (
            "SHOP-15",
            &[
                (6, json!({"type": "claimed"})),
                (6, json!({"type": "run_started", "run": "r11"})),
            ],
        ),
        (
            "SHOP-10",
            &[
                (40, json!({"type": "claimed"})),
                (40, json!({"type": "run_started", "run": "r12"})),
                (14, json!({"type": "held", "reason": "budget"})),
            ],
        ),
    ];
    histories
        .iter()
        .flat_map(|(id, events)| {
            events
                .iter()
                .map(move |(ago, event)| logged(id, ago * 60, event.clone()))
        })
        .collect()
}

/// An event as studiod sends it, `seconds_ago` before now; `numbered` sets
/// its `seq`.
fn logged(id: &str, seconds_ago: u64, event: Value) -> Value {
    json!({"seq": 0, "ticket_id": id, "at_ms": now_ms() - seconds_ago * 1000, "event": event})
}

/// Events sorted oldest first and numbered.
fn numbered(mut events: Vec<Value>) -> Value {
    events.sort_by_key(|e| e["at_ms"].as_u64());
    for (seq, event) in events.iter_mut().enumerate() {
        event["seq"] = json!(seq + 1);
    }
    json!(events)
}

/// The loaded hires, with the studio's own approver and router.
fn team() -> Value {
    let member =
        |id: &str, role: &str, model: &str| json!({"id": id, "role": role, "model": model});
    json!([
        member("approver", "approver", "haiku"),
        member("design-critic", "critic", "haiku"),
        member("designer", "designer", "haiku"),
        member("dev", "engineer", "sonnet"),
        member("producer", "producer", "opus"),
        member("reviewer", "reviewer", "sonnet"),
        member("router", "router", "haiku"),
        member("writer", "writer", "sonnet"),
    ])
}

/// What happened over the last day, oldest first: the last hour's activity
/// and the work that ended before it, only of `kinds` when there are any.
fn recent_events(kinds: &[String]) -> Value {
    let ended = [
        ("SHOP-17", 20 * 3600, json!({"type": "landed"})),
        ("SHOP-18", 19 * 3600, json!({"type": "landed"})),
        ("SHOP-11", 6 * 3600, json!({"type": "landed"})),
        ("EDIT-2", 5 * 3600, json!({"type": "landed"})),
        ("SHOP-20", 2 * 3600, json!({"type": "landed"})),
        // The last try's failed checks fail the ticket; no `failed` event follows.
        (
            "SHOP-9",
            79 * 60,
            json!({"type": "checks_failed", "detail": "cargo clippy: 2 warnings"}),
        ),
    ];
    let events = ended
        .into_iter()
        .map(|(id, ago, event)| logged(id, ago, event))
        .chain(activity())
        .filter(|e| kinds.is_empty() || kinds.iter().any(|k| e["event"]["type"] == *k))
        .collect();
    numbered(events)
}

/// A studio with nothing going on: no tickets, so the board is empty, the
/// desks are free and the team rests.
fn idle_reply(request: &Request) -> Value {
    match request {
        Request::Status => json!({
            "asks": 0, "running": 0, "tickets": {}, "goals": 0, "rules": 3, "daily_usd": 10.0,
            "spent_24h": 0.0, "projects": ["shop", "editor"],
            "hires": ["dev", "designer", "producer", "writer", "design-critic", "reviewer", "approver", "router"],
            "max_runs": 6, "closed": false, "team": team(),
        }),
        Request::Budget => {
            let mut idle = budget();
            idle["spent_24h"] = json!({"total": 0.0, "by_project": {}, "by_hire": {}, "by_model": {},
                                       "by_ticket": {}, "runs_by_hire": {}});
            idle["runs_24h"] = json!(0);
            idle["reserved"] = json!({});
            idle
        }
        // Nothing has landed yet, so there is nothing to win.
        Request::Trophies => json!([]),
        Request::ProjectChecks => {
            let mut checks = project_checks();
            for check in checks.as_array_mut().into_iter().flatten() {
                check["running"] = json!([]);
                check["latest"] = Value::Null;
            }
            checks
        }
        Request::Tickets
        | Request::Asks
        | Request::Estimates
        | Request::MergedTickets
        | Request::RecentEvents { .. }
        | Request::Activity { .. } => json!([]),
        Request::TicketSpends | Request::StateSince | Request::HiresToday => json!({}),
        Request::SpendByDay { days } => spend_by_day(*days, 0.0),
        _ => reply(request),
    }
}

/// A new studio: the starter team of five, and no projects, tickets, goals,
/// rules or trophies. The floor is a bare room.
fn empty_reply(request: &Request) -> Value {
    let member =
        |id: &str, role: &str, model: &str| json!({"id": id, "role": role, "model": model});
    match request {
        Request::Status => json!({
            "asks": 0, "running": 0, "tickets": {}, "goals": 0, "rules": 0, "daily_usd": 10.0,
            "spent_24h": 0.0, "projects": [],
            "hires": ["engineer", "producer", "reviewer", "approver", "router"],
            "max_runs": 3, "closed": false,
            "team": [
                member("engineer", "engineer", "sonnet"),
                member("producer", "producer", "sonnet"),
                member("reviewer", "reviewer", "opus"),
                member("approver", "approver", "haiku"),
                member("router", "router", "haiku"),
            ],
        }),
        Request::ProjectChecks | Request::RuleFiles => json!([]),
        _ => idle_reply(request),
    }
}

/// The busy studio after the owner closed the office: the same work, and
/// nothing new starts.
fn closed_reply(request: &Request) -> Value {
    let mut out = reply(request);
    if matches!(request, Request::Status) {
        out["closed"] = json!(true);
    }
    out
}

/// A ticket's history: a run sent back once, then a second run. Some
/// tickets also asked, were held, or began their run at another time.
fn events(id: &str) -> Value {
    let at = |minutes_ago: u64| now_ms() - minutes_ago * 60_000;
    if id == "SHOP-3" {
        return json!([
            {"seq": 1, "ticket_id": id, "at_ms": at(21), "event": {"type": "created", "id": id,
             "project": "shop", "title": "Add a cart drawer", "goal": "Checkout flow", "hire": "dev",
             "base_branch": "main", "created_at": at(21)}},
            {"seq": 2, "ticket_id": id, "at_ms": at(18), "event": {"type": "claimed"}},
            {"seq": 3, "ticket_id": id, "at_ms": at(18), "event": {"type": "run_started", "run": "r1"}},
            {"seq": 4, "ticket_id": id, "at_ms": at(2), "event": {"type": "ask_opened", "ask": "ASK-1"}},
        ]);
    }
    if id == "SHOP-9" {
        // Three tries, each stopped by the checks; the third fails the ticket.
        return json!([
            {"seq": 1, "ticket_id": id, "at_ms": at(106), "event": {"type": "claimed"}},
            {"seq": 2, "ticket_id": id, "at_ms": at(96), "event": {"type": "checks_started"}},
            {"seq": 3, "ticket_id": id, "at_ms": at(95),
             "event": {"type": "checks_failed", "detail": "cargo test: 1 failed"}},
            {"seq": 4, "ticket_id": id, "at_ms": at(88), "event": {"type": "checks_started"}},
            {"seq": 5, "ticket_id": id, "at_ms": at(87),
             "event": {"type": "checks_failed", "detail": "cargo clippy: 4 warnings"}},
            {"seq": 6, "ticket_id": id, "at_ms": at(80), "event": {"type": "checks_started"}},
            {"seq": 7, "ticket_id": id, "at_ms": at(79),
             "event": {"type": "checks_failed", "detail": "cargo clippy: 2 warnings"}},
        ]);
    }
    let run = match id {
        "SHOP-5" => 12,
        "SHOP-6" => 18,
        "EDIT-1" => 8,
        _ => 20,
    };
    let mut events = json!([
        {"seq": 1, "ticket_id": id, "at_ms": at(42), "event": {"type": "claimed"}},
        {"seq": 2, "ticket_id": id, "at_ms": at(30), "event": {"type": "checks_started"}},
        {"seq": 3, "ticket_id": id, "at_ms": at(24),
         "event": {"type": "checks_failed", "detail": "cargo test: 1 failed"}},
        {"seq": 4, "ticket_id": id, "at_ms": at(22),
         "event": {"type": "review_failed",
                   "detail": "**Must fix:** the `Sign out` button signs out at once.\nAsk first, then sign out."}},
        {"seq": 5, "ticket_id": id, "at_ms": at(21),
         "event": {"type": "changes_requested",
                   "detail": "Keep **Esc** as cancel:\n\n- close the dialog\n- keep the `session` cookie"}},
        {"seq": 6, "ticket_id": id, "at_ms": at(run), "event": {"type": "claimed"}},
        {"seq": 7, "ticket_id": id, "at_ms": at(9), "event": {"type": "checks_passed"}},
    ]);
    let extra = match id {
        "SHOP-4" => json!({"type": "ask_opened", "ask": "ASK-2"}),
        "SHOP-13" => json!({"type": "ask_opened", "ask": "ASK-3"}),
        "SHOP-10" => json!({"type": "held", "reason": "budget"}),
        _ => return events,
    };
    let minutes = match id {
        "SHOP-13" => 9,
        _ => 6,
    };
    if let Some(list) = events.as_array_mut() {
        list.push(json!({"seq": 8, "ticket_id": id, "at_ms": at(minutes), "event": extra}));
    }
    events
}

/// When each running ticket's run began and each held ticket was held, from
/// their histories.
fn state_since() -> Value {
    let mut since = serde_json::Map::new();
    for ticket in tickets().as_array().into_iter().flatten() {
        let state = &ticket["state"];
        let kind = state
            .as_str()
            .or_else(|| state.as_object()?.keys().next().map(String::as_str))
            .unwrap_or_default();
        let wanted = match kind {
            "claimed" | "working" | "checking" | "reviewing" => "claimed",
            "held" => "held",
            _ => continue,
        };
        let Some(id) = ticket["id"].as_str() else {
            continue;
        };
        let history = events(id);
        let at = history
            .as_array()
            .into_iter()
            .flatten()
            .rfind(|e| e["event"]["type"] == wanted)
            .map(|e| e["at_ms"].clone());
        if let Some(at) = at {
            since.insert(id.to_string(), at);
        }
    }
    Value::Object(since)
}

/// What each ticket spent in the last 24 hours, in USD. The totals below
/// are all worked out from these, so the money always agrees.
const SPENT_BY_TICKET: [(&str, f64); 11] = [
    ("SHOP-9", 1.40),
    ("SHOP-7", 1.20),
    ("SHOP-6", 1.00),
    ("SHOP-5", 0.70),
    ("SHOP-20", 0.62),
    ("SHOP-2", 0.38),
    ("SHOP-12", 0.30),
    ("SHOP-10", 0.20),
    ("SHOP-11", 0.12),
    ("EDIT-1", 0.30),
    ("EDIT-2", 0.18),
];

/// The same 24 hours by hire. Adds up to `spent_24h()`.
const SPENT_BY_HIRE: [(&str, f64); 6] = [
    ("dev", 4.4),
    ("designer", 0.6),
    ("producer", 0.6),
    ("reviewer", 0.5),
    ("approver", 0.2),
    ("router", 0.1),
];

/// What live runs hold: the three working, the one in review and the three
/// paused on an ask.
const RESERVED: [(&str, f64); 7] = [
    ("SHOP-2", 0.50),
    ("SHOP-15", 0.25),
    ("EDIT-1", 0.25),
    ("SHOP-6", 0.30),
    ("SHOP-3", 0.50),
    ("SHOP-4", 0.50),
    ("SHOP-13", 0.25),
];

/// Cents, so sums print the way they were written.
fn cents(usd: f64) -> f64 {
    (usd * 100.0).round() / 100.0
}

/// What the studio spent in the last 24 hours: the sum over tickets.
fn spent_24h() -> f64 {
    cents(SPENT_BY_TICKET.iter().map(|(_, usd)| usd).sum())
}

fn pairs(list: &[(&str, f64)]) -> Value {
    json!(
        list.iter()
            .copied()
            .collect::<std::collections::BTreeMap<_, _>>()
    )
}

/// The budget: limits, the last 24 hours split by project, hire, model and
/// ticket (each adding up to `spent_24h()`), and what live runs hold.
fn budget() -> Value {
    let by_project = |editor: bool| {
        cents(
            SPENT_BY_TICKET
                .iter()
                .filter(|(id, _)| id.starts_with("EDIT") == editor)
                .map(|(_, usd)| usd)
                .sum(),
        )
    };
    json!({
        "limits": {"daily_usd": 10.0, "ticket_usd": 2.0, "run_usd": null, "min_run_usd": 0.05,
                   "projects": {"shop": 6.0, "editor": 4.0}, "hires": {"dev": 8.0, "designer": 2.0}},
        "spent_24h": {"total": spent_24h(),
                      "by_project": {"shop": by_project(false), "editor": by_project(true)},
                      "by_hire": pairs(&SPENT_BY_HIRE),
                      "by_model": {"sonnet": 4.9, "haiku": 0.9, "opus": 0.6},
                      "by_ticket": pairs(&SPENT_BY_TICKET),
                      "runs_by_hire": {"dev": 9, "designer": 2, "producer": 1, "reviewer": 3,
                                       "approver": 41, "router": 6}},
        "runs_24h": 62,
        "reserved": pairs(&RESERVED),
    })
}

/// Epoch milliseconds as a plain date, such as `30 Oct 2026`.
fn plain_date(ms: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Days since 1970-01-01 to a civil date.
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{day} {} {year}", MONTHS[(month - 1) as usize % 12])
}

/// What the studio has won, as `Trophies` answers: a streak, a best day,
/// the first goal merged, ten tickets landed, and the next landed milestone
/// with its progress. The 11 landed tickets fell on four days in a row: 2, 4
/// (the best), 2 and 3 today.
fn trophies() -> Value {
    let hours = |h: u64| now_ms() - h * 3_600_000;
    json!([
        {"id": "streak", "title": "4-day streak", "detail": "Tickets landed on 4 days in a row.",
         "earned_at": now_ms() - 33 * 60_000, "progress": null},
        {"id": "best-day", "title": "Best day",
         "detail": format!("Landed 4 tickets on {}.", plain_date(hours(47))),
         "earned_at": hours(47), "progress": null},
        {"id": "first-goal-merged", "title": "First goal merged",
         "detail": "Polish the navigation was merged into main.",
         "earned_at": hours(18), "progress": null},
        {"id": "landed-10", "title": "10 tickets landed",
         "detail": "The studio has landed 10 tickets.", "earned_at": hours(2), "progress": null},
        {"id": "landed-25", "title": "25 tickets landed", "detail": "14 more to land.",
         "earned_at": null, "progress": [11, 25]},
    ])
}

/// Each project's checks as `ProjectChecks` answers: the commands, the
/// tickets in their checks now (as the board shows), and the newest result.
fn project_checks() -> Value {
    let command = |words: &[&str]| json!(words);
    json!([
        {"project": "shop",
         "verify": [command(&["cargo", "fmt", "--all", "--check"]),
                    command(&["cargo", "clippy", "--workspace", "--", "-D", "warnings"]),
                    command(&["cargo", "test", "--workspace"])],
         "running": ["SHOP-5"],
         "latest": {"ticket": "SHOP-6", "passed": true, "at_ms": now_ms() - 9 * 60_000}},
        {"project": "editor",
         "verify": [command(&["cargo", "fmt", "--check"]), command(&["cargo", "test"])],
         "running": [],
         "latest": {"ticket": "EDIT-2", "passed": true, "at_ms": now_ms() - 310 * 60_000}},
    ])
}

fn reply(request: &Request) -> Value {
    let at = |minutes_ago: u64| now_ms() - minutes_ago * 60_000;
    match request {
        Request::Status => json!({
            // Runs paused on an ask hold their slots too.
            "asks": 3, "running": 8, "goals": 6, "rules": 5,
            "tickets": {"queued": 5, "working": 3, "checking": 1, "reviewing": 1, "asking": 3,
                        "held": 1, "ready": 1, "landed": 11, "failed": 2},
            "daily_usd": 10.0, "spent_24h": spent_24h(), "projects": ["shop", "editor"],
            "hires": ["dev", "designer", "producer", "writer", "design-critic", "reviewer", "approver", "router"],
            "max_runs": 9, "closed": false, "team": team(),
        }),
        Request::RecentEvents { kinds, .. } => recent_events(kinds),
        // Adding a project and hiring answer as studiod does.
        Request::AddProject { path, name, branch } => json!({
            "name": name.clone().unwrap_or_else(|| "project".into()), "path": path,
            "branch": branch.clone().unwrap_or_else(|| "main".into()),
            "checks": format!("{path}/.studio.toml"),
        }),
        Request::SaveFile { name, .. } => json!({"name": name}),
        Request::SpendByDay { days } => spend_by_day(*days, spent_24h()),
        Request::Tickets => tickets(),
        Request::Events {
            ticket: Some(id), ..
        } => events(id),
        Request::Activity { .. } => numbered(activity()),
        Request::StateSince => state_since(),
        Request::Diff { ticket } if ticket == "SHOP-3" => json!(
            "diff --git a/src/cart.rs b/src/cart.rs\n--- a/src/cart.rs\n+++ b/src/cart.rs\n@@ -1 +1,5 @@\n-pub type Cart = Vec<(ProductId, u32)>;\n+pub type Cart = Vec<Line>;\n+\n+pub fn total(cart: &Cart) -> u32 {\n+    cart.iter().map(|line| line.price * line.quantity).sum()\n+}\ndiff --git a/templates/cart.html b/templates/cart.html\n--- a/templates/cart.html\n+++ b/templates/cart.html\n@@ -10,2 +10,2 @@\n-<main class=\"cart-page\">\n-  <h1>Your cart</h1>\n+<aside class=\"cart-drawer\" aria-label=\"Cart\">\n+  <h2>Your cart: {{ total }}</h2>\n"
        ),
        Request::Diff { .. } => json!(
            "diff --git a/src/landing.rs b/src/landing.rs\n--- a/src/landing.rs\n+++ b/src/landing.rs\n@@ -4,3 +4,7 @@ pub fn page(html: &mut Html, user: Option<&User>) {\n     html.heading(\"Corner Shop\");\n-    html.text(\"Opening soon\");\n+    html.link(\"Shop now\", \"/products\");\n+    if let Some(user) = user {\n+        html.text(&format!(\"Signed in as {}\", user.name));\n+        html.button(\"Sign out\", \"/sign-out\");\n+    }\n }\n"
        ),
        Request::Diffstat { .. } => json!(DIFFSTAT),
        Request::Diffstats { tickets } => json!(
            tickets
                .iter()
                .map(|t| (t.clone(), DIFFSTAT))
                .collect::<std::collections::BTreeMap<_, _>>()
        ),
        // Nothing is booked for a run that has only just started.
        Request::TicketSpend { ticket } if matches!(ticket.as_str(), "SHOP-2" | "SHOP-3") => {
            json!(0.0)
        }
        Request::TicketSpend { .. } => json!(0.31),
        Request::TicketSpends => json!({
            "EDIT-2": 0.18, "SHOP-8": 0.84, "SHOP-4": 0.11, "SHOP-11": 0.31, "SHOP-12": 0.84, "SHOP-13": 0.22, "SHOP-14": 0.12, "SHOP-15": 0.09,
            "SHOP-16": 0.11, "SHOP-17": 2.06, "SHOP-18": 0.84, "SHOP-19": 0.08, "SHOP-20": 1.12,
            "SHOP-2": 0.38, "SHOP-5": 0.91, "SHOP-6": 1.24, "SHOP-7": 2.06, "SHOP-9": 1.96,
            "SHOP-10": 0.52, "SHOP-21": 0.07, "SHOP-22": 0.35, "EDIT-1": 0.3,
        }),
        // When tickets last changed; the rest are left out.
        Request::UpdatedAt => json!({
            "EDIT-1": at(4), "EDIT-2": at(26), "SHOP-13": at(2), "SHOP-15": at(1),
            "SHOP-16": at(1500), "SHOP-18": at(1450), "SHOP-19": at(70), "SHOP-20": at(65),
            "SHOP-21": at(300), "SHOP-22": at(290),
        }),
        // The navigation goal is merged; the save-carts goal is done but not merged.
        Request::MergedTickets => json!(["SHOP-17", "SHOP-18"]),
        Request::Asks => json!([{
            "id": "ASK-1", "ticket": "SHOP-3", "hire": "dev", "kind": "permission", "at_ms": at(2),
            "tool": "Bash",
            "input": {"command": "cargo run -- --port 8080", "description": "Try the cart drawer"},
            "reasons": ["Approver: starting the store opens port 8080 on this machine, so it is yours to decide"],
        }, {
            "id": "ASK-2", "ticket": "SHOP-4", "hire": "dev", "kind": "permission", "at_ms": at(6),
            "tool": "Edit",
            "input": {"file_path": "src/payments/retry.rs",
                      "old_string": "const MAX_RETRIES: u32 = 1;",
                      "new_string": "const MAX_RETRIES: u32 = 3;"},
            "reasons": ["Payment code moves real money: ask before changing it (protect-payments)"],
        }, {
            "id": "ASK-3", "ticket": "SHOP-13", "hire": "writer", "kind": "permission", "at_ms": at(9),
            "tool": "Bash",
            "input": {"command": "cargo run --bin mailer -- send --template templates/email/launch.html --to owner@example.com --subject \"Corner Shop is open\""},
            "reasons": [],
        }]),
        Request::Budget => budget(),
        Request::Trophies => trophies(),
        Request::ProjectChecks => project_checks(),
        Request::Files {
            kind: FileKind::Hires,
        } => json!([
            hire_file("dev", "engineer", "sonnet", "opus", "Writes code, with tests, in small steps."),
            hire_file("designer", "designer", "sonnet", "", "Page layouts, images and icons."),
            hire_file("producer", "producer", "sonnet", "", "Plans goals as small tickets."),
            hire_file("design-critic", "critic", "opus", "sonnet", "Design critic for pages and images."),
            hire_file("reviewer", "reviewer", "opus", "sonnet", "Reads every diff before it lands."),
            {"name": "writer", "problem": "line 3: did not find expected key", "text": ""},
            hire_file("approver", "approver", "haiku", "", "Decides tool calls no rule covers."),
            hire_file("router", "router", "haiku", "", "Picks the hire for new tickets."),
        ]),
        Request::HiresToday => json!({
            "dev": {"done": 6}, "designer": {"done": 1}, "producer": {"planned": 2},
            "design-critic": {"critiques": 4}, "reviewer": {"reviews": 11},
            "approver": {"allowed": 81, "denied": 11, "to_owner": 4}, "router": {"routed": 12},
        }),
        Request::ApproverCalls { limit } => {
            let call = |ticket: &str,
                        minutes: u64,
                        tool: &str,
                        what: &str,
                        verdict: &str,
                        reason: &str| {
                json!({"ticket": ticket, "at": at(minutes), "tool": tool, "what": what,
                       "verdict": verdict, "reason": reason})
            };
            let calls = [
                call(
                    "SHOP-3",
                    2,
                    "Bash",
                    "cargo run -- --port 8080",
                    "to_owner",
                    "Starting the store opens port 8080 on this machine, so it is yours to decide.",
                ),
                call(
                    "SHOP-2",
                    4,
                    "Bash",
                    "cargo test -p shop search::cache",
                    "allowed",
                    "Runs the search tests inside the worktree.",
                ),
                call(
                    "SHOP-5",
                    7,
                    "Bash",
                    "cargo clippy --all-targets -- -D warnings",
                    "allowed",
                    "Lints the worktree; nothing leaves it.",
                ),
                call(
                    "SHOP-2",
                    12,
                    "Bash",
                    "rm -rf ~/.cargo/registry/cache",
                    "denied",
                    "Clearing the shared Cargo cache is outside the ticket and slows every other build.",
                ),
                call(
                    "SHOP-5",
                    15,
                    "WebFetch",
                    "https://doc.rust-lang.org/std/fs/fn.read_dir.html",
                    "allowed",
                    "Reads public documentation.",
                ),
                call(
                    "SHOP-6",
                    18,
                    "Bash",
                    "git diff --stat main...HEAD",
                    "allowed",
                    "Reads the branch's own changes.",
                ),
                call(
                    "SHOP-6",
                    21,
                    "Bash",
                    "cargo test --workspace",
                    "allowed",
                    "Runs the tests inside the worktree.",
                ),
                call(
                    "SHOP-5",
                    26,
                    "Bash",
                    "cargo run --release -- resize-images static/img/products",
                    "allowed",
                    "The ticket's resize step, writing inside the worktree.",
                ),
            ];
            json!(calls[..calls.len().min(*limit)])
        }
        Request::RuleFiles => json!([
            rule_file(
                "studio",
                "",
                "no-push",
                "kind: gate\nlevel: block\naction: \"bash:git push\"",
                "The studio lands work; agents never push."
            ),
            rule_file(
                "studio",
                "",
                "read-dependencies",
                "kind: gate\nlevel: allow\naction: \"read:~/.cargo/registry/**\"",
                "Agents may read the source of Rust dependencies."
            ),
            rule_file(
                "studio",
                "",
                "house-style",
                "kind: guide\nlevel: warn",
                "Read CONTRIBUTING.md before you start."
            ),
            rule_file(
                "project",
                "shop",
                "protect-payments",
                "kind: protect\nlevel: ask\nwhen:\n  paths: [\"src/payments/**\"]",
                "Payment code moves real money. Ask before\nchanging anything under src/payments."
            ),
            rule_file(
                "project",
                "shop",
                "run-the-checks",
                "kind: check\nlevel: block\ncommand: [cargo, test, --workspace]",
                "fmt, clippy and tests must pass."
            ),
        ]),
        _ => Value::Null,
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply.
fn fake_studiod(socket: &Path, reply: fn(&Request) -> Value) -> Result<()> {
    let listener = UnixListener::bind(socket)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                if BufReader::new(read).read_line(&mut line).await.is_err() {
                    return;
                }
                let Ok(request) = serde_json::from_str::<Request>(&line) else {
                    return;
                };
                let Ok(mut out) = serde_json::to_string(&Response::ok(reply(&request))) else {
                    return;
                };
                out.push('\n');
                let _ = write.write_all(out.as_bytes()).await;
            });
        }
    });
    Ok(())
}

/// Run logs for the tickets whose pages are photographed.
fn write_logs(runs: &Path) -> Result<()> {
    std::fs::create_dir_all(runs)?;
    // One line per entry, 25 seconds apart, the last just now.
    let lines = |entries: &[Value]| {
        let now = now_ms();
        let mut text = String::new();
        for (i, entry) in entries.iter().enumerate() {
            let mut entry = entry.clone();
            entry["at"] = json!(now - (entries.len() - i) as u64 * 25_000);
            text.push_str(&entry.to_string());
            text.push('\n');
        }
        text
    };
    let text = |text: &str| json!({"event": {"type": "text", "text": text}});
    let call = |name: &str, input: Value| json!({"event": {"type": "tool_call", "id": "t", "name": name, "input": input}});
    let fin = json!({"event": {"type": "finished", "outcome": {"outcome": "success"},
                               "cost": {"usd": 0.31}}});
    // A long run: the agent's words between bursts of the same few tools.
    let rule = json!({"note": "allowed Edit: inside the worktree"});
    let read = |file: &str| {
        call(
            "Read",
            json!({"file_path": format!("src/search/{file}.rs")}),
        )
    };
    let edit = |file: &str| {
        call(
            "Edit",
            json!({"file_path": format!("src/search/{file}.rs")}),
        )
    };
    let bash = |command: &str| call("Bash", json!({"command": command}));
    let mut long = vec![
        text(
            "Repeat searches miss the cache and run the full query again. Let me look at the search code.",
        ),
        read("cache"),
        read("query"),
        read("index"),
        read("rank"),
        call("Grep", json!({"pattern": "cache_key"})),
        call("Grep", json!({"pattern": "CACHE_SIZE"})),
        text(
            "The cache is keyed by the raw query, so `Mugs` and `mugs` miss each other. I'll key it by the trimmed, lowercased words instead.",
        ),
        edit("cache"),
        rule.clone(),
        edit("query"),
        rule.clone(),
        edit("index"),
        rule.clone(),
        bash("cargo test cache"),
        bash(
            "cargo test --workspace --locked -- --test-threads=1 --nocapture search::cache::same_words_hit_the_cache",
        ),
        text("Two tests fail: the cache is not cleared when a product's price changes."),
        read("cache"),
        read("rank"),
        edit("cache"),
        rule.clone(),
        bash("cargo test cache"),
        text("Repeat searches now come back from the cache in under 5ms."),
        call("Glob", json!({"pattern": "tests/**/*.rs"})),
        read("cache"),
        edit("cache"),
        rule.clone(),
        edit("index"),
        rule.clone(),
        bash("cargo fmt --all"),
        bash("cargo clippy --workspace --all-targets -- -D warnings"),
        text("Formatting and lints are clean. Running the full test suite."),
        read("query"),
        read("cache"),
        bash("cargo test --workspace"),
    ];
    long.push(bash("cargo test --workspace --release"));
    long.push(json!({"event": {"type": "permission", "tool": "Bash"}}));
    long.push(json!({"note": "allowed Bash: cargo test (rule allow-cargo-test)"}));
    std::fs::write(runs.join("SHOP-2.jsonl"), lines(&long))?;
    // A run that stopped to ask: its message, the steps after it with their
    // results, and the words just before the ask.
    let ago = |minutes: u64, mut entry: Value| {
        entry["at"] = json!(now_ms() - minutes * 60_000);
        entry
    };
    let numbered = |n: usize| {
        (0..n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let id_call = |id: &str, name: &str, input: Value| json!({"event": {"type": "tool_call", "id": id, "name": name, "input": input}});
    let output = |id: &str, output: String| json!({"event": {"type": "tool_result", "id": id, "output": output, "is_error": false}});
    let drawer = [
        ago(20, json!({"note": "routed to dev: cart logic with tests"})),
        ago(
            18,
            json!({"event": {"type": "started", "session": "s1", "model": "sonnet"}}),
        ),
        ago(
            18,
            text(
                "Adding an item sends the shopper to `/cart`, away from the product. I'll put prices on the cart's lines in `cart.rs` and turn `cart.html` into a drawer:\n\n- `Line`: a product with its price and quantity\n- `total(cart)`: the sum of every line\n- `cart-drawer`: the lines and total, opened in place",
            ),
        ),
        ago(
            17,
            id_call("r1", "Read", json!({"file_path": "templates/cart.html"})),
        ),
        ago(17, output("r1", numbered(142))),
        ago(
            17,
            id_call("r2", "Read", json!({"file_path": "src/cart.rs"})),
        ),
        ago(17, output("r2", numbered(310))),
        ago(16, id_call("g1", "Grep", json!({"pattern": "add_to_cart"}))),
        ago(
            16,
            output(
                "g1",
                "Found 3 files\nsrc/cart.rs\nsrc/routes.rs\ntemplates/product.html".into(),
            ),
        ),
        ago(
            14,
            id_call(
                "e1",
                "Edit",
                json!({"file_path": "src/cart.rs",
            "old_string": numbered(3), "new_string": numbered(48).replace("line", "row")}),
            ),
        ),
        ago(
            12,
            id_call(
                "e2",
                "Edit",
                json!({"file_path": "templates/cart.html",
            "old_string": numbered(22), "new_string": numbered(31).replace("line", "item")}),
            ),
        ),
        ago(
            8,
            id_call(
                "b1",
                "Bash",
                json!({"command": "cargo test --locked cart::"}),
            ),
        ),
        ago(
            8,
            json!({"event": {"type": "permission", "tool": "Bash", "input": {"command": "cargo test --locked cart::"}}}),
        ),
        ago(
            8,
            json!({"note": "approver allowed Bash: tests run inside the worktree"}),
        ),
        ago(
            7,
            output(
                "b1",
                "running 12 tests\ntest result: ok. 12 passed; 0 failed; 0 ignored".into(),
            ),
        ),
        ago(
            3,
            text("The tests pass. Before I hand it over I'd like to try the drawer in a browser."),
        ),
        ago(
            2,
            id_call("b2", "Bash", json!({"command": "cargo run -- --port 8080"})),
        ),
        ago(
            2,
            json!({"event": {"type": "permission", "tool": "Bash", "input": {"command": "cargo run -- --port 8080"}}}),
        ),
    ];
    let mut log = String::new();
    for entry in drawer {
        log.push_str(&entry.to_string());
        log.push('\n');
    }
    std::fs::write(runs.join("SHOP-3.jsonl"), log)?;
    // Checks under way, and a review: notes at fixed times before now.
    let at = |seconds_ago: u64, entry: Value| {
        let mut entry = entry;
        entry["at"] = json!(now_ms() - seconds_ago * 1000);
        format!("{entry}\n")
    };
    let note = |text: &str| json!({"note": text});
    std::fs::write(
        runs.join("SHOP-5.jsonl"),
        [
            at(
                400,
                text("Product images are resized to three widths now. Handing over to the checks."),
            ),
            at(116, note("check: cargo fmt --all --check")),
            at(112, note("check: cargo clippy --workspace -- -D warnings")),
            at(74, note("check: cargo test --workspace")),
        ]
        .concat(),
    )?;
    std::fs::write(
        runs.join("SHOP-6.jsonl"),
        [
            at(560, note("checks passed")),
            at(540, note("review by reviewer (sonnet)")),
            at(
                30,
                call("Read", json!({"file_path": "src/orders/history.rs"})),
            ),
        ]
        .concat(),
    )?;
    std::fs::write(
        runs.join("SHOP-7.jsonl"),
        lines(&[
            text(
                "Added a **landing page** with a Shop now link and a Sign out button:\n\n- `src/landing.rs` renders the page\n- `main.rs` serves it at `/`\n\n```rust\nrouter.get(\"/\", landing::page);\n```",
            ),
            call("Write", json!({"file_path": "src/landing.rs"})),
            call("Edit", json!({"file_path": "src/main.rs"})),
            call("Bash", json!({"command": "cargo test --workspace"})),
            json!({"note": "checks passed"}),
            json!({"note": "review by reviewer (opus)"}),
            json!({"note": "review passed"}),
            json!({"note": "gate design by design-critic (opus)"}),
            json!({"note": "gate design passed"}),
            fin,
        ]),
    )?;
    std::fs::write(
        runs.join("SHOP-6.jsonl"),
        lines(&[
            call("Write", json!({"file_path": "src/orders.rs"})),
            json!({"note": "checks passed"}),
            json!({"note": "review by reviewer (opus)"}),
            call("Read", json!({"file_path": "src/orders.rs"})),
        ]),
    )?;
    std::fs::write(
        runs.join("EDIT-1.jsonl"),
        lines(&[
            text(
                "The editor needs a form for new items, undo and saving. I'll file one ticket for each.",
            ),
            call(
                "mcp__studio__file_ticket",
                json!({"title": "Save drafts as JSON files"}),
            ),
        ]),
    )?;
    Ok(())
}

/// The first Chromium browser found, if any.
fn find_browser() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("STUDIO_BROWSER").filter(|p| !p.is_empty()) {
        return Some(path.into());
    }
    let apps = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    if let Some(app) = apps.iter().map(PathBuf::from).find(|p| p.exists()) {
        return Some(app);
    }
    let path = std::env::var_os("PATH")?;
    ["google-chrome", "chromium", "chromium-browser"]
        .iter()
        .flat_map(|name| std::env::split_paths(&path).map(move |dir| dir.join(name)))
        .find(|p| p.is_file())
}

/// Loads `url` in the browser and saves a screenshot of it. Pages keep a
/// live connection open, so a headless browser writes the screenshot but
/// may never exit: it is stopped once the file has stopped growing.
async fn shoot(
    browser: &Path,
    profile: &Path,
    url: &str,
    size: (u32, u32),
    out: &Path,
) -> Result<()> {
    // An old file must not pass for a new screenshot.
    let _ = std::fs::remove_file(out);
    let mut child = tokio::process::Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--timeout=4000",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={},{}", size.0, size.1))
        .arg(format!("--screenshot={}", out.display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", browser.display()))?;
    let size_of = || std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    let started = std::time::Instant::now();
    let mut last = 0;
    while started.elapsed() < PAGE_LIMIT {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!("the browser failed on {url}: {status}").into());
            }
            if size_of() == 0 {
                return Err(format!("the browser wrote no screenshot of {url}").into());
            }
            return Ok(());
        }
        let now = size_of();
        if now > 0 && now == last {
            let _ = child.kill().await;
            return Ok(());
        }
        last = now;
    }
    let _ = child.kill().await;
    Err(format!("{url} took longer than {}s", PAGE_LIMIT.as_secs()).into())
}

/// How pages become pictures.
enum Renderer {
    Browser(PathBuf),
    QuickLook(Inliner),
}

/// Makes a page self-contained for Quick Look, which loads nothing: the
/// stylesheet goes inline, with the fonts in it as data URLs.
struct Inliner {
    css: String,
}

impl Inliner {
    async fn new(base: &str) -> Result<Inliner> {
        let client = reqwest::Client::new();
        let mut css = client
            .get(format!("{base}/static/app.css"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        for font in [
            "ibm-plex-sans-var",
            "ibm-plex-mono-400",
            "ibm-plex-mono-500",
        ] {
            let path = format!("/static/fonts/{font}.woff2");
            let bytes = client
                .get(format!("{base}{path}"))
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            css = css.replace(&path, &format!("data:font/woff2;base64,{}", base64(&bytes)));
        }
        Ok(Inliner { css })
    }

    /// The page at `url`, with the stylesheet inline and `theme` forced.
    async fn page(&self, url: &str, theme: Option<&str>) -> Result<String> {
        let html = reqwest::Client::new()
            .get(url)
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let start = html
            .find("<link rel=\"stylesheet\"")
            .ok_or("the page has no stylesheet")?;
        let end = start
            + html[start..]
                .find('>')
                .ok_or("the stylesheet link isn't closed")?
            + 1;
        let page = format!(
            "{}<style>{}</style>{}",
            &html[..start],
            self.css,
            &html[end..]
        );
        Ok(match theme {
            Some(theme) => page.replacen("<html", &format!("<html data-theme=\"{theme}\""), 1),
            None => page,
        })
    }
}

/// Standard base64, for the fonts' data URLs.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn attribute_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders `page` with Quick Look at a CSS `width`. Quick Look lays pages
/// out in a 1024px square, so the page goes in an iframe of `width` (media
/// queries see that width), scaled down to fit when it is wider. A narrower
/// page is cut to its own width.
async fn quick_look(page: &str, width: u32, scratch: &Path, out: &Path) -> Result<()> {
    const SIDE: u32 = 1024;
    let scale = (f64::from(SIDE) / f64::from(width)).min(1.0);
    let height = (f64::from(SIDE) / scale) as u32;
    // A narrow frame sits one pixel in, since `sips` crops from (1, 1) at the least.
    let inset = if width < SIDE { "1px 0 0 1px" } else { "0" };
    let frame = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;background:#2a2a2e;overflow:hidden}}\
         iframe{{border:0;display:block;margin:{inset};width:{width}px;height:{height}px;\
         transform:scale({scale});transform-origin:0 0}}\
         </style></head><body><iframe srcdoc=\"{}\"></iframe></body></html>",
        attribute_escape(page)
    );
    let name = out.file_stem().ok_or("bad output name")?.to_string_lossy();
    let file = scratch.join(format!("{name}.html"));
    std::fs::write(&file, frame)?;
    let status = tokio::process::Command::new("qlmanage")
        .args(["-t", "-s", &SIDE.to_string(), "-o"])
        .arg(scratch)
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    let status = tokio::time::timeout(PAGE_LIMIT, status)
        .await
        .map_err(|_| {
            format!(
                "Quick Look took longer than {}s on {name}",
                PAGE_LIMIT.as_secs()
            )
        })??;
    let made = scratch.join(format!("{name}.html.png"));
    if !status.success() || !made.exists() {
        return Err(format!("Quick Look made no picture of {name}").into());
    }
    if width < SIDE {
        let status = tokio::process::Command::new("sips")
            .args(["-c", &(SIDE - 1).to_string(), &width.to_string()])
            .args(["--cropOffset", "1", "1"])
            .arg(&made)
            .arg("--out")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            return Err(format!("sips could not crop {name}").into());
        }
        std::fs::remove_file(made)?;
    } else {
        std::fs::rename(made, out)?;
    }
    Ok(())
}

fn has_quick_look() -> bool {
    Path::new("/usr/bin/qlmanage").exists()
}

/// Serves the web UI against a stand-in studiod that answers with `reply`;
/// returns its base URL.
async fn serve(dir: &Path, name: &str, reply: fn(&Request) -> Value) -> Result<String> {
    let socket = dir.join(format!("{name}.sock"));
    fake_studiod(&socket, reply)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let config = AppConfig {
        socket,
        data_dir: dir.to_path_buf(),
        token: TOKEN.into(),
        base_url: base.clone(),
    };
    tokio::spawn(async move { topcoat::serve(listener, router(config)).await });
    Ok(base)
}

async fn run(out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;

    let dir = tempfile::tempdir()?;
    write_logs(&dir.path().join("runs"))?;
    let busy = serve(dir.path(), "busy", reply).await?;
    let idle = serve(dir.path(), "idle", idle_reply).await?;
    let empty = serve(dir.path(), "empty", empty_reply).await?;
    let closed = serve(dir.path(), "closed", closed_reply).await?;

    // One profile for every page, so the login cookie carries over (cookies
    // ignore the port). Logging in also shows whether the browser works here.
    let profile = dir.path().join("profile");
    let mut renderer = None;
    if let Some(browser) = find_browser() {
        let login = shoot(
            &browser,
            &profile,
            &format!("{busy}/?token={TOKEN}"),
            (1024, 1024),
            &dir.path().join("login.png"),
        )
        .await;
        match login {
            Ok(()) => renderer = Some(Renderer::Browser(browser)),
            Err(e) if has_quick_look() => println!("{e}; using Quick Look"),
            Err(e) => return Err(e),
        }
    }
    let renderer = match renderer {
        Some(r) => r,
        None if has_quick_look() => Renderer::QuickLook(Inliner::new(&busy).await?),
        None => {
            println!("no Chromium browser found (set STUDIO_BROWSER); skipping the page gallery");
            return Ok(());
        }
    };

    // (name, server, path)
    let pages = [
        ("board", &busy, "/board"),
        ("board-idle", &idle, "/board"),
        ("ticket-working", &busy, "/tickets/SHOP-2"),
        ("ticket-asking", &busy, "/tickets/SHOP-3"),
        ("ticket-ready", &busy, "/tickets/SHOP-7"),
        ("ticket-failed", &busy, "/tickets/SHOP-9"),
        ("ticket-landed-merged", &busy, "/tickets/SHOP-17"),
        ("ticket-landed", &busy, "/tickets/EDIT-2"),
        ("asks", &busy, "/asks"),
        ("goals", &busy, "/goals"),
        ("budget", &busy, "/budget"),
        ("hires", &busy, "/hires"),
        ("rules", &busy, "/rules"),
        (
            "rules-project",
            &busy,
            "/rules?file=protect-payments&from=project/shop",
        ),
        ("hire-edit", &busy, "/hires?file=dev"),
        ("office", &busy, "/"),
        ("office-closed", &closed, "/"),
        ("office-tab-projects", &busy, "/?tab=projects"),
        ("office-tab-team", &busy, "/?tab=team"),
        ("office-tab-money", &busy, "/?tab=money"),
        ("office-tab-rules", &busy, "/?tab=rules"),
        ("office-tab-log", &busy, "/?tab=log"),
        ("office-working", &busy, "/?sel=tickets/SHOP-2"),
        ("office-asking", &busy, "/?sel=tickets/SHOP-3"),
        ("office-checks", &busy, "/?sel=tickets/SHOP-5"),
        ("office-review", &busy, "/?sel=tickets/SHOP-6"),
        ("office-ready", &busy, "/?sel=tickets/SHOP-7"),
        ("office-held", &busy, "/?sel=tickets/SHOP-10"),
        ("office-planner", &busy, "/?sel=tickets/EDIT-1"),
        ("office-hire", &busy, "/?sel=hires/approver"),
        ("office-goal", &busy, "/?sel=goals/SHOP-14"),
        ("office-idle", &idle, "/"),
        ("office-turn-1", &busy, "/?turn=1"),
        ("office-turn-2", &busy, "/?turn=2"),
        ("office-turn-3", &busy, "/?turn=3"),
        ("office-empty", &empty, "/?tab=team"),
        ("office-empty-start", &empty, "/?tab=needs"),
    ];
    // One picture for each thing on the floor.
    let elements = ELEMENTS.iter().map(|key| {
        (
            format!("office-el-{key}"),
            &busy,
            format!("/?sel=elements/{key}"),
        )
    });
    let pages = pages
        .into_iter()
        .map(|(name, base, path)| (name.to_string(), base, path.to_string()))
        .chain(elements);
    for (name, base, path) in pages {
        let name = name.as_str();
        let url = format!("{base}{path}");
        for (suffix, width) in WIDTHS {
            let png = out_dir.join(format!("{name}{suffix}.png"));
            match &renderer {
                Renderer::Browser(browser) => {
                    shoot(browser, &profile, &url, (width, 1024), &png).await?;
                }
                Renderer::QuickLook(inliner) => {
                    let page = inliner.page(&url, None).await?;
                    quick_look(&page, width, dir.path(), &png).await?;
                }
            }
            println!("wrote {}", png.display());
        }
        if let (Renderer::QuickLook(inliner), true) = (&renderer, LIGHT.contains(&name)) {
            let page = inliner.page(&url, Some("light")).await?;
            for (suffix, width) in &WIDTHS[..2] {
                let png = out_dir.join(format!("{name}-light{suffix}.png"));
                quick_look(&page, *width, dir.path(), &png).await?;
                println!("wrote {}", png.display());
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(out_dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: gallery <out-dir>");
        return ExitCode::FAILURE;
    };
    match run(Path::new(&out_dir)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gallery: {e}");
            ExitCode::FAILURE
        }
    }
}
