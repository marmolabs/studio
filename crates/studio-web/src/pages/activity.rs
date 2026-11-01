//! A ticket's activity: the studio's events and the run log in one feed,
//! oldest first. An agent's message is Markdown, and the tool calls after it
//! fold into one group with a line per call.

use serde_json::Value;
use std::collections::HashMap;
use studio_core::event::Event;
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_git::Repo;
use topcoat::view::{View, component, view};

use super::timeline::{exact, relative};
use crate::client::Stamped;
use crate::summary::{tool_arg, truncate};
use crate::words::{self, Wanted};
use crate::{markdown, ui};

const ARG_SHOWN: usize = 200;
const RAW_SHOWN: usize = 300;
const CODE_SHOWN: usize = 120;
/// How long before its event an ask's log line may be: the approver may
/// think first.
const ASK_LEAD_MS: u64 = 10 * 60_000;

/// One line of the run log.
#[derive(Clone, Debug, PartialEq)]
pub enum Logged {
    Started {
        model: Option<String>,
    },
    Text(String),
    Call {
        id: String,
        name: String,
        input: Value,
    },
    Result {
        id: String,
        output: String,
        error: bool,
    },
    /// A permission or question that reached the owner, and the answer.
    Asked {
        tool: String,
        input: Value,
        answer: Answer,
    },
    Note(String),
    Finished {
        outcome: String,
    },
}

/// What the owner said to an ask, as far as the log shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Waiting,
    Allowed,
    Denied(String),
}

/// The run log's lines with their times. A line from an older log without a
/// time takes the time of the line before it.
pub fn parse_log(text: &str) -> Vec<(u64, Logged)> {
    let mut lines = Vec::new();
    let mut last = 0;
    for raw in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let at = entry["at"].as_u64().filter(|at| *at > 0).unwrap_or(last);
        last = at;
        let logged = if let Some(note) = entry["note"].as_str() {
            Logged::Note(note.to_string())
        } else {
            let e = &entry["event"];
            let text = |key: &str| e[key].as_str().unwrap_or_default().to_string();
            match e["type"].as_str().unwrap_or_default() {
                "started" => Logged::Started {
                    model: e["model"].as_str().map(str::to_string),
                },
                "text" => Logged::Text(text("text")),
                "tool_call" => Logged::Call {
                    id: text("id"),
                    name: text("name"),
                    input: e["input"].clone(),
                },
                "tool_result" => Logged::Result {
                    id: text("id"),
                    output: text("output"),
                    error: e["is_error"] == true,
                },
                "permission" => Logged::Asked {
                    tool: text("tool"),
                    input: e["input"].clone(),
                    answer: Answer::Waiting,
                },
                "question" => Logged::Asked {
                    tool: "AskUserQuestion".into(),
                    input: Value::Null,
                    answer: Answer::Waiting,
                },
                "finished" => Logged::Finished {
                    outcome: e["outcome"]["outcome"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                },
                _ => continue,
            }
        };
        lines.push((at, logged));
    }
    settle_asks(lines)
}

/// Whether a note is a rule's or the approver's answer to a permission.
fn decided(note: &str) -> bool {
    [
        "allowed ",
        "blocked ",
        "approver allowed ",
        "approver denied ",
    ]
    .iter()
    .any(|p| note.starts_with(p))
}

/// Drops the permissions a rule or the approver answered (their note says
/// so), and works out what the owner said to the rest.
fn settle_asks(lines: Vec<(u64, Logged)>) -> Vec<(u64, Logged)> {
    let mut out: Vec<(u64, Logged)> = Vec::with_capacity(lines.len());
    for (i, (at, line)) in lines.iter().enumerate() {
        let Logged::Asked { tool, input, .. } = line else {
            out.push((*at, line.clone()));
            continue;
        };
        if matches!(lines.get(i + 1), Some((_, Logged::Note(n))) if decided(n)) {
            continue;
        }
        let mut answer = Answer::Waiting;
        for (_, later) in &lines[i + 1..] {
            match later {
                Logged::Note(n) => {
                    let denied = n
                        .strip_prefix("denied, run continues: ")
                        .or_else(|| n.strip_prefix("denied: "));
                    if let Some(why) = denied {
                        answer = Answer::Denied(why.to_string());
                        break;
                    }
                }
                _ => {
                    answer = Answer::Allowed;
                    break;
                }
            }
        }
        out.push((
            *at,
            Logged::Asked {
                tool: tool.clone(),
                input: input.clone(),
                answer,
            },
        ));
    }
    out
}

/// What the agent said just before its latest ask, if it said anything.
pub fn last_words(log: &str) -> Option<String> {
    for raw in log.lines().rev() {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        match entry["event"]["type"].as_str() {
            Some("permission" | "question" | "tool_call" | "usage" | "rate_limit") => {}
            Some("text") => {
                let text = entry["event"]["text"].as_str()?;
                let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
                return (!words.is_empty()).then_some(words);
            }
            _ => return None,
        }
    }
    None
}

/// Who filed a ticket.
#[derive(Clone, Debug, PartialEq)]
pub enum Filer {
    /// A hire, from the ticket its run was on.
    Hire(String),
    /// The studio itself, e.g. after a failed CI run.
    Studio(String),
    Owner,
}

pub fn filer(ticket: &Ticket, tickets: &[Ticket]) -> Filer {
    if let Some(parent) = &ticket.parent {
        let hire = tickets
            .iter()
            .find(|t| &t.id == parent)
            .map_or_else(|| parent.to_string(), |t| t.hire.to_string());
        return Filer::Hire(hire);
    }
    match &ticket.source {
        Some(source) => Filer::Studio(source.clone()),
        None => Filer::Owner,
    }
}

/// A piece of a feed line.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    Bold(String),
    Code(String),
    Model(String),
}

fn text(s: impl Into<String>) -> Part {
    Part::Text(s.into())
}

fn bold(s: impl Into<String>) -> Part {
    Part::Bold(s.into())
}

fn code(s: &str) -> Part {
    Part::Code(truncate(s, CODE_SHOWN))
}

/// "producer filed this from Enemy waves": who filed the ticket, and from
/// which goal.
pub fn filed_parts(ticket: &Ticket, filer: &Filer) -> Vec<Part> {
    let mut parts = match filer {
        Filer::Hire(hire) => vec![bold(hire), text(" filed this")],
        Filer::Studio(source) if source.starts_with("ci:") => {
            vec![bold("studio"), text(" filed this after a CI run failed")]
        }
        Filer::Studio(source) if source.starts_with("merge:") => {
            vec![
                bold("studio"),
                text(" filed this when merging the goal conflicted"),
            ]
        }
        Filer::Studio(_) => vec![bold("studio"), text(" filed this")],
        Filer::Owner if ticket.kind == TicketKind::Plan => {
            vec![bold("You"), text(" set this goal")]
        }
        Filer::Owner => vec![bold("You"), text(" filed this")],
    };
    if let (Some(goal), TicketKind::Work) = (goal_name(ticket), ticket.kind) {
        parts.extend([text(" from "), bold(goal)]);
    }
    parts
}

/// The first line of the ticket's goal, if it has one.
pub fn goal_name(ticket: &Ticket) -> Option<String> {
    ticket
        .goal
        .lines()
        .next()
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
}

/// The branch a work ticket lands on.
pub fn goal_branch(ticket: &Ticket) -> String {
    let goal = if ticket.goal.trim().is_empty() {
        ticket.id.to_string()
    } else {
        ticket.goal.clone()
    };
    Repo::goal_branch_name(&goal)
}

/// A one-line entry: something the studio, the owner or a reviewer did.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    at: u64,
    icon: &'static str,
    /// `attn`, `ok`, `bad`, or empty.
    tone: &'static str,
    parts: Vec<Part>,
    /// Longer text under the line, and whether it is Markdown (shown) or log
    /// output (folded).
    detail: Option<(String, bool)>,
}

fn line(at: u64, icon: &'static str, tone: &'static str, parts: Vec<Part>) -> Line {
    Line {
        at,
        icon,
        tone,
        parts,
        detail: None,
    }
}

/// An agent's message and the steps it took after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    at: u64,
    hire: String,
    model: Option<String>,
    message: Option<String>,
    steps: Vec<Step>,
}

/// One tool call, or a rule's answer to one.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    /// What it counts as in the group's summary: `read`, `edit`, `write`,
    /// `run`, `search`, `allowed`, `blocked` or `other`.
    kind: &'static str,
    icon: &'static str,
    label: String,
    arg: String,
    /// The call's input, for a tooltip.
    raw: String,
    result: Done,
}

/// What came of a step.
#[derive(Clone, Debug, PartialEq)]
enum Done {
    Nothing,
    Plain(String),
    Good(String),
    Bad(String),
    Lines(u64, u64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Line(Line),
    Turn(Turn),
}

/// A ticket's feed, and the model its own hire last ran on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    pub items: Vec<Item>,
    pub model: Option<String>,
}

/// Lines added and removed by replacing `old` with `new`, leaving out the
/// lines both start and end with.
fn changed_lines(old: &str, new: &str) -> (u64, u64) {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        (new.len() - start - end) as u64,
        (old.len() - start - end) as u64,
    )
}

fn call_step(name: &str, input: &Value) -> Step {
    let (kind, icon, label) = match name {
        "Read" | "NotebookRead" => ("read", "file-text", "Read"),
        "Edit" | "MultiEdit" | "NotebookEdit" => ("edit", "file-pen-line", "Edit"),
        "Write" => ("write", "file-pen-line", "Write"),
        "Bash" => ("run", "terminal", "Run"),
        "Grep" | "Glob" | "WebSearch" => ("search", "search", "Search"),
        "WebFetch" => ("other", "external-link", "Fetch"),
        "Task" | "Agent" => ("other", "bot", "Agent"),
        "TodoWrite" => ("other", "list-checks", "To-dos"),
        _ if name.starts_with("mcp__studio__") => ("other", "bot", "Studio"),
        _ => ("other", "circle-dashed", "Tool"),
    };
    let arg = match name {
        "Task" | "Agent" => input["description"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        "TodoWrite" => format!("{} items", input["todos"].as_array().map_or(0, Vec::len)),
        _ if name.starts_with("mcp__") => {
            let short = name.rsplit("__").next().unwrap_or(name).replace('_', " ");
            let what = ["title", "question", "name"]
                .iter()
                .find_map(|k| input[k].as_str())
                .unwrap_or_default();
            format!("{short} {what}")
        }
        "Read" | "Edit" | "MultiEdit" | "Write" | "Bash" | "Grep" | "Glob" | "WebFetch"
        | "WebSearch" | "NotebookRead" | "NotebookEdit" => tool_arg(name, input),
        _ => format!("{name} {}", tool_arg(name, input)),
    };
    let result = match name {
        "Edit" => {
            let (old, new) = (input["old_string"].as_str(), input["new_string"].as_str());
            let (add, del) = changed_lines(old.unwrap_or_default(), new.unwrap_or_default());
            Done::Lines(add, del)
        }
        "MultiEdit" => {
            let (add, del) = input["edits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| {
                    let s = |k: &str| e[k].as_str().unwrap_or_default();
                    changed_lines(s("old_string"), s("new_string"))
                })
                .fold((0, 0), |(a, d), (x, y)| (a + x, d + y));
            Done::Lines(add, del)
        }
        "Write" => {
            let lines = input["content"].as_str().map_or(0, |c| c.lines().count());
            Done::Lines(lines as u64, 0)
        }
        _ => Done::Nothing,
    };
    Step {
        kind,
        icon,
        label: label.to_string(),
        arg: truncate(arg.trim(), ARG_SHOWN),
        raw: truncate(&input.to_string(), RAW_SHOWN),
        result,
    }
}

/// "12 passed", summed over every test run in cargo's output.
fn tests_passed(output: &str) -> Option<u64> {
    let counts: Vec<u64> = output
        .lines()
        .filter_map(|l| l.trim().strip_prefix("test result: ok. "))
        .filter_map(|rest| rest.split(' ').next()?.parse().ok())
        .collect();
    (!counts.is_empty()).then(|| counts.iter().sum())
}

/// What a tool's output says about the call.
fn result_of(step: &Step, name: &str, output: &str, error: bool) -> Done {
    if error {
        return Done::Bad("failed".into());
    }
    let count = |what: &str| {
        let n = output.lines().filter(|l| !l.trim().is_empty()).count();
        format!("{n} {what}{}", if n == 1 { "" } else { "s" })
    };
    match name {
        "Read" => Done::Plain(count("line")),
        "Glob" if output.starts_with("No files found") => Done::Plain("0 files".into()),
        "Glob" => Done::Plain(count("file")),
        "Grep" => match output.lines().next() {
            Some(first) if first.starts_with("Found ") => Done::Plain(truncate(first, 24)),
            _ => step.result.clone(),
        },
        "Bash" => match tests_passed(output) {
            Some(n) => Done::Good(format!("{n} passed")),
            None => step.result.clone(),
        },
        _ => step.result.clone(),
    }
}

/// The step a rule or the approver answering a call shows as.
fn rule_step(note: &str) -> Option<Step> {
    let step = |kind, icon, label: &str, arg: String| Step {
        kind,
        icon,
        label: label.to_string(),
        arg: truncate(&arg, ARG_SHOWN),
        raw: note.to_string(),
        result: Done::Nothing,
    };
    let why = |rest: &str| {
        rest.split_once(": ")
            .map_or(rest, |(_, why)| why)
            .to_string()
    };
    if let Some(rest) = note.strip_prefix("approver allowed ") {
        return Some(step(
            "allowed",
            "shield-check",
            "Allowed",
            format!("approver: {}", why(rest)),
        ));
    }
    if let Some(rest) = note.strip_prefix("approver denied ") {
        return Some(step(
            "blocked",
            "shield-alert",
            "Denied",
            format!("approver: {}", why(rest)),
        ));
    }
    if let Some(rest) = note.strip_prefix("blocked ") {
        return Some(step("blocked", "shield-alert", "Blocked", why(rest)));
    }
    let rest = note.strip_prefix("allowed ")?;
    // Calls inside the worktree and the studio's own tools need no line.
    match why(rest).as_str() {
        "inside the worktree" | "studio tool" => None,
        why => Some(step("allowed", "shield-check", "Allowed", why.to_string())),
    }
}

/// "reviewer (sonnet)" as the hire and the model.
fn hire_and_model(text: &str) -> (String, Option<String>) {
    match text.rsplit_once(" (") {
        Some((hire, model)) => (
            hire.to_string(),
            Some(model.trim_end_matches(')').to_string()),
        ),
        None => (text.to_string(), None),
    }
}

/// The name of the gate a "gate <name> …" note is about, as in "gate art
/// by critic" or "gate art: measured 2 file(s)".
pub fn gate_of(note: &str) -> Option<&str> {
    note.strip_prefix("gate ")?
        .split([' ', ':'])
        .next()
        .filter(|name| !name.is_empty())
}

/// "Asked you to run `cargo test`" for an ask the log recorded.
fn asked_parts(tool: &str, input: &Value, answer: &Answer) -> Vec<Part> {
    let mut parts = vec![bold("Asked you")];
    match words::wanted("permission", tool, input) {
        _ if tool == "AskUserQuestion" => parts.push(text(" a question")),
        Wanted {
            verb,
            target: Some(target),
            ..
        } => parts.extend([text(format!(" to {verb} ")), code(&target)]),
        Wanted { verb, what, .. } => parts.extend([text(format!(" to {verb} ")), code(&what)]),
    }
    parts.push(text(match answer {
        Answer::Waiting => " · waiting".to_string(),
        Answer::Allowed if tool == "AskUserQuestion" => " · you answered".to_string(),
        Answer::Allowed => " · you allowed it".to_string(),
        Answer::Denied(why) if why.is_empty() || why == "Denied by the owner" => {
            " · you denied it".to_string()
        }
        Answer::Denied(why) => format!(" · you denied it: {why}"),
    }));
    parts
}

/// What a note from the studio shows as, if anything. `speaker` is who is
/// running now.
fn note_line(at: u64, note: &str, speaker: &str, failed_checks: &[&str]) -> Option<Line> {
    let quiet = |parts| Some(line(at, "info", "", parts));
    if let Some(rest) = note.strip_prefix("routed to ") {
        let (hire, why) = rest.split_once(": ").unwrap_or((rest, ""));
        let mut parts = vec![bold("router"), text(" gave it to "), bold(hire)];
        if !why.is_empty() {
            parts.push(text(format!(": {why}")));
        }
        return Some(line(at, "corner-down-right", "", parts));
    }
    if let Some(rest) = note.strip_prefix("review by ") {
        let (hire, model) = hire_and_model(rest);
        let mut parts = vec![bold(hire)];
        parts.extend(model.map(Part::Model));
        parts.push(text(" started reviewing"));
        return Some(line(at, "eye", "", parts));
    }
    if let Some(gate) = gate_of(note) {
        let rest = note
            .strip_prefix("gate ")
            .and_then(|r| r.strip_prefix(gate))
            .unwrap_or_default();
        let named = |before: &str, after: String| vec![text(before), bold(gate), text(after)];
        if let Some(who) = rest.strip_prefix(" by ") {
            let (hire, model) = hire_and_model(who);
            let mut parts = vec![bold(hire)];
            parts.extend(model.map(Part::Model));
            parts.extend([text(" is checking it as the "), bold(gate), text(" gate")]);
            return Some(line(at, "flask-conical", "", parts));
        }
        if rest == " passed by the owner" {
            let mut parts = vec![bold("You")];
            parts.extend(named(" passed the ", " gate".into()));
            return Some(line(at, "circle-check", "ok", parts));
        }
        if rest == " passed" {
            return Some(line(
                at,
                "circle-check",
                "ok",
                named("Passed the ", " gate".into()),
            ));
        }
        if rest.starts_with(" failed") {
            return Some(line(
                at,
                "circle-x",
                "bad",
                named("Failed the ", " gate".into()),
            ));
        }
        if let Some(why) = rest.strip_prefix(" refused: ") {
            let mut parts = vec![bold("You")];
            parts.extend(named(" refused the ", format!(" gate: {why}")));
            return Some(line(at, "circle-x", "bad", parts));
        }
        if let Some(why) = rest.strip_prefix(" skipped: ") {
            return quiet(named("Skipped the ", format!(" gate: {why}")));
        }
    }
    if note == "review passed" {
        return Some(line(
            at,
            "circle-check",
            "ok",
            vec![bold(speaker), text(" passed the review")],
        ));
    }
    if let Some(sha) = note.strip_prefix("committed ") {
        return Some(line(
            at,
            "git-branch",
            "",
            vec![text("Committed "), code(sha.get(..7).unwrap_or(sha))],
        ));
    }
    if let Some(files) = note.strip_prefix("evidence: ") {
        return Some(line(at, "eye", "", vec![text("Captured "), text(files)]));
    }
    if let Some(summary) = note.strip_prefix("plan: ") {
        let mut line = line(
            at,
            "list-checks",
            "",
            vec![bold(speaker), text(" filed its plan")],
        );
        line.detail = Some((summary.to_string(), true));
        return Some(line);
    }
    if let Some(why) = note.strip_prefix("change blocked: ") {
        return Some(line(
            at,
            "shield-alert",
            "bad",
            vec![text(format!("A rule sent the change back: {why}"))],
        ));
    }
    if let Some(why) = note.strip_prefix("change refused: ") {
        return Some(line(
            at,
            "circle-x",
            "bad",
            vec![bold("You"), text(format!(" refused the change: {why}"))],
        ));
    }
    let shown_elsewhere = [
        "checks passed",
        "review failed:",
        "denied: ",
        "denied, run continues: ",
        "paused by the owner",
        "cancelled by the owner",
        "resumed: ",
        "started from ",
        "quote: ",
        "auto-landed on ",
    ];
    if decided(note)
        || shown_elsewhere.iter().any(|p| note.starts_with(p))
        || failed_checks.contains(&note)
    {
        return None;
    }
    if note == "no reviewer on a different model; the owner reviews" {
        return quiet(vec![text("No reviewer on another model, so you review it")]);
    }
    let (first, rest) = note.split_once('\n').unwrap_or((note, ""));
    let mut line = line(at, "info", "", vec![text(first)]);
    if !rest.trim().is_empty() {
        line.detail = Some((rest.to_string(), false));
    }
    Some(line)
}

/// The line a studio event shows as, if any.
fn event_line(
    at: u64,
    event: &Event,
    ticket: &Ticket,
    filer: &Filer,
    claims: usize,
) -> Option<Line> {
    let plain = |icon, tone, words: &str| Some(line(at, icon, tone, vec![text(words)]));
    let detailed = |icon, tone, parts, detail: &str, prose| {
        let mut line = line(at, icon, tone, parts);
        line.detail = (!detail.trim().is_empty()).then(|| (detail.to_string(), prose));
        Some(line)
    };
    match event {
        Event::Created { .. } => {
            let icon = match filer {
                Filer::Owner => "user-round",
                Filer::Hire(_) | Filer::Studio(_) => "target",
            };
            Some(line(at, icon, "", filed_parts(ticket, filer)))
        }
        Event::Claimed if claims > 1 => plain("refresh-cw", "", "Started again"),
        Event::ChecksPassed => plain("circle-check", "ok", "Checks passed"),
        Event::ChecksFailed { detail } => detailed(
            "circle-x",
            "bad",
            vec![text("Checks failed")],
            detail,
            false,
        ),
        Event::ReviewPassed => plain("circle-check", "ok", "Passed review: ready to land"),
        Event::ReviewFailed { detail } => detailed(
            "circle-x",
            "bad",
            vec![text("Review sent it back")],
            detail,
            true,
        ),
        Event::Held { reason } => {
            let state = TicketState::Held {
                reason: reason.clone(),
            };
            let why = ui::state_detail(&state).unwrap_or_default();
            plain("circle-pause", "attn", &format!("Held: {why}"))
        }
        Event::Resumed => plain("circle-play", "", "Resumed"),
        Event::Landed if ticket.kind == TicketKind::Plan => plain("circle-check", "ok", "Done"),
        Event::Landed => Some(line(
            at,
            "circle-check",
            "ok",
            vec![text("Landed on "), code(&goal_branch(ticket))],
        )),
        Event::Cancelled => plain("circle-x", "", "Cancelled"),
        Event::Failed { class, detail } => {
            let state = TicketState::Failed {
                class: class.clone(),
                detail: String::new(),
            };
            let why = ui::state_detail(&state).unwrap_or_default();
            detailed(
                "circle-x",
                "bad",
                vec![text(format!("Failed: {why}"))],
                detail,
                false,
            )
        }
        Event::Retried => plain("refresh-cw", "", "Queued again"),
        Event::ChangesRequested { detail } => detailed(
            "corner-down-right",
            "",
            vec![bold("You"), text(" sent it back")],
            detail,
            true,
        ),
        Event::Conflicted { branch, files } => detailed(
            "git-merge",
            "",
            vec![
                text("Conflicted with "),
                code(branch),
                text("; back to its agent to resolve"),
            ],
            &files.join("\n"),
            false,
        ),
        _ => None,
    }
}

/// Where an entry of the merged history came from.
enum Source<'a> {
    Event(&'a Event),
    Log(Logged),
}

/// The ticket's history as a feed: its events and its run log, by time.
pub fn feed(ticket: &Ticket, filer: &Filer, events: &[Stamped], log: &str) -> Feed {
    let logged = parse_log(log);
    // The log's asks, each to be matched with its event once.
    let mut owner_asks: Vec<u64> = logged
        .iter()
        .filter(|(_, l)| matches!(l, Logged::Asked { .. }))
        .map(|(at, _)| *at)
        .collect();
    let failed_checks: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.event {
            Event::ChecksFailed { detail } => Some(detail.as_str()),
            _ => None,
        })
        .collect();
    // On a tie the event goes first: a claim comes before its run's log.
    let mut merged: Vec<(u64, Source)> = events
        .iter()
        .map(|e| (e.at_ms, Source::Event(&e.event)))
        .chain(logged.into_iter().map(|(at, l)| (at, Source::Log(l))))
        .collect();
    merged.sort_by_key(|(at, _)| *at);

    let hire = ticket.hire.to_string();
    let mut feed = Feed::default();
    let mut speaker = hire.clone();
    let mut model: Option<String> = None;
    // A reviewer or a critic is running; it can't ask or change anything.
    let mut reviewing = false;
    let mut claims = 0;
    // A call's id, and where its step is.
    let mut calls: HashMap<String, (usize, usize, String)> = HashMap::new();
    let items = &mut feed.items;
    for (at, source) in merged {
        match source {
            Source::Event(event) => {
                if matches!(event, Event::Claimed) {
                    claims += 1;
                }
                if matches!(event, Event::AskOpened { .. }) {
                    // A gate or a change asks without a line in the log.
                    let logged = owner_asks
                        .iter()
                        .rposition(|a| *a <= at + 1_000 && at.saturating_sub(*a) < ASK_LEAD_MS);
                    match logged {
                        Some(index) => {
                            owner_asks.remove(index);
                        }
                        None => {
                            let parts = vec![bold("Asked you")];
                            items.push(Item::Line(line(at, "shield-alert", "attn", parts)));
                        }
                    }
                }
                if let Some(line) = event_line(at, event, ticket, filer, claims) {
                    items.push(Item::Line(line));
                }
            }
            Source::Log(Logged::Started { model: started }) => {
                if started.is_some() {
                    model = started;
                }
                if !reviewing {
                    feed.model.clone_from(&model);
                }
            }
            Source::Log(Logged::Text(message)) => {
                if message.trim().is_empty() {
                    continue;
                }
                items.push(Item::Turn(Turn {
                    at,
                    hire: speaker.clone(),
                    model: model.clone(),
                    message: Some(message),
                    steps: Vec::new(),
                }));
            }
            Source::Log(Logged::Call { id, name, input }) => {
                let at_step = push_step(items, at, &speaker, &model, call_step(&name, &input));
                calls.insert(id, (at_step.0, at_step.1, name));
            }
            Source::Log(Logged::Result { id, output, error }) => {
                let Some((item, step, name)) = calls.get(&id) else {
                    continue;
                };
                if let Some(Item::Turn(turn)) = items.get_mut(*item)
                    && let Some(step) = turn.steps.get_mut(*step)
                {
                    step.result = result_of(step, name, &output, error);
                }
            }
            Source::Log(Logged::Asked {
                tool,
                input,
                answer,
            }) => {
                if !reviewing {
                    let parts = asked_parts(&tool, &input, &answer);
                    items.push(Item::Line(line(at, "shield-alert", "attn", parts)));
                }
            }
            Source::Log(Logged::Note(note)) => {
                if let Some(step) = rule_step(&note) {
                    push_step(items, at, &speaker, &model, step);
                    continue;
                }
                let starts = |p: &str| note.starts_with(p);
                if starts("review by ") || (starts("gate ") && note.contains(" by ")) {
                    let who = note.rsplit_once(" by ").map_or("", |(_, w)| w);
                    let (hire, ran_on) = hire_and_model(who);
                    speaker = hire;
                    model = ran_on;
                    reviewing = true;
                }
                if let Some(line) = note_line(at, &note, &speaker, &failed_checks) {
                    items.push(Item::Line(line));
                }
            }
            Source::Log(Logged::Finished { outcome }) => {
                let ended = match outcome.as_str() {
                    "success" => None,
                    "max_turns" => Some("ran out of turns".to_string()),
                    "budget" => Some("reached its budget".to_string()),
                    other => Some(other.replace('_', " ")),
                };
                if let Some(ended) = ended {
                    let parts = vec![
                        bold(speaker.clone()),
                        text(format!("'s run ended: {ended}")),
                    ];
                    items.push(Item::Line(line(at, "info", "", parts)));
                }
                // The ticket's own hire speaks next, on its own model.
                speaker.clone_from(&hire);
                model.clone_from(&feed.model);
                reviewing = false;
            }
        }
    }
    feed
}

/// Adds a step to the speaker's current turn, or starts one; returns where
/// it went.
fn push_step(
    items: &mut Vec<Item>,
    at: u64,
    speaker: &str,
    model: &Option<String>,
    step: Step,
) -> (usize, usize) {
    let joins = matches!(items.last(), Some(Item::Turn(t)) if t.hire == speaker);
    if !joins {
        items.push(Item::Turn(Turn {
            at,
            hire: speaker.to_string(),
            model: model.clone(),
            message: None,
            steps: Vec::new(),
        }));
    }
    let index = items.len() - 1;
    match items.last_mut() {
        Some(Item::Turn(turn)) => {
            turn.steps.push(step);
            (index, turn.steps.len() - 1)
        }
        _ => (index, 0),
    }
}

/// "Read 2 files, searched once, edited 2 files, ran 1 command".
fn steps_summary(steps: &[Step]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for step in steps {
        match counts.iter_mut().find(|(k, _)| *k == step.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((step.kind, 1)),
        }
    }
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            one.to_string()
        } else {
            format!("{n} {many}")
        }
    };
    let phrases: Vec<String> = counts
        .into_iter()
        .filter_map(|(kind, n)| {
            Some(match kind {
                "read" => format!("read {}", plural(n, "1 file", "files")),
                "edit" => format!("edited {}", plural(n, "1 file", "files")),
                "write" => format!("wrote {}", plural(n, "1 file", "files")),
                "run" => format!("ran {}", plural(n, "a command", "commands")),
                "search" => format!("searched {}", plural(n, "once", "times")),
                "blocked" => format!("{n} blocked"),
                "other" => plural(n, "1 other step", "other steps"),
                _ => return None,
            })
        })
        .collect();
    let joined = phrases.join(", ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => joined,
    }
}

// The feed's pieces, after the prototype's `.ev`.
const EV: &str = "relative grid grid-cols-[24px_minmax(0,1fr)] gap-x-3.5 pb-5 before:absolute before:bottom-1 before:left-[11.5px] before:top-7 before:w-px before:bg-line last:before:hidden";
const HEAD: &str = "min-h-6 pt-0.5 text-[13px] leading-5 text-fg-3 [overflow-wrap:anywhere]";
const MESSAGE: &str = "mt-1 max-w-[68ch] leading-[22px] text-fg";

fn circle(tone: &str) -> &'static str {
    match tone {
        "attn" => {
            "grid size-6 place-items-center rounded-full border border-attn-line bg-attn-bg text-attn-fg [&_.i]:size-[13px]"
        }
        "ok" => {
            "grid size-6 place-items-center rounded-full border border-ok-line bg-ok-bg text-ok-fg [&_.i]:size-[13px]"
        }
        "bad" => {
            "grid size-6 place-items-center rounded-full border border-bad-line bg-bad-bg text-bad-fg [&_.i]:size-[13px]"
        }
        _ => {
            "grid size-6 place-items-center rounded-full border border-line bg-bg-2 text-fg-3 [&_.i]:size-[13px]"
        }
    }
}

/// The feed under its heading; `state` sets the note on the right, and a
/// `live` ticket opens its latest group of steps.
#[component]
pub async fn activity(
    items: Vec<Item>,
    state: &'static str,
    live: bool,
) -> topcoat::Result<impl View> {
    let now = ui::now_ms();
    let latest = items
        .iter()
        .rposition(|i| matches!(i, Item::Turn(t) if !t.steps.is_empty()));
    let look = ui::look(state);
    let running = ui::run_step(state).is_some();
    Ok(view! {
        <section id="activity" aria-label="Activity">
            <div class="mb-3 flex items-center gap-2">
                <h2 class=(ui::SECTION_TITLE)>"Activity"</h2>
                if state == "asking" {
                    <span class="ml-auto inline-flex items-center gap-1.5 text-[12.5px] text-fg-3">
                        <span class="size-2 rounded-full bg-attn"></span>
                        "Waiting for you"
                    </span>
                } else if running {
                    <span class="ml-auto inline-flex items-center gap-1.5 text-[12.5px] text-fg-3">
                        <span class="pulse"></span>
                        (look.label)
                    </span>
                }
            </div>
            if items.is_empty() {
                <p class=(ui::MUTED)>"Nothing yet."</p>
            }
            <ol class="flex flex-col">
                for (index, item) in items.into_iter().enumerate() {
                    match item {
                        Item::Line(line) => line_item(line: line, id: format!("act-{index}"), now: now),
                        Item::Turn(turn) => turn_item(
                            turn: turn,
                            id: format!("act-{index}"),
                            open: live && latest == Some(index),
                            now: now
                        ),
                    }
                }
            </ol>
        </section>
    })
}

#[component]
async fn phrase(parts: Vec<Part>) -> topcoat::Result<impl View> {
    Ok(view! {
        for part in parts {
            match part {
                Part::Text(words) => <span>(words)</span>,
                Part::Bold(words) => <b class="font-medium text-fg">(words)</b>,
                Part::Code(words) => <span class=(format!("{} inline-block max-w-full truncate align-bottom", ui::CODE))>(words)</span>,
                Part::Model(model) => <span class="mx-1">ui::model_tag(model: model)</span>,
            }
        }
    })
}

#[component]
async fn when(at: u64, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <span title=(exact(at))>" · " (relative(now, at))</span>
    })
}

#[component]
async fn line_item(line: Line, id: String, now: u64) -> topcoat::Result<impl View> {
    let Line {
        at,
        icon,
        tone,
        parts: words,
        detail,
    } = line;
    Ok(view! {
        <li class=(EV)>
            <span class=(circle(tone))>ui::icon(name: icon)</span>
            <div class="min-w-0">
                <p class=(HEAD)>
                    phrase(parts: words)
                    when(at: at, now: now)
                </p>
                match detail {
                    Some((text, true)) => <div class=(format!("{MESSAGE} mt-1.5 border-l-2 border-line-2 pl-3 text-fg-2 {}", markdown::STYLE))>(markdown::render(&text))</div>,
                    Some((text, false)) => <details id=(id) data-preserve-attr="open" class="group mt-1">
                        <summary class="flex cursor-pointer select-none list-none items-center gap-1 text-[12.5px] text-fg-3 hover:text-fg [&::-webkit-details-marker]:hidden">
                            <span class="flex transition-transform group-open:rotate-90">ui::icon(name: "chevron-right", small: true)</span>
                            "Details"
                        </summary>
                        <pre class="mt-1.5 max-h-72 overflow-auto rounded-lg border border-line bg-bg-2 px-3 py-2 font-mono text-[12px] leading-[18px] whitespace-pre-wrap text-fg-2">(text)</pre>
                    </details>,
                    None => <span hidden=""></span>,
                }
            </div>
        </li>
    })
}

#[component]
async fn turn_item(turn: Turn, id: String, open: bool, now: u64) -> topcoat::Result<impl View> {
    let Turn {
        at,
        hire,
        model,
        message,
        steps,
    } = turn;
    let summary = steps_summary(&steps);
    let count = steps.len();
    Ok(view! {
        <li class=(EV)>
            ui::avatar(hire: hire.clone(), large: true)
            <div class="min-w-0">
                <p class=(HEAD)>
                    <b class="font-medium text-fg">(hire)</b>
                    if let Some(model) = model {
                        <span class="mx-1">ui::model_tag(model: model)</span>
                    }
                    when(at: at, now: now)
                </p>
                if let Some(message) = message {
                    <div class=(format!("{MESSAGE} {}", markdown::STYLE))>(markdown::render(&message))</div>
                }
                if count > 0 {
                    <details id=(id) data-preserve-attr="open" open=(open.then_some("")) class="group mt-2 overflow-hidden rounded-lg border border-line">
                        <summary class="flex cursor-pointer list-none items-center gap-2 bg-bg-2 px-3 py-2 text-[13px] text-fg-2 [&::-webkit-details-marker]:hidden">
                            <span class="text-fg-3 transition-transform group-open:rotate-90">ui::icon(name: "chevron-right", small: true)</span>
                            <span class="whitespace-nowrap font-medium text-fg">(count) if count == 1 { " step" } else { " steps" }</span>
                            <span class="min-w-0 truncate text-fg-3">(summary)</span>
                        </summary>
                        for step in steps {
                            step_row(step: step)
                        }
                    </details>
                }
            </div>
        </li>
    })
}

#[component]
async fn step_row(step: Step) -> topcoat::Result<impl View> {
    let Step {
        icon,
        label,
        arg,
        raw,
        result,
        ..
    } = step;
    Ok(view! {
        <div title=(raw) class="grid grid-cols-[16px_52px_minmax(0,1fr)_auto] items-center gap-2.5 border-t border-line px-3 py-[7px] text-[12.5px]">
            <span class="text-fg-3">ui::icon(name: icon, small: true)</span>
            <span class="font-medium text-fg-2">(label)</span>
            <span class="truncate font-mono text-[12px] leading-[18px] text-fg-2">(arg)</span>
            <span class="whitespace-nowrap text-fg-3 tabular-nums">
                match result {
                    Done::Nothing => <span></span>,
                    Done::Plain(words) => <span>(words)</span>,
                    Done::Good(words) => <span class=(ui::ADD)>(words)</span>,
                    Done::Bad(words) => <span class=(ui::DEL)>(words)</span>,
                    Done::Lines(added, removed) => ui::diff_size(added: added, removed: removed),
                }
            </span>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use topcoat::context::Cx;

    fn ticket(extra: Value) -> Ticket {
        let mut base = json!({
            "id": "GAME-3", "project": "game", "title": "Waves", "goal": "Enemy waves",
            "hire": "dev", "state": "working", "attempts": 0, "base_branch": "main",
            "created_at": 0,
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value(base).unwrap()
    }

    fn log(entries: &[Value]) -> String {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut e = e.clone();
                e["at"] = json!(1_000 + i as u64 * 1_000);
                e.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn call(id: &str, name: &str, input: Value) -> Value {
        json!({"event": {"type": "tool_call", "id": id, "name": name, "input": input}})
    }

    fn result(id: &str, output: &str) -> Value {
        json!({"event": {"type": "tool_result", "id": id, "output": output, "is_error": false}})
    }

    fn turns(feed: &Feed) -> Vec<&Turn> {
        feed.items
            .iter()
            .filter_map(|i| match i {
                Item::Turn(t) => Some(t),
                Item::Line(_) => None,
            })
            .collect()
    }

    #[test]
    fn calls_fold_under_the_message_before_them_with_their_results() {
        let text = log(&[
            json!({"event": {"type": "started", "session": "s", "model": "sonnet"}}),
            json!({"event": {"type": "text", "text": "Looking at **spawning**"}}),
            call("1", "Read", json!({"file_path": "src/spawning.rs"})),
            result("1", "a\nb\nc"),
            call(
                "2",
                "Edit",
                json!({"file_path": "src/rules.rs", "old_string": "a\nb\nc", "new_string": "a\nx\ny\nc"}),
            ),
            json!({"note": "allowed Edit: inside the worktree"}),
            call("3", "Bash", json!({"command": "cargo test"})),
            json!({"event": {"type": "permission", "tool": "Bash", "input": {"command": "cargo test"}}}),
            json!({"note": "approver allowed Bash: tests run inside the worktree"}),
            result(
                "3",
                "test result: ok. 12 passed; 0 failed\ntest result: ok. 2 passed; 0 failed",
            ),
        ]);
        let feed = feed(&ticket(json!({})), &Filer::Owner, &[], &text);
        assert_eq!(feed.model.as_deref(), Some("sonnet"));
        let turns = turns(&feed);
        assert_eq!(turns.len(), 1);
        let turn = turns[0];
        assert_eq!(turn.message.as_deref(), Some("Looking at **spawning**"));
        let shown: Vec<(&str, &str, &Done)> = turn
            .steps
            .iter()
            .map(|s| (s.label.as_str(), s.arg.as_str(), &s.result))
            .collect();
        assert_eq!(
            shown,
            [
                ("Read", "src/spawning.rs", &Done::Plain("3 lines".into())),
                ("Edit", "src/rules.rs", &Done::Lines(2, 1)),
                ("Run", "cargo test", &Done::Good("14 passed".into())),
                (
                    "Allowed",
                    "approver: tests run inside the worktree",
                    &Done::Nothing
                ),
            ]
        );
        assert_eq!(
            steps_summary(&turn.steps),
            "Read 1 file, edited 1 file, ran a command"
        );
    }

    #[test]
    fn an_owner_ask_is_its_own_line_with_its_answer() {
        let ask = json!({"event": {"type": "permission", "tool": "Bash",
                                   "input": {"command": "cargo run --example waves"}}});
        let waiting = log(&[
            json!({"event": {"type": "text", "text": "I want to watch it"}}),
            call("1", "Bash", json!({"command": "cargo run --example waves"})),
            ask.clone(),
        ]);
        let feed = feed(&ticket(json!({})), &Filer::Owner, &[], &waiting);
        let Some(Item::Line(asked)) = feed.items.last() else {
            panic!("{:?}", feed.items);
        };
        assert_eq!(asked.tone, "attn");
        assert!(
            asked
                .parts
                .contains(&Part::Code("cargo run --example waves".into()))
        );
        assert!(asked.parts.contains(&text(" · waiting")));
        assert_eq!(last_words(&waiting).as_deref(), Some("I want to watch it"));

        let denied = log(&[ask, json!({"note": "denied, run continues: not now"})]);
        let feed = super::feed(&ticket(json!({})), &Filer::Owner, &[], &denied);
        let Some(Item::Line(asked)) = feed.items.first() else {
            panic!("{:?}", feed.items);
        };
        assert!(asked.parts.contains(&text(" · you denied it: not now")));
        assert_eq!(feed.items.len(), 1);
    }

    #[test]
    fn events_and_log_merge_by_time() {
        let events = [
            Stamped {
                at_ms: 500,
                event: Event::Claimed,
            },
            Stamped {
                at_ms: 2_500,
                event: Event::ChecksFailed {
                    detail: "boom".into(),
                },
            },
        ];
        let text = log(&[
            json!({"note": "routed to dev: game logic"}),
            json!({"event": {"type": "text", "text": "Hi"}}),
            json!({"note": "boom"}),
            json!({"note": "review by reviewer (opus)"}),
            json!({"event": {"type": "text", "text": "VERDICT: pass"}}),
            json!({"note": "review passed"}),
            json!({"event": {"type": "finished", "outcome": {"outcome": "success"}}}),
            json!({"event": {"type": "text", "text": "Back"}}),
        ]);
        let feed = feed(&ticket(json!({})), &Filer::Owner, &events, &text);
        let heads: Vec<String> = feed
            .items
            .iter()
            .map(|i| match i {
                Item::Line(l) => format!("{:?}", l.parts.first()),
                Item::Turn(t) => format!("{} {:?}", t.hire, t.model),
            })
            .collect();
        assert_eq!(
            heads,
            [
                r#"Some(Bold("router"))"#,
                "dev None",
                r#"Some(Text("Checks failed"))"#,
                r#"Some(Bold("reviewer"))"#,
                r#"reviewer Some("opus")"#,
                r#"Some(Bold("reviewer"))"#,
                "dev None",
            ]
        );
    }

    #[test]
    fn the_filer_comes_from_the_parent_or_the_source() {
        let plan = ticket(json!({"id": "GAME-1", "hire": "producer", "kind": "plan"}));
        let child = ticket(json!({"parent": "GAME-1"}));
        let tickets = [plan.clone(), child.clone()];
        assert_eq!(filer(&child, &tickets), Filer::Hire("producer".into()));
        assert_eq!(
            filed_parts(&child, &filer(&child, &tickets)),
            [
                bold("producer"),
                text(" filed this"),
                text(" from "),
                bold("Enemy waves")
            ]
        );
        let ci = ticket(json!({"source": "ci:12"}));
        assert_eq!(filer(&ci, &tickets), Filer::Studio("ci:12".into()));
        let merge = ticket(json!({"source": "merge:master"}));
        assert_eq!(
            filed_parts(&merge, &filer(&merge, &tickets))[..2],
            [
                bold("studio"),
                text(" filed this when merging the goal conflicted")
            ]
        );
        assert_eq!(filer(&plan, &tickets), Filer::Owner);
        assert_eq!(
            filed_parts(&plan, &Filer::Owner),
            [bold("You"), text(" set this goal")]
        );
    }

    #[test]
    fn a_conflict_says_what_it_conflicted_with_and_where() {
        let events = [Stamped {
            at_ms: 500,
            event: Event::Conflicted {
                branch: "studio/goal/first-level".into(),
                files: vec!["README.md".into(), "src/a.rs".into()],
            },
        }];
        let feed = feed(&ticket(json!({})), &Filer::Owner, &events, "");
        let Some(Item::Line(line)) = feed.items.first() else {
            panic!("expected a line: {:?}", feed.items);
        };
        assert_eq!(
            line.parts,
            [
                text("Conflicted with "),
                code("studio/goal/first-level"),
                text("; back to its agent to resolve")
            ]
        );
        assert_eq!(
            line.detail,
            Some(("README.md\nsrc/a.rs".to_string(), false))
        );
    }

    #[test]
    fn edits_count_only_the_lines_that_changed() {
        assert_eq!(changed_lines("a\nb\nc", "a\nb\nc"), (0, 0));
        assert_eq!(changed_lines("a\nb", "a\nb\nc\nd"), (2, 0));
        assert_eq!(changed_lines("", "x"), (1, 0));
        assert_eq!(changed_lines("x\ny", "z"), (1, 2));
    }

    #[tokio::test]
    async fn the_feed_renders_escaped_with_groups_that_stay_open() {
        let text = log(&[
            json!({"event": {"type": "text", "text": "**done** <script>x</script>"}}),
            call("1", "Read", json!({"file_path": "a.rs"})),
        ]);
        let feed = feed(&ticket(json!({})), &Filer::Owner, &[], &text);
        let cx = Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => activity(items: feed.items, state: "working", live: true) },
        )
        .await
        .unwrap();
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("<strong>done</strong>"), "{html}");
        assert!(html.contains(r#"id="act-0""#), "{html}");
        assert!(html.contains(r#"data-preserve-attr="open""#), "{html}");
        assert!(html.contains(" open"), "{html}");
        assert!(html.contains("1 step"), "{html}");
        // A running ticket says what it is doing, next to the title.
        assert!(html.contains("Working"), "{html}");
    }
}
