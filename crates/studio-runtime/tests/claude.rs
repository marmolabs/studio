//! The Claude adapter against recorded sessions. `fixtures/*.jsonl` are real
//! `claude -p` stream-json output (trimmed and anonymised).
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use studio_runtime::{
    ClaudeCode, Control, FailureKind, Outcome, Parsed, RunEvent, RunSpec, Runtime, parse_line,
};

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
