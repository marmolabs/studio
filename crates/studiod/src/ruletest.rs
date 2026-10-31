//! Tries a rule on the tool calls agents already made, from the run logs.

use crate::api::{RuleHit, RuleTest};
use crate::studio::{Studio, StudioError, other};
use crate::worker::tool_verdict;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use studio_core::policy::Context;
use studio_core::rule::{Rule, RuleKind};
use studio_core::ticket::Ticket;

const MAX_HITS: usize = 200;
const WHAT_CHARS: usize = 200;
const WORKTREES: &str = ".studio/worktrees/";

/// Replays `text` against every ticket's logged tool calls made at or after
/// `since`.
pub(crate) fn run(
    studio: &Studio,
    tickets: &[Ticket],
    text: &str,
    since: u64,
) -> Result<RuleTest, StudioError> {
    studio_config::check_rule("draft", text).map_err(other)?;
    let rule = studio_config::parse_studio_rule("draft", text).map_err(other)?;
    if !matches!(
        rule.kind,
        RuleKind::Gate | RuleKind::Protect | RuleKind::Lint
    ) {
        return Err(other(format!(
            "only gate, protect and lint rules act on tool calls; this is a {} rule",
            snake(&rule.kind)
        )));
    }
    let mut result = RuleTest {
        rule: format!("{}, {}", snake(&rule.kind), snake(&rule.level)),
        scanned: 0,
        hits: Vec::new(),
    };
    let Ok(entries) = fs::read_dir(studio.paths.data_dir.join("runs")) else {
        return Ok(result);
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    logs.sort();
    for log in logs {
        let modified = fs::metadata(&log).and_then(|m| m.modified()).ok();
        if modified.is_some_and(|m| m < UNIX_EPOCH + Duration::from_millis(since)) {
            continue;
        }
        let Some(ticket) = log
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|id| tickets.iter().find(|t| t.id.to_string() == id))
        else {
            continue;
        };
        let Ok(lines) = fs::read_to_string(&log) else {
            continue;
        };
        let role = studio
            .hire(&ticket.hire)
            .map(|h| h.role)
            .unwrap_or_default();
        let ctx = Context {
            project: &ticket.project.0,
            hire: &ticket.hire.0,
            role: &role,
        };
        let project = studio.project(&ticket.project).map(|p| p.path);
        let root = project
            .unwrap_or_default()
            .join(WORKTREES)
            .join(ticket.id.to_string().to_lowercase());
        let (scanned, hits) = replay(&rule, &ctx, &root, &ticket.id.to_string(), since, &lines);
        result.scanned += scanned;
        let room = MAX_HITS.saturating_sub(result.hits.len());
        result.hits.extend(hits.into_iter().take(room));
    }
    Ok(result)
}

/// How many tool calls in a run log's `lines` were made at or after `since`,
/// and which of them `rule` acts on.
fn replay(
    rule: &Rule,
    ctx: &Context,
    root: &Path,
    ticket: &str,
    since: u64,
    lines: &str,
) -> (usize, Vec<RuleHit>) {
    let mut scanned = 0;
    let mut hits = Vec::new();
    for line in lines.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let at = value["at"].as_u64().unwrap_or(0);
        let event = &value["event"];
        if event["type"] != "tool_call" || at < since {
            continue;
        }
        let (Some(tool), input) = (event["name"].as_str(), &event["input"]) else {
            continue;
        };
        scanned += 1;
        let root = worktree_in(input).unwrap_or_else(|| root.to_path_buf());
        let verdict = tool_verdict(std::slice::from_ref(rule), ctx, &root, tool, input);
        if let Some(finding) = verdict.findings.first() {
            hits.push(RuleHit {
                ticket: ticket.to_string(),
                at,
                tool: tool.to_string(),
                what: what(input),
                level: snake(&finding.level),
            });
        }
    }
    (scanned, hits)
}

/// The worktree folder a path in `input` is inside, if any does.
fn worktree_in(input: &Value) -> Option<PathBuf> {
    ["file_path", "path", "notebook_path"]
        .iter()
        .filter_map(|key| input[*key].as_str())
        .find_map(|path| {
            let (head, rest) = path.split_once(WORKTREES)?;
            let folder = rest.split('/').next().filter(|s| !s.is_empty())?;
            Some(Path::new(head).join(WORKTREES).join(folder))
        })
}

/// The command or path of a tool call, else its input cut short.
fn what(input: &Value) -> String {
    let named = ["command", "file_path", "path", "notebook_path"]
        .iter()
        .find_map(|key| input[*key].as_str());
    let text = named.map_or_else(|| input.to_string(), str::to_string);
    text.chars().take(WHAT_CHARS).collect()
}

/// A rule kind or level as its snake case name.
fn snake(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(name)) => name,
        _ => String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(at: u64, name: &str, input: Value) -> String {
        json!({"at": at, "event": {"type": "tool_call", "id": "t", "name": name, "input": input}})
            .to_string()
    }

    fn rule(text: &str) -> Rule {
        studio_config::parse_studio_rule("draft", text).unwrap()
    }

    #[test]
    fn replay_counts_tool_calls_and_finds_matches() {
        let gate = rule("---\nkind: gate\nlevel: block\naction: \"bash:git push\"\n---\nNo.\n");
        let logs = [
            line(10, "Bash", json!({"command": "git push origin main"})),
            line(20, "Bash", json!({"command": "cargo test"})),
            line(30, "Bash", json!({"command": "git push"})),
            json!({"at": 40, "event": {"type": "text", "text": "hi"}}).to_string(),
            "not json".to_string(),
        ]
        .join("\n");
        let ctx = Context {
            project: "game",
            hire: "dev",
            role: "engineer",
        };
        let (scanned, hits) = replay(&gate, &ctx, Path::new("/p"), "GAME-1", 15, &logs);
        assert_eq!(scanned, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].what, "git push");
        assert_eq!(hits[0].level, "block");
        assert_eq!(hits[0].ticket, "GAME-1");
        assert_eq!(hits[0].at, 30);
    }

    #[test]
    fn replay_makes_paths_relative_to_the_worktree_in_them() {
        let protect = rule(
            "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"src/main.rs\"]\n---\nHands off.\n",
        );
        let path = "/p/.studio/worktrees/game-1/src/main.rs";
        let other = "/p/.studio/worktrees/game-1/src/lib.rs";
        let logs = [
            line(1, "Edit", json!({"file_path": path})),
            line(2, "Edit", json!({"file_path": other})),
        ]
        .join("\n");
        let ctx = Context {
            project: "game",
            hire: "dev",
            role: "",
        };
        let (scanned, hits) = replay(&protect, &ctx, Path::new("/elsewhere"), "GAME-1", 0, &logs);
        assert_eq!(scanned, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].what, path);
        assert_eq!(hits[0].level, "ask");
    }

    #[test]
    fn what_cuts_other_inputs() {
        let long = "x".repeat(500);
        assert_eq!(what(&json!({"url": long})).chars().count(), WHAT_CHARS);
    }
}
