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

/// The notes that come before a reviewer's or a gate's critic's run in the
/// log, whose last words are a verdict and not the work's result.
const JUDGING: [&str; 2] = ["review by ", "gate "];

/// The longest a result may be.
const RESULT_MAX: usize = 140;

/// What the task `ticket` came to, from its run log: the first sentence of
/// the last words of the agent that did the work. None when the log has no
/// such words. The answer is kept for as long as the log keeps its size, and a
/// finished task's log no longer grows.
pub(crate) fn result_of(runs: &Path, ticket: &str) -> Option<String> {
    static SEEN: Mutex<BTreeMap<PathBuf, (u64, Option<String>)>> = Mutex::new(BTreeMap::new());
    let path = log_path(runs, ticket);
    let len = std::fs::metadata(&path).map_or(0, |m| m.len());
    let known = SEEN
        .lock()
        .ok()
        .and_then(|seen| seen.get(&path).cloned())
        .filter(|(size, _)| *size == len);
    if let Some((_, answer)) = known {
        return answer;
    }
    let answer = result_in(&tail(&path, REVIEW_TAIL));
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(path, (len, answer.clone()));
    }
    answer
}

/// The result a log's text gives: the first sentence of the last words of a
/// run that was not a judge's and ended well.
fn result_in(text: &str) -> Option<String> {
    let mut judging = false;
    let mut last_words = None;
    for raw in text.lines() {
        let mentions_note = raw.contains("\"note\"");
        if !mentions_note && !raw.contains("\"finished\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        if mentions_note {
            if let Some(note) = entry["note"].as_str() {
                judging |= JUDGING.iter().any(|start| note.starts_with(start));
            }
            continue;
        }
        let event = &entry["event"];
        if event["type"] != "finished" {
            continue;
        }
        if std::mem::take(&mut judging) {
            continue;
        }
        if event["outcome"]["outcome"] == "success"
            && let Some(words) = event["summary"].as_str().filter(|s| !s.trim().is_empty())
        {
            last_words = Some(words.to_string());
        }
    }
    let line = first_sentence(&last_words?);
    (!line.is_empty()).then_some(line)
}

/// The first sentence of `words`, at most `RESULT_MAX` characters, with the
/// markdown taken out. Headings, rules, code blocks and tables are not
/// sentences, so the first one of plain words after them is the one; if there
/// are none, the first heading is.
pub(crate) fn first_sentence(words: &str) -> String {
    let mut in_code = false;
    let mut heading = None;
    let mut paragraph: Vec<&str> = Vec::new();
    for line in words.lines().map(str::trim) {
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        let rule = line.len() >= 3 && line.chars().all(|c| matches!(c, '-' | '*' | '_' | '='));
        let table = line.starts_with('|');
        if in_code || rule || table {
            continue;
        }
        if let Some(title) = line.strip_prefix('#') {
            heading.get_or_insert(title.trim_start_matches('#').trim());
            continue;
        }
        if line.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        // Each item of a list is a line of its own.
        let item = ["- ", "* ", "+ "].iter().any(|m| line.starts_with(m));
        if item && !paragraph.is_empty() {
            break;
        }
        paragraph.push(line);
    }
    let text = if paragraph.is_empty() {
        plain(heading.unwrap_or_default())
    } else {
        plain(&paragraph.join(" "))
    };
    cut_at(sentence(&text), RESULT_MAX)
}

/// `text` without the markdown that marks a list item, a quote, a task box,
/// emphasis, code or a link, and on one line.
pub(crate) fn plain(text: &str) -> String {
    let mut text = text.trim();
    // A list item, a quote or a numbered item starts the line.
    loop {
        let before = text;
        text = text.trim_start_matches(['>', ' ']);
        for marker in ["- ", "* ", "+ "] {
            text = text.strip_prefix(marker).unwrap_or(text);
        }
        let digits = text.chars().take_while(char::is_ascii_digit).count();
        if digits > 0
            && let Some(rest) = text[digits..]
                .strip_prefix(". ")
                .or_else(|| text[digits..].strip_prefix(") "))
        {
            text = rest;
        }
        for marker in ["[ ] ", "[x] ", "[X] "] {
            text = text.strip_prefix(marker).unwrap_or(text);
        }
        if text == before {
            break;
        }
    }
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        // A link or an image keeps its words and loses its address.
        if let Some(after) = rest.strip_prefix("![").or_else(|| rest.strip_prefix('['))
            && let Some((label, tail)) = after.split_once("](")
            && let Some((_, tail)) = tail.split_once(')')
            && !label.contains(']')
        {
            plain.push_str(label);
            rest = tail;
            continue;
        }
        if rest.starts_with("__") {
            rest = &rest[2..];
        } else if matches!(c, '*' | '`' | '~') {
            rest = &rest[c.len_utf8()..];
        } else {
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` up to the end of its first sentence: a full stop, question mark or
/// exclamation mark that is followed by a space or ends the text. "e.g." and
/// "i.e." don't end one.
fn sentence(text: &str) -> &str {
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let end = at + c.len_utf8();
        let ends = chars.peek().is_none_or(|(_, next)| next.is_whitespace());
        let abbreviation = c == '.' && (text[..at].ends_with("e.g") || text[..at].ends_with("i.e"));
        if ends && !abbreviation {
            return &text[..end];
        }
    }
    text
}

/// `text` cut to at most `max` characters, with an ellipsis in the last if cut.
fn cut_at(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn a_result_is_the_first_sentence_without_markdown() {
        assert_eq!(
            first_sentence("Added the **cart drawer** to `shop.rs`. It opens from the header."),
            "Added the cart drawer to shop.rs."
        );
        assert_eq!(first_sentence("Done"), "Done");
        assert_eq!(first_sentence("Did it! Then more."), "Did it!");
        assert_eq!(first_sentence(""), "");
        assert_eq!(first_sentence("   \n\n"), "");
    }

    #[test]
    fn headings_lists_links_and_code_are_not_the_sentence() {
        let words = "## Summary\n\n```\ncargo test\n```\n\n---\n\n- [x] Fixed the [save bug](https://example.com/x), see e.g. the log.\n- Next";
        assert_eq!(
            first_sentence(words),
            "Fixed the save bug, see e.g. the log."
        );
        assert_eq!(first_sentence("> 1. Moved the file."), "Moved the file.");
        // A message of only a heading is that heading.
        assert_eq!(first_sentence("# Shipped"), "Shipped");
        // A version number is not the end of a sentence.
        assert_eq!(
            first_sentence("Bumped to v1.2 and tagged it."),
            "Bumped to v1.2 and tagged it."
        );
        assert_eq!(
            first_sentence("Used snake_case names."),
            "Used snake_case names."
        );
    }

    #[test]
    fn a_sentence_wraps_over_lines_and_is_cut_to_140_characters() {
        assert_eq!(
            first_sentence("Changed the menu\nso that it\nscrolls.\n\nMore."),
            "Changed the menu so that it scrolls."
        );
        let long = format!("{} end.", "word ".repeat(60));
        let line = first_sentence(&long);
        assert_eq!(line.chars().count(), RESULT_MAX);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn the_result_is_what_the_agent_said_and_not_what_a_reviewer_said() {
        let log = [
            r#"{"at":1,"event":{"type":"finished","outcome":{"outcome":"success"},"summary":"First try: broke it."}}"#,
            r#"{"at":2,"note":"checks passed"}"#,
            r#"{"at":3,"event":{"type":"finished","outcome":{"outcome":"success"},"summary":"Added the **cart**. More words."}}"#,
            r#"{"at":4,"note":"review by critic (fake-2)"}"#,
            r#"{"at":5,"event":{"type":"finished","outcome":{"outcome":"success"},"summary":"Looks right.\nVERDICT: PASS"}}"#,
            r#"{"at":6,"note":"gate art by critic (fake-2)"}"#,
            r#"{"at":7,"event":{"type":"finished","outcome":{"outcome":"success"},"summary":"The art is fine."}}"#,
        ]
        .join("\n");
        assert_eq!(result_in(&log).as_deref(), Some("Added the cart."));
        // A run cut short, or one that said nothing, doesn't replace the words.
        let cut = format!(
            "{log}\n{}\n{}",
            r#"{"at":8,"event":{"type":"finished","outcome":{"outcome":"budget"},"summary":"Stopped."}}"#,
            r#"{"at":9,"event":{"type":"finished","outcome":{"outcome":"success"},"summary":"  "}}"#
        );
        assert_eq!(result_in(&cut).as_deref(), Some("Added the cart."));
        assert_eq!(result_in(""), None);
        assert_eq!(result_in(r#"{"at":1,"note":"checks passed"}"#), None);
    }

    #[test]
    fn a_result_is_read_from_the_log_once_it_stops_growing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SHOP-1.jsonl");
        let done = |text: &str| {
            format!(
                "{}\n",
                json!({"at": 1, "event": {"type": "finished", "outcome": {"outcome": "success"}, "summary": text}})
            )
        };
        std::fs::write(&path, done("Added the cart.")).unwrap();
        assert_eq!(
            result_of(dir.path(), "SHOP-1").as_deref(),
            Some("Added the cart.")
        );
        // Sent back and done again: the log grew, so the answer is read again.
        std::fs::write(
            &path,
            done("Added the cart.") + &done("Added the cart and a total."),
        )
        .unwrap();
        assert_eq!(
            result_of(dir.path(), "SHOP-1").as_deref(),
            Some("Added the cart and a total.")
        );
        assert_eq!(result_of(dir.path(), "SHOP-2"), None);
    }

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
