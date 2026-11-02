//! The Claude adapter against recorded sessions. `fixtures/*.jsonl` are real
//! `claude -p` stream-json output (trimmed and anonymised).
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{ClaudeCode, Parsed, parse_line};
use crate::{Control, FailureKind, Outcome, RunEvent, RunSpec, Runtime};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn events_of(name: &str) -> Vec<RunEvent> {
    fs::read_to_string(fixture(name))
        .unwrap()
        .lines()
        .flat_map(parse_line)
        .map(|parsed| match parsed {
            Parsed::Event(event) => event,
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn parses_an_approved_write() {
    let events = events_of("claude-allow.jsonl");
    assert!(matches!(&events[0], RunEvent::Started { model: Some(m), .. } if m.contains("haiku")));
    assert!(events.iter().any(
        |e| matches!(e, RunEvent::ToolCall { name, input, .. } if name == "Write" && input["content"] == "hi\n")
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::Permission { tool, .. } if tool == "Write"))
    );
    assert!(events.iter().any(
        |e| matches!(e, RunEvent::ToolResult { is_error: false, output, .. } if output.contains("created"))
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::RateLimit { status, .. } if status == "allowed"))
    );
    match events.last().unwrap() {
        RunEvent::Finished {
            outcome,
            cost,
            summary,
            session,
        } => {
            assert_eq!(*outcome, Outcome::Success);
            assert!(cost.reported && cost.cumulative && cost.usd > 0.0);
            assert_eq!(summary.as_deref(), Some("done"));
            assert!(session.is_some());
        }
        other => panic!("last event was {other:?}"),
    }
}

#[test]
fn parses_a_question() {
    let events = events_of("claude-question.jsonl");
    let questions = events
        .iter()
        .find_map(|e| match e {
            RunEvent::Question { questions, .. } => Some(questions.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].options.len(), 2);
}

#[test]
fn parses_a_failed_permission_as_a_tool_error() {
    let events = events_of("claude-stream-closed.jsonl");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::ToolResult { is_error: true, .. }))
    );
}

/// A stand-in for `claude`: replays a fixture, and after each permission
/// request waits for the studio's control response and logs it.
const FAKE: &str = r#"#!/bin/sh
fixture="$1"; log="$2"
while IFS= read -r out <&4; do
  printf '%s\n' "$out"
  case "$out" in
    *'"control_request"'*can_use_tool*)
      while IFS= read -r reply; do
        printf '%s\n' "$reply" >> "$log"
        case "$reply" in *control_response*) break;; esac
      done;;
  esac
done 4< "$fixture"
"#;

fn fake_claude(dir: &Path, fixture_name: &str) -> (ClaudeCode, PathBuf) {
    let script = dir.join("fake-claude.sh");
    fs::write(&script, FAKE).unwrap();
    let log = dir.join("stdin.log");
    let runtime = ClaudeCode::with_program(
        "sh",
        vec![
            script.to_string_lossy().into_owned(),
            fixture(fixture_name).to_string_lossy().into_owned(),
            log.to_string_lossy().into_owned(),
        ],
    );
    (runtime, log)
}

fn logged(log: &Path) -> Vec<Value> {
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn approves_a_permission_over_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, log) = fake_claude(dir.path(), "claude-allow.jsonl");
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "Create hello.txt".into(),
            ..RunSpec::default()
        })
        .unwrap();

    let mut seen = Vec::new();
    while let Some(event) = run.next().await {
        if let RunEvent::Permission { request_id, .. } = &event {
            run.allow(request_id).await.unwrap();
        }
        seen.push(event);
    }
    assert!(matches!(
        seen.last(),
        Some(RunEvent::Finished {
            outcome: Outcome::Success,
            ..
        })
    ));

    let sent = logged(&log);
    assert_eq!(sent[0]["request"]["subtype"], "initialize");
    assert_eq!(sent[1]["message"]["content"], "Create hello.txt");
    let reply = &sent.last().unwrap()["response"];
    assert_eq!(reply["response"]["behavior"], "allow");
    assert_eq!(reply["response"]["updatedInput"]["content"], "hi\n");
}

#[tokio::test]
async fn says_something_to_the_agent_over_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, log) = fake_claude(dir.path(), "claude-allow.jsonl");
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "Create hello.txt".into(),
            ..RunSpec::default()
        })
        .unwrap();
    while let Some(event) = run.next().await {
        if let RunEvent::Permission { request_id, .. } = &event {
            run.say("Make it say hello.").await.unwrap();
            run.allow(request_id).await.unwrap();
        }
    }
    let sent = logged(&log);
    // The first prompt, then the message, each a user message, then the answer.
    assert_eq!(sent[1]["type"], "user");
    assert_eq!(sent[1]["message"]["content"], "Create hello.txt");
    assert_eq!(sent[2]["type"], "user");
    assert_eq!(sent[2]["message"]["role"], "user");
    assert_eq!(sent[2]["message"]["content"], "Make it say hello.");
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[3]["response"]["response"]["behavior"], "allow");
}

#[tokio::test]
async fn answers_a_question_over_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, log) = fake_claude(dir.path(), "claude-question.jsonl");
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "Ask me".into(),
            ..RunSpec::default()
        })
        .unwrap();
    while let Some(event) = run.next().await {
        if let RunEvent::Question {
            request_id,
            questions,
        } = &event
        {
            let answers = BTreeMap::from([(
                questions[0].question.clone(),
                questions[0].options[1].clone(),
            )]);
            run.send(Control::Answer {
                request_id: request_id.clone(),
                answers,
            })
            .await
            .unwrap();
        }
    }
    let sent = logged(&log);
    let reply = &sent.last().unwrap()["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    let answers = reply["updatedInput"]["answers"].as_object().unwrap();
    assert_eq!(answers.len(), 1);
    assert!(reply["updatedInput"]["questions"].is_array());
}

#[tokio::test]
async fn a_crash_before_the_result_is_classified() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = ClaudeCode::with_program(
        "sh",
        vec![
            "-c".into(),
            "echo \"You've hit your session limit\" >&2; exit 1".into(),
            "fake".into(),
        ],
    );
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "hi".into(),
            ..RunSpec::default()
        })
        .unwrap();
    let mut last = None;
    while let Some(event) = run.next().await {
        last = Some(event);
    }
    match last {
        Some(RunEvent::Finished {
            outcome: Outcome::Failed { kind, message },
            ..
        }) => {
            assert_eq!(kind, FailureKind::Quota);
            assert!(message.contains("session limit"));
        }
        other => panic!("expected a classified failure, got {other:?}"),
    }
}

#[test]
fn reports_a_missing_binary() {
    let runtime = ClaudeCode::with_program("/nonexistent/claude", Vec::new());
    let report = runtime.check_env();
    assert!(!report.installed);
    assert!(report.problem.is_some());
}

#[tokio::test]
async fn runs_keep_long_commands_in_the_foreground() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("env.txt");
    let runtime = ClaudeCode::with_program(
        "sh",
        vec![
            "-c".into(),
            r#"printf '%s %s %s\n' "$CLAUDE_CODE_DISABLE_BACKGROUND_TASKS" "$BASH_MAX_TIMEOUT_MS" "$SHARED" > "$0""#
                .into(),
            seen.to_string_lossy().into_owned(),
        ],
    );
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "Build it".into(),
            // The project's own environment can't undo the studio's settings.
            env: vec![
                ("SHARED".into(), "cache".into()),
                ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".into(), "0".into()),
            ],
            ..RunSpec::default()
        })
        .unwrap();
    while run.next().await.is_some() {}
    assert_eq!(fs::read_to_string(&seen).unwrap(), "1 1800000 cache\n");
}

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s1","model":"haiku"}"#;

fn result(text: &str) -> String {
    format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"result":"{text}","session_id":"s1","total_cost_usd":0.01,"usage":{{}}}}"#
    )
}

/// A stand-in for `claude` that runs `body` after reading the initialize
/// request and the first prompt, and exits when the studio closes its stdin.
fn scripted_claude(dir: &Path, body: &str) -> ClaudeCode {
    let script = dir.join("scripted-claude.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nread init; read prompt\n{body}\ncat > /dev/null\n"),
    )
    .unwrap();
    ClaudeCode::with_program("sh", vec![script.to_string_lossy().into_owned()])
}

/// Every event of a run that is told something as soon as it has started.
async fn events_when_told(runtime: &ClaudeCode, dir: &Path, tell: bool) -> Vec<RunEvent> {
    let mut run = runtime
        .start(RunSpec {
            cwd: dir.to_path_buf(),
            prompt: "Work".into(),
            ..RunSpec::default()
        })
        .unwrap();
    let mut seen = Vec::new();
    while let Some(event) = run.next().await {
        if tell && matches!(event, RunEvent::Started { .. }) {
            run.say("Use three colours.").await.unwrap();
        }
        seen.push(event);
    }
    seen
}

#[tokio::test]
async fn a_message_the_turn_took_in_gives_one_result() {
    // What the CLI does with a message that reaches it in the middle of a
    // turn that uses tools: it answers it in that turn, with one result.
    let dir = tempfile::tempdir().unwrap();
    let body = format!("echo '{INIT}'\nread said\necho '{}'", result("one"));
    let runtime = scripted_claude(dir.path(), &body);
    let seen = events_when_told(&runtime, dir.path(), true).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(matches!(
        &seen[1],
        RunEvent::Finished { summary: Some(s), .. } if s == "one"
    ));
}

#[tokio::test]
async fn a_message_that_came_too_late_for_the_turn_is_answered_in_a_turn_of_its_own() {
    // What the CLI does with a message that reaches it as a turn ends: the
    // result of the turn, then a second turn. The run goes on to the second
    // result, and the first is not reported as the end.
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Three colours."}]}}"#;
    let body = format!(
        "echo '{INIT}'\nread said\necho '{}'\necho '{INIT}'\necho '{text}'\necho '{}'",
        result("first"),
        result("second")
    );
    let runtime = scripted_claude(dir.path(), &body);
    let seen = events_when_told(&runtime, dir.path(), true).await;
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(matches!(seen[0], RunEvent::Started { .. }));
    assert_eq!(
        seen[1],
        RunEvent::Text {
            text: "Three colours.".into()
        }
    );
    assert!(matches!(
        &seen[2],
        RunEvent::Finished { summary: Some(s), .. } if s == "second"
    ));
}

#[tokio::test]
async fn a_run_nobody_spoke_to_reports_its_result_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!("echo '{INIT}'\necho '{}'", result("only"));
    let runtime = scripted_claude(dir.path(), &body);
    let seen = events_when_told(&runtime, dir.path(), false).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(matches!(
        &seen[1],
        RunEvent::Finished { summary: Some(s), .. } if s == "only"
    ));
}

/// The real CLI, to see how it takes a message in the middle of its work.
/// Costs a cent or two: `cargo test -p studio-runtime -- --ignored`.
#[tokio::test]
#[ignore = "uses the real claude CLI and its model"]
async fn the_real_cli_takes_a_message_in_the_middle_of_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = ClaudeCode::default()
        .start(RunSpec {
            cwd: dir.path().to_path_buf(),
            prompt: "Run these bash commands one at a time, each as its own tool call: \
                     `sleep 5; echo one`, then `sleep 5; echo two`. Then reply with just: finished."
                .into(),
            model: Some("haiku".into()),
            max_budget_usd: Some(0.15),
            ..RunSpec::default()
        })
        .unwrap();
    let (mut told, mut finished, mut words) = (false, 0, String::new());
    while let Some(event) = run.next().await {
        match event {
            RunEvent::Permission { request_id, .. } => run.allow(&request_id).await.unwrap(),
            RunEvent::ToolCall { .. } if !told => {
                told = true;
                run.say("Change of plan: also run `echo BANANA` at the end, and say BANANA in your reply.")
                    .await
                    .unwrap();
            }
            RunEvent::Text { text } => words.push_str(&text),
            RunEvent::Finished {
                outcome, summary, ..
            } => {
                assert_eq!(outcome, Outcome::Success);
                words.push_str(&summary.unwrap_or_default());
                finished += 1;
            }
            _ => {}
        }
    }
    assert_eq!(finished, 1);
    assert!(words.contains("BANANA"), "{words}");
}
