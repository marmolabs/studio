#![allow(clippy::unwrap_used, clippy::panic)]

use serde_json::json;
use std::time::Duration;
use studio_runtime::{Control, Cost, FakeRuntime, Outcome, RunEvent, RunSpec, Runtime};

#[tokio::test]
async fn fake_waits_for_decisions_and_records_them() {
    let runtime = FakeRuntime::new(vec![
        RunEvent::Started {
            session: "s1".into(),
            model: None,
        },
        RunEvent::Permission {
            request_id: "p1".into(),
            tool: "Write".into(),
            input: json!({ "file_path": "a.txt" }),
        },
        RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost::default(),
            summary: Some("done".into()),
        },
    ]);
    let mut run = runtime
        .start(RunSpec {
            prompt: "go".into(),
            ..RunSpec::default()
        })
        .unwrap();
    let mut count = 0;
    while let Some(event) = run.next().await {
        if let RunEvent::Permission { request_id, .. } = &event {
            run.deny(request_id, "not now").await.unwrap();
        }
        count += 1;
    }
    assert_eq!(count, 3);
    assert_eq!(
        runtime.received(),
        vec![Control::Deny {
            request_id: "p1".into(),
            message: "not now".into()
        }]
    );
    assert_eq!(runtime.specs()[0].prompt, "go");
}

fn finish() -> RunEvent {
    RunEvent::Finished {
        outcome: Outcome::Success,
        session: None,
        cost: Cost::default(),
        summary: None,
    }
}

fn said(text: &str) -> Control {
    Control::Say { text: text.into() }
}

#[tokio::test]
async fn fake_records_what_it_is_told() {
    let runtime = FakeRuntime::new(vec![
        RunEvent::Started {
            session: "s1".into(),
            model: None,
        },
        RunEvent::Permission {
            request_id: "p1".into(),
            tool: "Write".into(),
            input: json!({ "file_path": "a.txt" }),
        },
        finish(),
    ]);
    let mut run = runtime.start(RunSpec::default()).unwrap();
    assert!(runtime.told().is_empty());
    run.say("Use three colours.").await.unwrap();
    let mut count = 0;
    while let Some(event) = run.next().await {
        if let RunEvent::Permission { request_id, .. } = &event {
            // A message while the run waits for a decision is not the decision.
            run.say("And keep it short.").await.unwrap();
            run.allow(request_id).await.unwrap();
        }
        count += 1;
    }
    assert_eq!(count, 3);
    assert_eq!(runtime.told(), ["Use three colours.", "And keep it short."]);
    assert_eq!(
        runtime.received(),
        vec![
            said("Use three colours."),
            said("And keep it short."),
            Control::Allow {
                request_id: "p1".into()
            }
        ]
    );
}

#[tokio::test]
async fn a_message_does_not_end_a_held_run_as_an_interrupt_does() {
    let (runtime, release) = FakeRuntime::new(vec![finish()]).held();
    let mut run = runtime.start(RunSpec::default()).unwrap();
    run.say("Hello.").await.unwrap();
    let early = tokio::time::timeout(Duration::from_millis(100), run.next()).await;
    assert!(early.is_err(), "the message ended the held run");
    release.notify_one();
    assert!(matches!(run.next().await, Some(RunEvent::Finished { .. })));
    assert_eq!(runtime.told(), ["Hello."]);

    let (runtime, _release) = FakeRuntime::new(vec![finish()]).held();
    let mut run = runtime.start(RunSpec::default()).unwrap();
    run.interrupt().await.unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(10), run.next()).await;
    assert!(matches!(ended, Ok(Some(RunEvent::Finished { .. }))));
    assert_eq!(runtime.received(), vec![Control::Interrupt]);
    assert!(runtime.told().is_empty());
}

#[tokio::test]
async fn fake_always_finishes() {
    let runtime = FakeRuntime::new(Vec::new());
    let mut run = runtime.start(RunSpec::default()).unwrap();
    assert!(matches!(
        run.next().await,
        Some(RunEvent::Finished {
            outcome: Outcome::Success,
            ..
        })
    ));
    assert!(run.next().await.is_none());
}

#[tokio::test]
async fn fake_sequence_plays_one_script_per_run() {
    let finish = |text: &str| RunEvent::Finished {
        outcome: Outcome::Success,
        session: None,
        cost: Cost::default(),
        summary: Some(text.into()),
    };
    let runtime = FakeRuntime::sequence(vec![vec![finish("one")], vec![finish("two")]]);
    let mut seen = Vec::new();
    for _ in 0..3 {
        let mut run = runtime.start(RunSpec::default()).unwrap();
        if let Some(RunEvent::Finished { summary, .. }) = run.next().await {
            seen.push(summary.unwrap());
        }
    }
    assert_eq!(seen, ["one", "two", "two"]);
}
