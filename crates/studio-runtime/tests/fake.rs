#![allow(clippy::unwrap_used, clippy::panic)]

use serde_json::json;
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
