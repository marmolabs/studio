//! Saves a screenshot of every web page, served against a stand-in studiod
//! full of sample data.
//!
//! `cargo run -q -p studio-web --example gallery -- <out-dir>`
//!
//! Writes `<name>.png` at 1024px wide, `<name>-1440.png` and `<name>-phone.png`
//! (390px) for every page, plus a few in the light theme at 1024px and 1440px.
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
const PAGE_LIMIT: Duration = Duration::from_secs(30);
const DIFFSTAT: &str = " src/title.rs | 6 +++++-\n src/main.rs  | 1 +\n 2 files changed, 6 insertions(+), 1 deletion(-)\n";

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
        "game"
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
                       "goal": "Ship a level editor: place tiles, undo mistakes, save levels"});
    json!([
        ticket(
            "GAME-1",
            "Add a pause menu",
            "dev",
            json!("queued"),
            json!({"attempts": 0})
        ),
        ticket(
            "GAME-2",
            "Fix the jump buffer",
            "dev",
            json!("working"),
            json!({"goal": "Polish the player movement feel"})
        ),
        ticket(
            "GAME-3",
            "Spawn enemies in waves",
            "dev",
            json!({"asking": {"ask": "ASK-1"}}),
            json!({"goal": "Enemy waves", "parent": "GAME-21", "attempts": 0, "created_at": now_ms() - 21 * 60_000,
                   "detail": "Enemies arrive in waves that grow in size. A wave starts once the previous one is cleared, after a three second pause. Wave sizes come from a table in `rules.rs`, so they can be tuned without touching systems.\n\n**Done when** waves 1 to 8 follow the table, later waves add two enemies each, and `rules::wave_size` has tests."})
        ),
        ticket(
            "GAME-4",
            "Tune the coyote time",
            "dev",
            json!({"asking": {"ask": "ASK-2"}}),
            json!({})
        ),
        ticket(
            "GAME-5",
            "Shake the screen on a hit",
            "dev",
            json!("checking"),
            json!({})
        ),
        ticket(
            "GAME-6",
            "Save the high score to disk",
            "dev",
            json!("reviewing"),
            json!({})
        ),
        ticket(
            "GAME-7",
            "Add a title screen",
            "dev",
            json!("ready"),
            json!({"goal": "Polish the menus", "feedback": "The **Quit** button needs a confirm step:\n\n- ask before closing\n- keep `Esc` as cancel"})
        ),
        ticket(
            "GAME-8",
            "Fix the player sprite flicker",
            "dev",
            json!("landed"),
            json!({})
        ),
        ticket(
            "GAME-9",
            "Load the tilemap faster",
            "dev",
            json!({"failed": {"class": "checks", "detail": "cargo clippy: 2 warnings"}}),
            json!({"attempts": 3, "created_at": now_ms() - 110 * 60_000})
        ),
        ticket(
            "GAME-10",
            "Add gamepad support",
            "dev",
            json!({"held": {"reason": "budget"}}),
            json!({})
        ),
        ticket(
            "EDIT-1",
            "Plan: ship the level editor",
            "producer",
            json!("working"),
            json!({"goal": "Ship a level editor: place tiles, undo mistakes, save levels",
                   "kind": "plan"})
        ),
        ticket(
            "EDIT-2",
            "Add a tile palette",
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
            "Save levels as RON files",
            "artist",
            json!("queued"),
            child
        ),
        ticket(
            "GAME-11",
            "Draw the title logo",
            "artist",
            json!("landed"),
            json!({"parent": "GAME-14", "attempts": 0})
        ),
        ticket(
            "GAME-12",
            "Paint the forest tileset",
            "artist",
            json!({"failed": {"class": "review", "detail": "palette too wide"}}),
            json!({"parent": "GAME-14"})
        ),
        ticket(
            "GAME-13",
            "Mix the boss theme",
            "composer",
            json!({"asking": {"ask": "ASK-3"}}),
            json!({"parent": "GAME-14"})
        ),
        ticket(
            "GAME-14",
            "Plan: give the game its look and sound",
            "producer",
            json!("landed"),
            json!({"goal": "Give the game its look and sound: title logo, forest tiles, boss music",
                   "kind": "plan"})
        ),
        // A goal whose tickets all landed and whose branch is merged.
        ticket(
            "GAME-16",
            "Plan: polish the menus",
            "producer",
            json!("landed"),
            json!({"goal": "Polish the menus", "kind": "plan"})
        ),
        ticket(
            "GAME-17",
            "Title screen with Start and Quit",
            "dev",
            json!("landed"),
            json!({"parent": "GAME-16"})
        ),
        ticket(
            "GAME-18",
            "Volume sliders for music and effects",
            "dev",
            json!("landed"),
            json!({"parent": "GAME-16"})
        ),
        // A goal that is done but not merged yet.
        ticket(
            "GAME-19",
            "Plan: save games",
            "producer",
            json!("landed"),
            json!({"goal": "Save games between sessions", "kind": "plan"})
        ),
        ticket(
            "GAME-20",
            "Save settings between sessions",
            "dev",
            json!("landed"),
            json!({"parent": "GAME-19"})
        ),
        // A goal whose next tickets wait for a slot.
        ticket(
            "GAME-21",
            "Plan: enemy waves",
            "producer",
            json!("landed"),
            json!({"goal": "Enemy waves", "kind": "plan"})
        ),
        ticket(
            "GAME-22",
            "Count the enemies left in a wave",
            "dev",
            json!("landed"),
            json!({"parent": "GAME-21"})
        ),
        ticket(
            "GAME-23",
            "Pause three seconds between waves",
            "dev",
            json!("queued"),
            json!({"parent": "GAME-21", "attempts": 0})
        ),
        ticket(
            "GAME-24",
            "Show the wave number on screen",
            "artist",
            json!("queued"),
            json!({"parent": "GAME-21", "attempts": 0})
        ),
        ticket(
            "GAME-15",
            "Plan: add a settings screen",
            "producer",
            json!("working"),
            json!({"goal": "Add a settings screen for volume and key bindings", "kind": "plan"})
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
        "project" => format!("{home}/games/{owner}/.studio/rules/{name}.md"),
        _ => format!("{home}/Library/Application Support/studio/rules/{name}.md"),
    };
    json!({
        "origin": origin, "owner": owner, "name": name, "path": path, "problem": null,
        "text": format!("---\n{head}\n---\n{why}\n"),
    })
}

/// What spend history looks like: 14 days, the last one ending now.
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
        .map(|(i, usd)| json!({"start_ms": start + i as u64 * 86_400_000, "usd": usd}))
        .collect();
    json!(all[all.len() - days..])
}

/// The last hour or so of the busy studio, for the office's lanes: each busy
/// or recently busy ticket's history, in an order the state machine accepts.
fn activity() -> Vec<Value> {
    let histories: [(&str, &[(u64, Value)]); 12] = [
        (
            "GAME-8",
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
            "GAME-7",
            &[
                (70, json!({"type": "claimed"})),
                (70, json!({"type": "run_started", "run": "r2"})),
                (38, json!({"type": "checks_started"})),
                (35, json!({"type": "checks_passed"})),
                (24, json!({"type": "review_passed"})),
            ],
        ),
        (
            "GAME-12",
            &[
                (58, json!({"type": "claimed"})),
                (58, json!({"type": "run_started", "run": "r3"})),
                (45, json!({"type": "checks_started"})),
                (43, json!({"type": "checks_passed"})),
                (
                    40,
                    json!({"type": "review_failed", "detail": "art gate: the palette is too wide for the forest set"}),
                ),
                (25, json!({"type": "checks_started"})),
                (24, json!({"type": "checks_passed"})),
                (
                    20,
                    json!({"type": "review_failed", "detail": "art gate: the palette is still too wide"}),
                ),
                (
                    18,
                    json!({"type": "failed", "class": "review", "detail": "palette too wide"}),
                ),
            ],
        ),
        (
            "GAME-2",
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
            "GAME-6",
            &[
                (31, json!({"type": "claimed"})),
                (31, json!({"type": "run_started", "run": "r6"})),
                (15, json!({"type": "checks_started"})),
                (9, json!({"type": "checks_passed"})),
            ],
        ),
        (
            "GAME-5",
            &[
                (24, json!({"type": "claimed"})),
                (24, json!({"type": "run_started", "run": "r7"})),
                (2, json!({"type": "checks_started"})),
            ],
        ),
        (
            "GAME-3",
            &[
                (18, json!({"type": "claimed"})),
                (18, json!({"type": "run_started", "run": "r8"})),
                (2, json!({"type": "ask_opened", "ask": "ASK-1"})),
            ],
        ),
        (
            "GAME-13",
            &[
                (14, json!({"type": "claimed"})),
                (14, json!({"type": "run_started", "run": "r9"})),
                (9, json!({"type": "ask_opened", "ask": "ASK-3"})),
            ],
        ),
        (
            "GAME-4",
            &[
                (9, json!({"type": "claimed"})),
                (9, json!({"type": "run_started", "run": "r10"})),
                (6, json!({"type": "ask_opened", "ask": "ASK-2"})),
            ],
        ),
        (
            "GAME-15",
            &[
                (6, json!({"type": "claimed"})),
                (6, json!({"type": "run_started", "run": "r11"})),
            ],
        ),
        (
            "GAME-10",
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
        member("artist", "pixel artist", "haiku"),
        member("composer", "composer", "sonnet"),
        member("dev", "engineer", "sonnet"),
        member("producer", "producer", "opus"),
        member("reviewer", "reviewer", "sonnet"),
        member("router", "router", "haiku"),
        member("tester", "qa", "haiku"),
    ])
}

/// What happened over the last day, oldest first: the last hour's activity
/// and the work that ended before it, only of `kinds` when there are any.
fn recent_events(kinds: &[String]) -> Value {
    let ended = [
        ("GAME-17", 20 * 3600, json!({"type": "landed"})),
        ("GAME-18", 19 * 3600, json!({"type": "landed"})),
        ("GAME-11", 6 * 3600, json!({"type": "landed"})),
        ("EDIT-2", 5 * 3600, json!({"type": "landed"})),
        ("GAME-20", 2 * 3600, json!({"type": "landed"})),
        // The last try's failed checks fail the ticket; no `failed` event follows.
        (
            "GAME-9",
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
            "spent_24h": 0.0, "projects": ["game", "editor"],
            "hires": ["dev", "artist", "producer", "composer", "tester", "reviewer", "approver", "router"],
            "max_runs": 6, "team": team(),
        }),
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

/// A ticket's history: a run sent back once, then a second run. Some
/// tickets also asked, were held, or began their run at another time.
fn events(id: &str) -> Value {
    let at = |minutes_ago: u64| now_ms() - minutes_ago * 60_000;
    if id == "GAME-3" {
        return json!([
            {"seq": 1, "ticket_id": id, "at_ms": at(21), "event": {"type": "created", "id": id,
             "project": "game", "title": "Spawn enemies in waves", "goal": "Enemy waves", "hire": "dev",
             "base_branch": "main", "created_at": at(21)}},
            {"seq": 2, "ticket_id": id, "at_ms": at(18), "event": {"type": "claimed"}},
            {"seq": 3, "ticket_id": id, "at_ms": at(18), "event": {"type": "run_started", "run": "r1"}},
            {"seq": 4, "ticket_id": id, "at_ms": at(2), "event": {"type": "ask_opened", "ask": "ASK-1"}},
        ]);
    }
    if id == "GAME-9" {
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
        "GAME-5" => 12,
        "GAME-6" => 18,
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
                   "detail": "**Must fix:** the `Quit` button closes at once.\nAsk first, then quit."}},
        {"seq": 5, "ticket_id": id, "at_ms": at(21),
         "event": {"type": "changes_requested",
                   "detail": "Keep **Esc** as cancel:\n\n- close the dialog\n- stay on `Title`"}},
        {"seq": 6, "ticket_id": id, "at_ms": at(run), "event": {"type": "claimed"}},
        {"seq": 7, "ticket_id": id, "at_ms": at(9), "event": {"type": "checks_passed"}},
    ]);
    let extra = match id {
        "GAME-4" => json!({"type": "ask_opened", "ask": "ASK-2"}),
        "GAME-13" => json!({"type": "ask_opened", "ask": "ASK-3"}),
        "GAME-10" => json!({"type": "held", "reason": "budget"}),
        _ => return events,
    };
    let minutes = match id {
        "GAME-13" => 9,
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

fn reply(request: &Request) -> Value {
    let at = |minutes_ago: u64| now_ms() - minutes_ago * 60_000;
    match request {
        Request::Status => json!({
            // Runs paused on an ask hold their slots too.
            "asks": 3, "running": 8, "goals": 6, "rules": 5,
            "tickets": {"queued": 5, "working": 3, "checking": 1, "reviewing": 1, "asking": 3,
                        "held": 1, "ready": 1, "landed": 11, "failed": 2},
            "daily_usd": 10.0, "spent_24h": 6.4, "projects": ["game", "editor"],
            "hires": ["dev", "artist", "producer", "composer", "tester", "reviewer", "approver", "router"],
            "max_runs": 9, "team": team(),
        }),
        Request::RecentEvents { kinds, .. } => recent_events(kinds),
        Request::SpendByDay { days } => spend_by_day(*days, 6.4),
        Request::Tickets => tickets(),
        Request::Events {
            ticket: Some(id), ..
        } => events(id),
        Request::Activity { .. } => numbered(activity()),
        Request::StateSince => state_since(),
        Request::Diff { ticket } if ticket == "GAME-3" => json!(
            "diff --git a/src/rules.rs b/src/rules.rs\n--- a/src/rules.rs\n+++ b/src/rules.rs\n@@ -1,3 +1,6 @@\n-pub const WAVE: u32 = 4;\n+pub const WAVES: [u32; 8] = [4, 5, 6, 8, 10, 12, 14, 16];\n+\n+pub fn wave_size(n: usize) -> u32 {\n+    WAVES.get(n).copied().unwrap_or(16 + 2 * (n as u32 - 7))\n+}\ndiff --git a/src/spawning.rs b/src/spawning.rs\n--- a/src/spawning.rs\n+++ b/src/spawning.rs\n@@ -10,4 +10,4 @@\n-    timer.tick(time.delta());\n-    if timer.finished() {\n+    if wave_cleared(&enemies) {\n+        pause.tick(time.delta());\n"
        ),
        Request::Diff { .. } => json!(
            "diff --git a/src/title.rs b/src/title.rs\n--- a/src/title.rs\n+++ b/src/title.rs\n@@ -4,7 +4,11 @@ pub fn draw_title(ui: &mut Ui) {\n     ui.heading(\"Starlight Run\");\n-    ui.label(\"Press any key\");\n+    if ui.button(\"Start\").clicked() {\n+        next.set(GameState::Playing);\n+    }\n+    if ui.button(\"Quit\").clicked() {\n+        exit.send(AppExit);\n+    }\n }\n"
        ),
        Request::Diffstat { .. } => json!(DIFFSTAT),
        Request::Diffstats { tickets } => json!(
            tickets
                .iter()
                .map(|t| (t.clone(), DIFFSTAT))
                .collect::<std::collections::BTreeMap<_, _>>()
        ),
        // Nothing is booked for a run that has only just started.
        Request::TicketSpend { ticket } if matches!(ticket.as_str(), "GAME-2" | "GAME-3") => {
            json!(0.0)
        }
        Request::TicketSpend { .. } => json!(0.31),
        Request::TicketSpends => json!({
            "EDIT-2": 0.18, "GAME-8": 0.84, "GAME-4": 0.11, "GAME-11": 0.31, "GAME-12": 0.84, "GAME-13": 0.22, "GAME-14": 0.12, "GAME-15": 0.09,
            "GAME-16": 0.11, "GAME-17": 2.06, "GAME-18": 0.84, "GAME-19": 0.08, "GAME-20": 1.12,
            "GAME-2": 0.38, "GAME-5": 0.91, "GAME-6": 1.24, "GAME-7": 2.06, "GAME-9": 1.96,
            "GAME-10": 0.52, "GAME-21": 0.07, "GAME-22": 0.35, "EDIT-1": 0.3,
        }),
        // When tickets last changed; the rest are left out.
        Request::UpdatedAt => json!({
            "EDIT-1": at(4), "EDIT-2": at(26), "GAME-13": at(2), "GAME-15": at(1),
            "GAME-16": at(1500), "GAME-18": at(1450), "GAME-19": at(70), "GAME-20": at(65),
            "GAME-21": at(300), "GAME-22": at(290),
        }),
        // The menus goal is merged; the save-games goal is done but not merged.
        Request::MergedTickets => json!(["GAME-17", "GAME-18"]),
        Request::Asks => json!([{
            "id": "ASK-1", "ticket": "GAME-3", "hire": "dev", "kind": "permission", "at_ms": at(2),
            "tool": "Bash",
            "input": {"command": "cargo run --example waves", "description": "Try the wave spawner"},
            "reasons": ["Approver: running an example opens a game window, so it is yours to decide"],
        }, {
            "id": "ASK-2", "ticket": "GAME-4", "hire": "dev", "kind": "permission", "at_ms": at(6),
            "tool": "Edit",
            "input": {"file_path": "src/player/movement.rs",
                      "old_string": "const COYOTE_TIME: f32 = 0.08;",
                      "new_string": "const COYOTE_TIME: f32 = 0.12;"},
            "reasons": ["Movement feel is tuned by hand: ask before changing it (protect-player)"],
        }, {
            "id": "ASK-3", "ticket": "GAME-13", "hire": "composer", "kind": "permission", "at_ms": at(9),
            "tool": "Bash",
            "input": {"command": "ffmpeg -i assets/music/boss_theme_raw.wav -af loudnorm=I=-16:TP=-1.5:LRA=11 -c:a libvorbis -q:a 5 assets/music/boss_theme.ogg"},
            "reasons": [],
        }]),
        Request::Budget => json!({
            "limits": {"daily_usd": 10.0, "ticket_usd": 2.0, "run_usd": null, "min_run_usd": 0.05,
                       "projects": {"game": 6.0, "editor": 4.0}, "hires": {"dev": 8.0, "artist": 2.0}},
            "spent_24h": {"total": 6.4, "by_project": {"game": 4.2, "editor": 1.5, "assets": 0.7},
                          "by_hire": {"dev": 4.4, "artist": 0.6, "producer": 0.6, "reviewer": 0.5,
                                      "approver": 0.2, "router": 0.1},
                          "by_model": {"sonnet": 4.9, "haiku": 0.9, "opus": 0.6},
                          "by_ticket": {"GAME-7": 2.06, "GAME-9": 1.96, "GAME-6": 1.24, "GAME-20": 1.12,
                                        "GAME-5": 0.91},
                          "runs_by_hire": {"dev": 9, "artist": 2, "producer": 1, "reviewer": 3,
                                           "approver": 41, "router": 6}},
            "runs_24h": 62,
            "reserved": {"GAME-2": 0.5, "GAME-5": 0.4, "EDIT-1": 0.25},
        }),
        Request::Files {
            kind: FileKind::Hires,
        } => json!([
            hire_file("dev", "engineer", "sonnet", "opus", "Writes code, with tests, in small steps."),
            hire_file("artist", "pixel artist", "sonnet", "", "Pixel art and UI."),
            hire_file("producer", "producer", "sonnet", "", "Plans goals as small tickets."),
            hire_file("tester", "critic", "opus", "sonnet", "Design critic for menus and feel."),
            hire_file("reviewer", "reviewer", "opus", "sonnet", "Reads every diff before it lands."),
            {"name": "composer", "problem": "line 3: did not find expected key", "text": ""},
            hire_file("approver", "approver", "haiku", "", "Decides tool calls no rule covers."),
            hire_file("router", "router", "haiku", "", "Picks the hire for new tickets."),
        ]),
        Request::HiresToday => json!({
            "dev": {"done": 6}, "artist": {"done": 1}, "producer": {"planned": 2},
            "tester": {"critiques": 4}, "reviewer": {"reviews": 11},
            "approver": {"allowed": 81, "denied": 11, "to_owner": 4}, "router": {"routed": 12},
        }),
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
                "game",
                "protect-player",
                "kind: protect\nlevel: ask\nwhen:\n  paths: [\"src/player/**\"]",
                "Player movement is tuned by hand. Ask before\nchanging anything under src/player."
            ),
            rule_file(
                "project",
                "game",
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
            json!({"file_path": format!("src/player/{file}.rs")}),
        )
    };
    let edit = |file: &str| {
        call(
            "Edit",
            json!({"file_path": format!("src/player/{file}.rs")}),
        )
    };
    let bash = |command: &str| call("Bash", json!({"command": command}));
    let mut long = vec![
        text(
            "The jump buffer drops inputs pressed just before landing. Let me look at the player controller.",
        ),
        read("jump"),
        read("input"),
        read("state"),
        read("movement"),
        call("Grep", json!({"pattern": "jump_buffer"})),
        call("Grep", json!({"pattern": "COYOTE_TIME"})),
        text(
            "The timer resets every frame while the key is held. I'll store the press time instead.",
        ),
        edit("jump"),
        rule.clone(),
        edit("input"),
        rule.clone(),
        edit("state"),
        rule.clone(),
        bash("cargo test jump"),
        bash(
            "cargo test --workspace --locked -- --test-threads=1 --nocapture player::jump::buffered_press_fires_after_landing",
        ),
        text("Two tests fail: the buffer is cleared before the landing check runs."),
        read("jump"),
        read("movement"),
        edit("jump"),
        rule.clone(),
        bash("cargo test jump"),
        text("Buffered jumps now fire up to 120ms after the press."),
        call("Glob", json!({"pattern": "tests/**/*.rs"})),
        read("jump"),
        edit("jump"),
        rule.clone(),
        edit("state"),
        rule.clone(),
        bash("cargo fmt --all"),
        bash("cargo clippy --workspace --all-targets -- -D warnings"),
        text("Formatting and lints are clean. Running the full test suite."),
        read("input"),
        read("jump"),
        bash("cargo test --workspace"),
    ];
    long.push(bash("cargo test --workspace --release"));
    long.push(json!({"event": {"type": "permission", "tool": "Bash"}}));
    long.push(json!({"note": "allowed Bash: cargo test (rule allow-cargo-test)"}));
    std::fs::write(runs.join("GAME-2.jsonl"), lines(&long))?;
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
    let waves = [
        ago(20, json!({"note": "routed to dev: game logic with tests"})),
        ago(
            18,
            json!({"event": {"type": "started", "session": "s1", "model": "sonnet"}}),
        ),
        ago(
            18,
            text(
                "Spawning runs on a fixed timer in `spawning.rs`. I'll move wave sizes into `rules.rs` as a table and drive spawning from it:\n\n- `WAVES`: sizes for waves 1 to 8\n- `wave_size(n)`: the table, then two more per wave\n- `wave_cleared`: true when no enemy is alive",
            ),
        ),
        ago(
            17,
            id_call("r1", "Read", json!({"file_path": "src/spawning.rs"})),
        ),
        ago(17, output("r1", numbered(142))),
        ago(
            17,
            id_call("r2", "Read", json!({"file_path": "src/rules.rs"})),
        ),
        ago(17, output("r2", numbered(310))),
        ago(16, id_call("g1", "Grep", json!({"pattern": "spawn_enemy"}))),
        ago(
            16,
            output(
                "g1",
                "Found 3 files\nsrc/spawning.rs\nsrc/rules.rs\nsrc/main.rs".into(),
            ),
        ),
        ago(
            14,
            id_call(
                "e1",
                "Edit",
                json!({"file_path": "src/rules.rs",
            "old_string": numbered(3), "new_string": numbered(48).replace("line", "row")}),
            ),
        ),
        ago(
            12,
            id_call(
                "e2",
                "Edit",
                json!({"file_path": "src/spawning.rs",
            "old_string": numbered(22), "new_string": numbered(31).replace("line", "wave")}),
            ),
        ),
        ago(
            8,
            id_call(
                "b1",
                "Bash",
                json!({"command": "cargo test --locked rules::"}),
            ),
        ),
        ago(
            8,
            json!({"event": {"type": "permission", "tool": "Bash", "input": {"command": "cargo test --locked rules::"}}}),
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
            text("The tests pass. Before I hand it over I'd like to watch it run."),
        ),
        ago(
            2,
            id_call(
                "b2",
                "Bash",
                json!({"command": "cargo run --example waves"}),
            ),
        ),
        ago(
            2,
            json!({"event": {"type": "permission", "tool": "Bash", "input": {"command": "cargo run --example waves"}}}),
        ),
    ];
    let mut log = String::new();
    for entry in waves {
        log.push_str(&entry.to_string());
        log.push('\n');
    }
    std::fs::write(runs.join("GAME-3.jsonl"), log)?;
    // Checks under way, and a review: notes at fixed times before now.
    let at = |seconds_ago: u64, entry: Value| {
        let mut entry = entry;
        entry["at"] = json!(now_ms() - seconds_ago * 1000);
        format!("{entry}\n")
    };
    let note = |text: &str| json!({"note": text});
    std::fs::write(
        runs.join("GAME-5.jsonl"),
        [
            at(
                400,
                text("The screen shakes on every hit now. Handing over to the checks."),
            ),
            at(116, note("check: cargo fmt --all --check")),
            at(112, note("check: cargo clippy --workspace -- -D warnings")),
            at(74, note("check: cargo test --workspace")),
        ]
        .concat(),
    )?;
    std::fs::write(
        runs.join("GAME-6.jsonl"),
        [
            at(560, note("checks passed")),
            at(540, note("review by reviewer (sonnet)")),
            at(
                30,
                call("Read", json!({"file_path": "src/save/high_score.rs"})),
            ),
        ]
        .concat(),
    )?;
    std::fs::write(
        runs.join("GAME-7.jsonl"),
        lines(&[
            text(
                "Added a **title screen** with Start and Quit buttons:\n\n- `src/title.rs` spawns the menu\n- `main.rs` starts in the `Title` state\n\n```rust\napp.add_systems(OnEnter(State::Title), spawn_title);\n```",
            ),
            call("Write", json!({"file_path": "src/title.rs"})),
            call("Edit", json!({"file_path": "src/main.rs"})),
            call("Bash", json!({"command": "cargo test --workspace"})),
            json!({"note": "checks passed"}),
            json!({"note": "review by reviewer (opus)"}),
            json!({"note": "review passed"}),
            json!({"note": "gate design by tester (opus)"}),
            json!({"note": "gate design passed"}),
            fin,
        ]),
    )?;
    std::fs::write(
        runs.join("GAME-6.jsonl"),
        lines(&[
            call("Write", json!({"file_path": "src/save.rs"})),
            json!({"note": "checks passed"}),
            json!({"note": "review by reviewer (opus)"}),
            call("Read", json!({"file_path": "src/save.rs"})),
        ]),
    )?;
    std::fs::write(
        runs.join("EDIT-1.jsonl"),
        lines(&[
            text("The editor needs a palette, undo and saving. I'll file one ticket for each."),
            call(
                "mcp__studio__file_ticket",
                json!({"title": "Save levels as RON files"}),
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
        ("ticket-working", &busy, "/tickets/GAME-2"),
        ("ticket-asking", &busy, "/tickets/GAME-3"),
        ("ticket-ready", &busy, "/tickets/GAME-7"),
        ("ticket-failed", &busy, "/tickets/GAME-9"),
        ("ticket-landed-merged", &busy, "/tickets/GAME-17"),
        ("ticket-landed", &busy, "/tickets/EDIT-2"),
        ("asks", &busy, "/asks"),
        ("goals", &busy, "/goals"),
        ("budget", &busy, "/budget"),
        ("hires", &busy, "/hires"),
        ("rules", &busy, "/rules"),
        (
            "rules-project",
            &busy,
            "/rules?file=protect-player&from=project/game",
        ),
        ("hire-edit", &busy, "/hires?file=dev"),
        ("office", &busy, "/"),
        // The Last hour drawer, opened (Quick Look only).
        ("office-lanes", &busy, "/"),
        ("office-working", &busy, "/?sel=tickets/GAME-2"),
        ("office-asking", &busy, "/?sel=tickets/GAME-3"),
        ("office-checks", &busy, "/?sel=tickets/GAME-5"),
        ("office-review", &busy, "/?sel=tickets/GAME-6"),
        ("office-ready", &busy, "/?sel=tickets/GAME-7"),
        ("office-held", &busy, "/?sel=tickets/GAME-10"),
        ("office-planner", &busy, "/?sel=tickets/EDIT-1"),
        ("office-hire", &busy, "/?sel=hires/approver"),
        ("office-goal", &busy, "/?sel=goals/GAME-14"),
        ("office-idle", &idle, "/"),
    ];
    for (name, base, path) in pages {
        let url = format!("{base}{path}");
        for (suffix, width) in WIDTHS {
            let png = out_dir.join(format!("{name}{suffix}.png"));
            match &renderer {
                Renderer::Browser(browser) => {
                    shoot(browser, &profile, &url, (width, 1024), &png).await?;
                }
                Renderer::QuickLook(inliner) => {
                    let mut page = inliner.page(&url, None).await?;
                    // Opened, with the floor left out so the lanes fit the picture.
                    if name == "office-lanes" {
                        page = page
                            .replacen("<details class=\"group", "<details open class=\"group", 1)
                            .replacen(
                                "</head>",
                                "<style>#office>.grid{display:none}</style></head>",
                                1,
                            );
                    }
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
