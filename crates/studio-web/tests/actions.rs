//! Every action the pages offer sends studiod the request it should.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TOKEN: &str = "0123abcd";

/// Every request the stand-in studiod was sent.
static SEEN: Mutex<Vec<Request>> = Mutex::new(Vec::new());

fn ticket(id: &str, state: Value) -> Value {
    json!({"id": id, "project": "game", "title": "T", "goal": "", "hire": "dev",
           "state": state, "attempts": 0, "base_branch": "master", "created_at": 0})
}

fn reply(request: &Request) -> Value {
    match request {
        Request::Tickets => json!([
            ticket("GAME-1", json!("working")),
            ticket("GAME-5", json!("ready"))
        ]),
        Request::CreateTicket { .. } => ticket("GAME-11", json!("queued")),
        Request::CreateGoal { .. } => ticket("GAME-12", json!("queued")),
        Request::Asks => json!([{
            "id": "ASK-1", "ticket": "GAME-1", "hire": "dev", "kind": "permission",
            "tool": "Bash", "input": {"command": "cargo test"}, "reasons": [],
        }]),
        Request::Files { .. } | Request::RuleFiles => json!([]),
        Request::Land { .. } => json!({"branch": "goal/x", "sha": "abc"}),
        Request::Merge { .. } => {
            json!({"branch": "goal/x", "into": "master", "sha": "abcdef1234"})
        }
        Request::Decide { ask, .. } => json!({"ask": ask}),
        Request::Cancel { ticket } | Request::Resume { ticket } | Request::Retry { ticket } => {
            ticket_json(ticket)
        }
        Request::Pause { ticket } => json!({"ticket": ticket}),
        Request::SaveFile { name, .. } | Request::DeleteFile { name, .. } => json!({"name": name}),
        _ => Value::Null,
    }
}

fn ticket_json(id: &str) -> Value {
    ticket(id, json!("queued"))
}

fn fake_studiod(socket: &Path) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                SEEN.lock().unwrap().push(request.clone());
                let mut out = serde_json::to_string(&Response::ok(reply(&request))).unwrap();
                out.push('\n');
                write.write_all(out.as_bytes()).await.unwrap();
            });
        }
    });
}

struct Web {
    base: String,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Web {
    async fn start() -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        fake_studiod(&socket);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = AppConfig {
            socket,
            data_dir: dir.path().to_path_buf(),
            token: TOKEN.into(),
            base_url: base.clone(),
        };
        tokio::spawn(async move { topcoat::serve(listener, router(config)).await });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Web {
            base,
            http,
            _dir: dir,
        }
    }

    /// Posts a form and returns the reply's text.
    async fn post(&self, path: &str, form: &[(&str, &str)]) -> String {
        let reply = self
            .http
            .post(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", &self.base)
            .form(form)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 200, "{path}");
        reply.text().await.unwrap()
    }
}

/// The requests that change something, sent since the last call.
fn sent() -> Vec<Request> {
    SEEN.lock()
        .unwrap()
        .drain(..)
        .filter(|r| {
            !matches!(
                r,
                Request::Tickets
                    | Request::Asks
                    | Request::Files { .. }
                    | Request::RuleFiles
                    | Request::Status
            )
        })
        .collect()
}

fn decide(
    allow: bool,
    message: Option<&str>,
    answers: Option<BTreeMap<String, String>>,
) -> Request {
    Request::Decide {
        ask: "ASK-1".into(),
        allow,
        message: message.map(String::from),
        answers,
    }
}

#[tokio::test]
async fn every_action_sends_its_request() {
    let web = Web::start().await;

    web.post("/asks/ASK-1", &[("decision", "approve")]).await;
    assert_eq!(sent(), [decide(true, None, None)]);
    // A note can only go with a denial, and the owner is told so.
    let text = web
        .post(
            "/asks/ASK-1",
            &[("decision", "approve"), ("message", "use --release")],
        )
        .await;
    assert_eq!(sent(), [decide(true, None, None)]);
    assert!(text.contains("note was not sent"), "{text}");
    web.post(
        "/asks/ASK-1",
        &[("decision", "deny"), ("message", "not now")],
    )
    .await;
    assert_eq!(sent(), [decide(false, Some("not now"), None)]);
    web.post(
        "/asks/ASK-1",
        &[("decision", "approve"), ("Which one?", "A")],
    )
    .await;
    let answers = BTreeMap::from([("Which one?".to_string(), "A".to_string())]);
    assert_eq!(sent(), [decide(true, None, Some(answers))]);
    web.post("/asks/ASK-1", &[("decision", "always")]).await;
    let always = sent();
    assert_eq!(always[0], decide(true, None, None));
    assert!(
        matches!(&always[1..], [Request::SaveFile { kind: FileKind::Rules, text, .. }] if text.contains("cargo test")),
        "{always:?}"
    );

    let id = |s: &str| s.to_string();
    for (path, request) in [
        (
            "/tickets/GAME-5/land",
            Request::Land {
                ticket: id("GAME-5"),
            },
        ),
        (
            "/tickets/GAME-5/merge",
            Request::Merge {
                ticket: id("GAME-5"),
            },
        ),
        (
            "/tickets/GAME-1/pause",
            Request::Pause {
                ticket: id("GAME-1"),
            },
        ),
        (
            "/tickets/GAME-1/resume",
            Request::Resume {
                ticket: id("GAME-1"),
            },
        ),
        (
            "/tickets/GAME-1/cancel",
            Request::Cancel {
                ticket: id("GAME-1"),
            },
        ),
        (
            "/tickets/GAME-1/retry",
            Request::Retry {
                ticket: id("GAME-1"),
            },
        ),
    ] {
        web.post(path, &[]).await;
        assert_eq!(sent(), [request], "{path}");
    }

    let text = web
        .post(
            "/tickets",
            &[
                ("project", "game"),
                ("hire", ""),
                ("title", "Add a menu"),
                ("detail", "a\r\nb"),
            ],
        )
        .await;
    assert!(text.contains("GAME-11"), "{text}");
    assert_eq!(
        sent(),
        [Request::CreateTicket {
            project: "game".into(),
            title: "Add a menu".into(),
            goal: String::new(),
            hire: String::new(),
            base: None,
            detail: "a\nb".into(),
        }]
    );
    web.post(
        "/goals",
        &[("project", "game"), ("text", "Ship it"), ("detail", "")],
    )
    .await;
    assert_eq!(
        sent(),
        [Request::CreateGoal {
            project: "game".into(),
            text: "Ship it".into(),
            hire: None,
            detail: String::new(),
        }]
    );

    let hire = "---\nname: X\nrole: engineer\nruntime: claude-code\nmodel: sonnet\n---\nx\n";
    web.post("/hires/save", &[("name", "x"), ("text", hire)])
        .await;
    assert!(matches!(
        &sent()[..],
        [Request::SaveFile {
            kind: FileKind::Hires,
            ..
        }]
    ));
    web.post("/hires/x/delete?open=x", &[]).await;
    assert!(matches!(
        &sent()[..],
        [Request::DeleteFile {
            kind: FileKind::Hires,
            ..
        }]
    ));
    let rule = "---\nkind: guide\nlevel: warn\n---\nx\n";
    web.post("/rules/save", &[("name", "r"), ("text", rule)])
        .await;
    assert!(matches!(
        &sent()[..],
        [Request::SaveFile {
            kind: FileKind::Rules,
            ..
        }]
    ));
    web.post("/rules/r/delete?open=r", &[]).await;
    assert!(matches!(
        &sent()[..],
        [Request::DeleteFile {
            kind: FileKind::Rules,
            ..
        }]
    ));
}
