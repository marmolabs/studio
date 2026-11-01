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
/// Tests share `SEEN`, so they take turns.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        Request::Status => json!({
            "team": [{"id": "dev", "role": "engineer", "model": "sonnet"},
                     {"id": "approver", "role": "approver", "model": "haiku"}],
        }),
        Request::AddProject {
            path, name, branch, ..
        } => json!({
            "name": name.clone().unwrap_or_else(|| "repo".into()), "path": path,
            "branch": branch.clone().unwrap_or_else(|| "main".into()),
            "checks": if path.ends_with("fresh") { json!(format!("{path}/.studio.toml")) } else { Value::Null },
        }),
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
    let _turn = TURN.lock().await;
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

#[tokio::test]
async fn the_office_controls_send_their_requests() {
    let _turn = TURN.lock().await;
    let web = Web::start().await;

    let text = web.post("/office/close", &[]).await;
    assert!(text.contains("The office is closed"), "{text}");
    assert_eq!(sent(), [Request::SetOpen { open: false }]);
    let text = web.post("/office/open", &[]).await;
    assert!(text.contains("The office is open"), "{text}");
    assert_eq!(sent(), [Request::SetOpen { open: true }]);

    let text = web.post("/office/desks", &[("max_runs", "4")]).await;
    assert!(text.contains("4 agents can work at once"), "{text}");
    assert_eq!(sent(), [Request::SetMaxRuns { max_runs: 4 }]);
    // Out of range or not a number: told so, nothing sent.
    for bad in ["0", "10", "many", ""] {
        let text = web.post("/office/desks", &[("max_runs", bad)]).await;
        assert!(text.contains("Pick between 1 and 9 desks"), "{bad}: {text}");
    }
    assert!(sent().is_empty());

    let text = web
        .post(
            "/office/limits",
            &[("daily_usd", "$12.50"), ("ticket_usd", "")],
        )
        .await;
    assert!(text.contains("The daily limit is $12.50"), "{text}");
    assert_eq!(
        sent(),
        [Request::SetLimits {
            daily_usd: Some(12.5),
            ticket_usd: None
        }]
    );
    web.post("/office/limits", &[("daily_usd", ""), ("ticket_usd", "3")])
        .await;
    assert_eq!(
        sent(),
        [Request::SetLimits {
            daily_usd: None,
            ticket_usd: Some(3.0)
        }]
    );
    for form in [
        [("daily_usd", "0"), ("ticket_usd", "")],
        [("daily_usd", "lots"), ("ticket_usd", "")],
        [("daily_usd", ""), ("ticket_usd", "")],
    ] {
        web.post("/office/limits", &form).await;
    }
    assert!(sent().is_empty());
}

#[tokio::test]
async fn the_office_controls_refuse_posts_from_other_sites() {
    let _turn = TURN.lock().await;
    let web = Web::start().await;
    for path in [
        "/office/close",
        "/office/open",
        "/office/desks",
        "/office/limits",
        "/office/projects",
        "/office/hires",
    ] {
        let reply = web
            .http
            .post(format!("{}{path}", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", "https://evil.example")
            .form(&[("max_runs", "4"), ("daily_usd", "1")])
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 403, "{path}");
        let no_cookie = web
            .http
            .post(format!("{}{path}", web.base))
            .header("origin", &web.base)
            .send()
            .await
            .unwrap();
        assert_eq!(no_cookie.status(), 401, "{path}");
    }
    assert!(sent().is_empty());
}

#[tokio::test]
async fn adding_a_project_from_the_office_sends_the_request() {
    let _turn = TURN.lock().await;
    let web = Web::start().await;

    // The first submit only checks: studiod is asked what would be added and
    // nothing is added. Empty name and branch go as absent.
    let text = web
        .post(
            "/office/projects",
            &[("path", "/code/fresh"), ("name", ""), ("branch", "  ")],
        )
        .await;
    assert_eq!(
        sent(),
        [Request::AddProject {
            path: "/code/fresh".into(),
            name: None,
            branch: None,
            daily_usd: None,
            pack: None,
            check: true,
        }]
    );
    // What it found goes to the panel to be confirmed: the name, the full
    // folder, the branch and the checks.
    assert!(text.contains("datastar-patch-signals"), "{text}");
    for found in [
        r#""name":"repo""#,
        r#""path":"/code/fresh""#,
        r#""branch":"main""#,
        "/code/fresh/.studio.toml",
    ] {
        assert!(text.contains(found), "{found}: {text}");
    }
    assert!(!text.contains("Added"), "{text}");

    // Add it sends the same form again, and only now is anything added.
    let text = web
        .post(
            "/office/projects?add=yes",
            &[("path", "/code/fresh"), ("name", ""), ("branch", "  ")],
        )
        .await;
    assert!(
        text.contains("Added repo. Its checks: /code/fresh/.studio.toml"),
        "{text}"
    );
    assert_eq!(
        sent(),
        [Request::AddProject {
            path: "/code/fresh".into(),
            name: None,
            branch: None,
            daily_usd: None,
            pack: None,
            check: false,
        }]
    );

    // Typed fields go as typed, in both steps; a project that had its own
    // checks says only that it was added.
    let typed = [
        ("path", " /code/shop "),
        ("name", "shop"),
        ("branch", "develop"),
    ];
    let request = |check| Request::AddProject {
        path: "/code/shop".into(),
        name: Some("shop".into()),
        branch: Some("develop".into()),
        daily_usd: None,
        pack: None,
        check,
    };
    let text = web.post("/office/projects", &typed).await;
    assert!(text.contains("It already has its own checks."), "{text}");
    assert_eq!(sent(), [request(true)]);
    let text = web.post("/office/projects?add=yes", &typed).await;
    assert!(text.contains("Added shop."), "{text}");
    assert!(!text.contains("Its checks"), "{text}");
    assert_eq!(sent(), [request(false)]);

    // No folder: told so, nothing sent.
    let text = web
        .post("/office/projects", &[("path", " "), ("name", "x")])
        .await;
    assert!(
        text.contains("Enter the folder of a git repository"),
        "{text}"
    );
    assert!(sent().is_empty());
    let text = web
        .post("/office/projects?add=yes", &[("path", " "), ("name", "x")])
        .await;
    assert!(
        text.contains("Enter the folder of a git repository"),
        "{text}"
    );
    assert!(sent().is_empty());
}

#[tokio::test]
async fn hiring_from_the_office_saves_the_hires_file() {
    let _turn = TURN.lock().await;
    let web = Web::start().await;

    let text = web
        .post(
            "/office/hires",
            &[
                ("name", "quick-dev"),
                ("role", "engineer"),
                ("model", "opus"),
                ("about", "Small steps, tests first."),
            ],
        )
        .await;
    assert!(text.contains("Hired quick-dev as engineer."), "{text}");
    assert_eq!(
        sent(),
        [Request::SaveFile {
            kind: FileKind::Hires,
            name: "quick-dev".into(),
            text: "---\nname: quick-dev\nrole: engineer\nruntime: claude-code\nmodel: opus\n---\nSmall steps, tests first.\n".into(),
        }]
    );

    // Without a line about how they work, the file still has a text.
    web.post(
        "/office/hires",
        &[("name", "scribe"), ("role", "writer"), ("model", "haiku")],
    )
    .await;
    assert!(matches!(&sent()[..], [Request::SaveFile { text, .. }]
            if text.starts_with("---\nname: scribe\nrole: writer\nruntime: claude-code\nmodel: haiku\n---\nYou are a careful writer.")),);

    // A name already on the team, or not an id, a role or a model that isn't
    // offered: told so, nothing sent.
    let taken = web
        .post(
            "/office/hires",
            &[("name", "dev"), ("role", "engineer"), ("model", "sonnet")],
        )
        .await;
    assert!(taken.contains("dev is already on the team"), "{taken}");
    for (name, role, model, said) in [
        ("Quick Dev", "engineer", "sonnet", "lowercase letters"),
        ("", "engineer", "sonnet", "lowercase letters"),
        ("../evil", "engineer", "sonnet", "lowercase letters"),
        ("ok", "janitor", "sonnet", "Pick a role"),
        ("ok", "engineer", "gpt", "Pick a model"),
    ] {
        let text = web
            .post(
                "/office/hires",
                &[("name", name), ("role", role), ("model", model)],
            )
            .await;
        assert!(text.contains(said), "{name}: {text}");
    }
    assert!(sent().is_empty());
}
