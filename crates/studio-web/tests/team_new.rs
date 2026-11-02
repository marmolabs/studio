//! A new project and the studio's settings against a stand-in studiod: what
//! the form says and posts, what the card of a setup shows in each of its
//! states and what its buttons post, and what the settings page holds.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use studio_web::{AppConfig, router};
use studiod::api::{NewLead, Request, Response, SetupChoice};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{named, text_of};

const TOKEN: &str = "0123abcd";

fn command(program: &str, tried: Value) -> Value {
    json!({"command": [format!("./scripts/{program}")], "why": "", "tried": tried})
}

fn passed(summary: &str) -> Value {
    json!({"passed": true, "seconds": 38, "summary": summary, "output_tail": ""})
}

/// What the lead proposed, with each check tried as `lint` says.
fn proposal(lint: Value) -> Value {
    json!({
        "summary": "A small 2D game with tests.",
        "verify": [command("test.sh", passed("112 pass")), command("lint.sh", lint)],
        "capture": command("capture.sh", passed("4 pictures")),
        "release": [["./scripts/package.sh"], ["./scripts/upload.sh"]],
        "release_source": "your CI",
        "protected": [{"glob": "saves/**", "why": "Saves"}, {"glob": ".env*", "why": "Secrets"}],
        "daily_usd": 10.0,
        "first_goal": {"text": "Fix the three crashes", "version": "0.9", "why": "They block play."},
    })
}

/// A studio of five projects, one for each state of a setup, all led by Mira
/// but the shop, which Nora leads.
fn office() -> Value {
    let names = ["reading", "checking", "ready", "accepted", "failed"];
    let failed_lint =
        json!({"passed": false, "seconds": 4, "summary": "failed", "output_tail": ""});
    let setups = json!({
        "reading": {"project": "reading", "lead": "mira", "state": "reading"},
        "checking": {"project": "checking", "lead": "mira", "state": "checking",
            "proposal": proposal(Value::Null)},
        "ready": {"project": "ready", "lead": "mira", "state": "ready",
            "proposal": proposal(failed_lint)},
        "accepted": {"project": "accepted", "lead": "mira", "state": "accepted",
            "proposal": proposal(passed("clean"))},
        "failed": {"project": "failed", "lead": "mira", "state": "failed",
            "error": "The repository has no commits yet."},
    });
    json!({
        "status": {
            "projects": names, "hires": ["nora", "mira", "dev"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "mira", "name": "Mira", "role": "lead", "model": "opus"},
                {"id": "dev", "role": "engineer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.5, "max_runs": 3, "rules": 2,
        },
        "tickets": [], "asks": [], "events": [], "spend": {}, "merged": [], "sizes": {},
        "today": {}, "judges": {},
        "projects": names.iter().map(|name| json!({
            "name": name, "lead": "mira", "path": format!("/games/{name}"),
        })).collect::<Vec<_>>(),
        "setups": setups,
        "now": 1_700_000_000_000_u64,
    })
}

fn answer(request: &Request) -> Response {
    match request {
        Request::Office => Response::ok(office()),
        // The lead posts the card of a setup once it is ready, so the
        // conversations of the ready and the accepted ones have it; one that is
        // still being set up, or failed, has none.
        Request::Conversation { project, .. } => {
            let said = |card: Value| {
                json!([{
                    "id": 1, "at": 1_699_999_000_000_u64,
                    "from": {"who": "lead", "hire": "mira"},
                    "text": "Here is how I'd run it.", "card": card,
                }])
            };
            let messages = match project.as_str() {
                "ready" | "accepted" => said(json!({"type": "setup", "project": project})),
                _ => json!([]),
            };
            Response::ok(json!({"messages": messages, "thinking": false}))
        }
        Request::Budget => Response::ok(json!({"limits": {"ticket_usd": 2.5}})),
        Request::SetupProject { path, name, .. } => Response::ok(json!({
            "project": name.clone().unwrap_or_else(|| path.rsplit('/').next().unwrap_or("x").into()),
            "lead": "mira", "state": "reading",
        })),
        Request::AcceptSetup { project, .. } => {
            Response::ok(json!({"project": project, "state": "accepted"}))
        }
        Request::CreateGoal { .. } => Response::err("not here"),
        _ => Response::ok(Value::Null),
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply. It
/// keeps every request it was sent.
fn fake_studiod(socket: &Path, seen: Arc<Mutex<Vec<Request>>>) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let mut out = serde_json::to_string(&answer(&request)).unwrap();
                seen.lock().unwrap().push(request);
                out.push('\n');
                write.write_all(out.as_bytes()).await.unwrap();
            });
        }
    });
}

struct Web {
    base: String,
    http: reqwest::Client,
    seen: Arc<Mutex<Vec<Request>>>,
    _dir: tempfile::TempDir,
}

impl Web {
    async fn start() -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        fake_studiod(&socket, Arc::clone(&seen));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = AppConfig {
            socket,
            data_dir: dir.path().to_path_buf(),
            token: TOKEN.into(),
            base_url: base.clone(),
        };
        tokio::spawn(async move { topcoat::serve(listener, router(config)).await });
        Web {
            base,
            http: reqwest::Client::new(),
            seen,
            _dir: dir,
        }
    }

    async fn page(&self, path: &str) -> String {
        let reply = self
            .http
            .get(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 200, "{path}");
        reply.text().await.unwrap()
    }

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

    /// What studiod was sent besides a look at the office, since the last
    /// call.
    fn sent(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .drain(..)
            .filter(|request| {
                !matches!(
                    request,
                    Request::Office | Request::Budget | Request::Conversation { .. }
                )
            })
            .collect()
    }
}

/// The text of the card of how the project runs on a conversation's page.
async fn card(web: &Web, project: &str) -> String {
    let html = web.page(&format!("/leads/{project}")).await;
    let at = html
        .find(r#"class="setup "#)
        .expect("the card is on the page");
    html[at..].to_string()
}

/// The first button whose words are `label`, if the page has one.
fn button<'a>(html: &'a str, label: &str) -> Option<probe::Tag<'a>> {
    named(html, "button")
        .into_iter()
        .find(|button| button.text() == label)
}

#[tokio::test]
async fn the_form_asks_for_a_folder_and_a_lead() {
    let web = Web::start().await;
    let html = web.page("/new").await;
    let form = named(&html, "form")
        .into_iter()
        .find(|form| form.attr_has("data-on:submit", "/new"))
        .expect("a form that posts to /new");
    let inside = form.inner_html();
    let field = |name: &str| {
        named(inside, "input")
            .into_iter()
            .chain(named(inside, "select"))
            .find(|input| input.attr_is("name", name))
    };
    assert!(field("path").is_some());
    // The studio suggests the first name nobody on the team has taken.
    let name = field("new_lead_name").expect("a name for a new lead");
    assert_eq!(name.attr("value").as_deref(), Some("Sam"));
    assert!(field("new_lead_model").is_some_and(|m| m.attr_is("value", "opus")));
    // The leads it already has can lead the project too.
    assert!(field("lead").is_some());
    let text = text_of(inside);
    assert!(text.contains("Mira"), "{text}");
    assert!(text.contains("Nora"), "{text}");
    assert!(button(inside, "Hire and set up").is_some());
}

#[tokio::test]
async fn sending_the_form_sets_the_project_up_and_goes_to_its_conversation() {
    let web = Web::start().await;
    let reply = web
        .post(
            "/new",
            &[
                ("path", " ~/games/garden "),
                ("new_lead_name", "Sam"),
                ("new_lead_model", "opus"),
            ],
        )
        .await;
    assert!(reply.contains("_go"), "{reply}");
    assert!(reply.contains("/leads/garden"), "{reply}");
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "~/games/garden".into(),
            name: None,
            lead: None,
            new_lead: Some(NewLead {
                name: "Sam".into(),
                model: "opus".into()
            }),
        }]
    );
    // An existing lead may lead it; if a new name came too, the new hire wins.
    web.post("/new", &[("path", "~/games/garden"), ("lead", "nora")])
        .await;
    web.post(
        "/new",
        &[
            ("path", "~/games/garden"),
            ("lead", "nora"),
            ("new_lead_name", "Sam"),
        ],
    )
    .await;
    let sent = web.sent();
    assert!(
        matches!(&sent[0], Request::SetupProject { lead: Some(lead), new_lead: None, .. } if lead == "nora")
    );
    assert!(matches!(
        &sent[1],
        Request::SetupProject {
            lead: None,
            new_lead: Some(_),
            ..
        }
    ));
    // Without a folder or a lead there is nothing to set up, and it is said.
    let nothing = web.post("/new", &[("path", ""), ("lead", "nora")]).await;
    assert!(nothing.contains("folder"), "{nothing}");
    let nobody = web.post("/new", &[("path", "~/games/garden")]).await;
    assert!(nobody.contains("lead"), "{nobody}");
    assert!(web.sent().is_empty());
}

#[tokio::test]
async fn the_card_says_what_the_lead_is_doing_in_each_state() {
    let web = Web::start().await;
    let reading = text_of(&card(&web, "reading").await);
    assert!(reading.contains("Mira is reading reading…"), "{reading}");

    let checking = card(&web, "checking").await;
    let text = text_of(&checking);
    assert!(text.contains("Trying the checks…"), "{text}");
    assert!(text.contains("test.sh ✓ 112 pass"), "{text}");
    assert!(text.contains("lint.sh trying…"), "{text}");
    assert!(button(&checking, "Use these").is_none());

    let ready = card(&web, "ready").await;
    let text = text_of(&ready);
    assert!(text.contains("How ready runs"), "{text}");
    assert!(text.contains("lint.sh ✗ failed"), "{text}");
    assert!(text.contains("capture.sh ✓ 4 pictures"), "{text}");
    assert!(text.contains("from your CI"), "{text}");
    assert!(text.contains("saves/** · .env*"), "{text}");
    assert!(text.contains("$10 a day"), "{text}");
    // The file it would write has what passed and nothing that failed.
    assert!(
        text.contains(r#"verify = [["./scripts/test.sh"]]"#),
        "{text}"
    );
    for label in ["Use these", "Change something", "See the file"] {
        assert!(button(&ready, label).is_some(), "{label}");
    }

    let accepted = text_of(&card(&web, "accepted").await);
    assert!(
        accepted.contains("Set up. Checks: test.sh, lint.sh"),
        "{accepted}"
    );

    let failed = card(&web, "failed").await;
    assert!(text_of(&failed).contains("The repository has no commits yet."));
    assert!(button(&failed, "Try again").is_some());
}

#[tokio::test]
async fn use_these_posts_the_choice_and_plan_it_posts_it_with_the_plan() {
    let web = Web::start().await;
    let ready = card(&web, "ready").await;
    let posted = |label: &str| -> SetupChoice {
        let form = named(&ready, "form")
            .into_iter()
            .find(|form| button(form.inner_html(), label).is_some())
            .unwrap_or_else(|| panic!("a form with {label}"));
        assert!(form.attr_has("data-on:submit", "/leads/ready/setup/accept"));
        let choice = named(form.inner_html(), "input")
            .into_iter()
            .find(|input| input.attr_is("name", "choice"))
            .and_then(|input| input.attr("value"))
            .expect("the choice");
        serde_json::from_str(&choice).unwrap()
    };
    let kept = posted("Use these");
    assert_eq!(kept.verify, [vec!["./scripts/test.sh".to_string()]]);
    assert_eq!(kept.capture, ["./scripts/capture.sh"]);
    assert_eq!(kept.release.len(), 2);
    assert_eq!(kept.protected, ["saves/**", ".env*"]);
    assert_eq!(kept.daily_usd, Some(10.0));
    assert!(!kept.plan_first);
    let planned = posted("Plan it");
    assert!(planned.plan_first);
    assert_eq!(planned.verify, kept.verify);
    // What the button posts is what studiod is sent.
    let json = serde_json::to_string(&kept).unwrap();
    web.post("/leads/ready/setup/accept", &[("choice", &json)])
        .await;
    assert_eq!(
        web.sent(),
        [Request::AcceptSetup {
            project: "ready".into(),
            choice: kept
        }]
    );
}

#[tokio::test]
async fn once_the_setup_is_accepted_plan_it_starts_the_goal() {
    let web = Web::start().await;
    let accepted = card(&web, "accepted").await;
    let form = named(&accepted, "form")
        .into_iter()
        .find(|form| button(form.inner_html(), "Plan it").is_some())
        .expect("a Plan it form");
    assert!(form.attr_has("data-on:submit", "'/goals'"));
    let value = |name: &str| {
        named(form.inner_html(), "input")
            .into_iter()
            .find(|input| input.attr_is("name", name))
            .and_then(|input| input.attr("value"))
    };
    assert_eq!(value("project").as_deref(), Some("accepted"));
    assert_eq!(value("hire").as_deref(), Some("mira"));
    assert_eq!(value("text").as_deref(), Some("Fix the three crashes"));
    assert_eq!(value("version").as_deref(), Some("0.9"));
    // It is offered with the lead's words, and can be put off.
    let text = text_of(&accepted);
    assert!(text.contains("Shall I plan it as v0.9?"), "{text}");
    assert!(button(&accepted, "Not yet").is_some());
    web.post(
        "/goals",
        &[("project", "accepted"), ("text", "Fix the three crashes")],
    )
    .await;
    assert!(
        matches!(&web.sent()[..], [Request::CreateGoal { project, .. }] if project == "accepted")
    );
}

#[tokio::test]
async fn try_again_sets_the_project_up_again_in_its_folder_with_its_lead() {
    let web = Web::start().await;
    let said = web.post("/leads/failed/setup/retry", &[]).await;
    assert!(said.contains("failed"), "{said}");
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "/games/failed".into(),
            name: Some("failed".into()),
            lead: Some("mira".into()),
            new_lead: None,
        }]
    );
}

#[tokio::test]
async fn the_settings_hold_the_money_the_desks_the_hires_the_rules_and_the_office() {
    let web = Web::start().await;
    let html = web.page("/studio").await;
    let text = text_of(&html);
    for part in ["Money", "Desks", "Hires", "Rules", "The office"] {
        assert!(text.contains(part), "{part}");
    }
    assert!(text.contains("Up to 3 agents work at once"), "{text}");
    assert!(text.contains("Nora lead · opus"), "{text}");
    assert!(text.contains("dev engineer · sonnet"), "{text}");
    let posts = |to: &str| {
        named(&html, "form")
            .iter()
            .any(|form| form.attr_has("data-on:submit", to))
    };
    // The existing forms: the limits, the desks, hiring.
    for to in ["/office/limits", "/office/desks", "/office/hires"] {
        assert!(posts(to), "{to}");
    }
    // One task's limit shows as it stands.
    assert!(html.contains(r#"value="2.5""#));
    assert!(
        named(&html, "a")
            .iter()
            .any(|a| a.attr_is("href", "/rules"))
    );
    assert!(button(&html, "Close the office").is_some());
}
