//! What the run logs say about the tickets the office shows: a live run's
//! latest steps and checks, and whether a ready ticket went unreviewed. Only
//! the end of a log is read, so a long log costs no more than a short one.

use crate::api::{CheckView, RUN_CALLS, RunView, StepView};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// How much of a log's end a live run's steps are read from.
const STEPS_TAIL: u64 = 256 * 1024;
/// How far back from a log's end a ready ticket's review is looked for.
const REVIEW_TAIL: u64 = 16 * 1024 * 1024;
/// The most characters of a message or of one text field of a tool's input.
const CUT: usize = 400;
/// A part of a tool's input over this many bytes is left out.
const WHOLE: usize = 2000;
/// The note that says no reviewer on another model read the work.
const UNREVIEWED: &str = "no reviewer on a different model";

/// Where a ticket's run log is.
fn log_path(runs: &Path, ticket: &str) -> PathBuf {
    runs.join(format!("{ticket}.jsonl"))
}

/// The last `most` bytes of the file at `path` as whole lines; empty when it
/// can't be read.
fn tail(path: &Path, most: u64) -> String {
    let read = || -> std::io::Result<String> {
        let mut file = File::open(path)?;
        let start = file.metadata()?.len().saturating_sub(most);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        // A read that began mid-line leaves out that line.
        Ok(match text.split_once('\n') {
            Some((_, rest)) if start > 0 => rest.to_string(),
            _ if start > 0 => String::new(),
            _ => text,
        })
    };
    read().unwrap_or_default()
}

/// What the run of `ticket` is doing: for one in checks, the checks it has
/// started; for any other, its latest steps.
pub(crate) fn run_of(runs: &Path, ticket: &str, checking: bool) -> RunView {
    let text = tail(&log_path(runs, ticket), STEPS_TAIL);
    if checking {
        RunView {
            checks: checks_of(&text),
            ..RunView::default()
        }
    } else {
        steps_of(&text)
    }
}

/// `text` on one line, cut to `CUT` characters.
fn one_line(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.join(" ").chars().take(CUT).collect()
}

/// A tool's input with what a line of words never needs left out: each text
/// field cut, and any other part over `WHOLE` bytes dropped.
fn slim(input: &Value) -> Value {
    let part = |value: &Value| match value {
        Value::String(text) => Value::String(text.chars().take(CUT).collect()),
        other if other.to_string().len() <= WHOLE => other.clone(),
        _ => Value::Null,
    };
    match input {
        Value::Object(fields) => {
            Value::Object(fields.iter().map(|(k, v)| (k.clone(), part(v))).collect())
        }
        other => part(other),
    }
}

/// The latest steps a log's text shows: its newest tool call or message, its
/// last `RUN_CALLS` tool calls and its newest words.
fn steps_of(text: &str) -> RunView {
    let mut run = RunView::default();
    for raw in text.lines().rev() {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let event = &entry["event"];
        let at = entry["at"].as_u64().unwrap_or(0);
        match event["type"].as_str() {
            Some("tool_call") => {
                let Some(tool) = event["name"].as_str() else {
                    continue;
                };
                let step = StepView {
                    tool: Some(tool.to_string()),
                    input: slim(&event["input"]),
                    text: String::new(),
                    at,
                };
                if run.latest.is_none() {
                    run.latest = Some(step.clone());
                }
                if run.calls.len() < RUN_CALLS {
                    run.calls.push(step);
                }
            }
            Some("text") => {
                let words = one_line(event["text"].as_str().unwrap_or_default());
                if words.is_empty() {
                    continue;
                }
                if run.latest.is_none() {
                    run.latest = Some(StepView {
                        tool: None,
                        input: Value::Null,
                        text: words.clone(),
                        at,
                    });
                }
                run.said.get_or_insert(words);
            }
            _ => {}
        }
        if run.latest.is_some() && run.calls.len() == RUN_CALLS && run.said.is_some() {
            break;
        }
    }
    run.calls.reverse();
    run
}

/// The project checks at the end of a log, oldest first: the run of "check:"
/// notes that nothing else follows, which are the checks of the try under
/// way.
fn checks_of(text: &str) -> Vec<CheckView> {
    let mut checks = Vec::new();
    for raw in text.lines().rev() {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let Some(command) = entry["note"]
            .as_str()
            .and_then(|note| note.strip_prefix("check: "))
        else {
            break;
        };
        checks.push(CheckView {
            command: command.to_string(),
            at: entry["at"].as_u64().unwrap_or(0),
        });
    }
    checks.reverse();
    checks
}

/// Whether the latest review of the work, since its checks last passed, had
/// no reviewer on another model.
fn unreviewed_in(text: &str) -> bool {
    for raw in text.lines().rev() {
        if !raw.contains("\"note\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        match entry["note"].as_str() {
            Some("checks passed") => return false,
            Some(note) if note.starts_with(UNREVIEWED) => return true,
            _ => {}
        }
    }
    false
}

/// Whether the ready ticket's work went unreviewed. A ready ticket's log no
/// longer grows, so the answer is kept for as long as the log keeps its size.
pub(crate) fn unreviewed(runs: &Path, ticket: &str) -> bool {
    static SEEN: Mutex<BTreeMap<PathBuf, (u64, bool)>> = Mutex::new(BTreeMap::new());
    let path = log_path(runs, ticket);
    let len = std::fs::metadata(&path).map_or(0, |m| m.len());
    let known = SEEN
        .lock()
        .ok()
        .and_then(|seen| seen.get(&path).copied())
        .filter(|(size, _)| *size == len);
    if let Some((_, answer)) = known {
        return answer;
    }
    let answer = unreviewed_in(&tail(&path, REVIEW_TAIL));
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(path, (len, answer));
    }
    answer
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn the_latest_steps_come_from_the_end_of_the_log() {
        let log = [
            r#"{"at":1,"event":{"type":"text","text":"Looking  around"}}"#,
            r#"{"at":2,"event":{"type":"tool_call","name":"Read","input":{"file_path":"a.rs"}}}"#,
            r#"{"at":3,"event":{"type":"tool_call","name":"Grep","input":{"pattern":"x"}}}"#,
            r#"{"at":4,"event":{"type":"tool_call","name":"Edit","input":{"file_path":"b.rs"}}}"#,
            r#"{"at":5,"event":{"type":"tool_call","name":"Bash","input":{"command":"cargo test"}}}"#,
            r#"{"at":6,"event":{"type":"tool_result","output":"ok"}}"#,
            "not json",
        ]
        .join("\n");
        let run = steps_of(&log);
        let latest = run.latest.unwrap();
        assert_eq!(latest.tool.as_deref(), Some("Bash"));
        assert_eq!(latest.input, json!({"command": "cargo test"}));
        assert_eq!(latest.at, 5);
        let calls: Vec<(&str, u64)> = run
            .calls
            .iter()
            .map(|c| (c.tool.as_deref().unwrap(), c.at))
            .collect();
        assert_eq!(calls, [("Grep", 3), ("Edit", 4), ("Bash", 5)]);
        assert_eq!(run.said.as_deref(), Some("Looking around"));
        assert_eq!(steps_of(""), RunView::default());
    }

    #[test]
    fn a_message_can_be_the_latest_step() {
        let log = [
            r#"{"at":1,"event":{"type":"tool_call","name":"Read","input":{}}}"#,
            r#"{"at":9,"event":{"type":"text","text":"  Reading  it "}}"#,
            r#"{"at":12,"note":"checks passed"}"#,
        ]
        .join("\n");
        let run = steps_of(&log);
        let latest = run.latest.unwrap();
        assert_eq!(
            (latest.tool, latest.text.as_str(), latest.at),
            (None, "Reading it", 9)
        );
        assert_eq!(run.calls.len(), 1);
    }

    #[test]
    fn long_inputs_are_cut() {
        let long = "x".repeat(5000);
        let input = json!({"file_path": "a.rs", "content": long, "edits": [long]});
        let slim = slim(&input);
        assert_eq!(slim["file_path"], "a.rs");
        assert_eq!(slim["content"].as_str().unwrap().len(), CUT);
        assert_eq!(slim["edits"], Value::Null);
        assert_eq!(one_line(&long).len(), CUT);
    }

    #[test]
    fn checks_are_the_notes_at_the_end() {
        let log = [
            r#"{"at":10,"note":"check: cargo fmt --check"}"#,
            r#"{"at":20,"note":"checks passed"}"#,
            r#"{"at":50,"note":"check: cargo fmt --check"}"#,
            r#"{"at":54,"note":"check: cargo test"}"#,
        ]
        .join("\n");
        let checks: Vec<(String, u64)> = checks_of(&log)
            .into_iter()
            .map(|c| (c.command, c.at))
            .collect();
        assert_eq!(
            checks,
            [
                ("cargo fmt --check".to_string(), 50),
                ("cargo test".to_string(), 54)
            ]
        );
        assert!(checks_of("").is_empty());
    }

    #[test]
    fn the_latest_review_says_whether_the_owner_reviews() {
        let none = r#"{"at":3,"note":"no reviewer on a different model; the owner reviews"}"#;
        let passed = r#"{"at":1,"note":"checks passed"}"#;
        let reviewed = r#"{"at":2,"note":"review passed"}"#;
        assert!(unreviewed_in(&[passed, none].join("\n")));
        assert!(!unreviewed_in(&[passed, reviewed].join("\n")));
        // An earlier try's missing reviewer doesn't count against this one.
        assert!(!unreviewed_in(&[none, passed, reviewed].join("\n")));
        assert!(!unreviewed_in(""));
    }

    #[test]
    fn only_the_end_of_a_long_log_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("T-1.jsonl");
        let filler = format!("{}\n", json!({"at": 1, "note": "x".repeat(100)}));
        let last = format!("{}\n", json!({"at": 2, "note": "last"}));
        std::fs::write(&path, format!("{}{last}", filler.repeat(5000))).unwrap();
        let text = tail(&path, 1000);
        assert!(text.len() < 1000);
        assert!(text.ends_with(&last));
        // Every line it kept is whole.
        assert!(
            text.lines()
                .all(|l| serde_json::from_str::<Value>(l).is_ok())
        );
        assert_eq!(tail(&dir.path().join("none.jsonl"), 1000), "");
    }
}
