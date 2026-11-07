#![allow(clippy::unwrap_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, named, text_of};

const TOKEN: &str = "0123abcd";

fn office() -> Value {
    json!({
        "status": {
            "projects": ["shop"], "daily_usd": 10.0, "spent_24h": 2.0,
            "team": [{"id":"nora","name":"Nora","role":"lead","model":"opus"},
                     {"id":"dev","name":"Dev","role":"engineer","model":"sonnet"}],
        },
        "tickets": [{
            "id":"SHOP-0", "project":"shop", "title":"Foundations", "goal":"Checkout",
            "hire":"dev", "state":"queued", "attempts":0, "base_branch":"main", "created_at":0,
        }, {
            "id":"SHOP-1", "project":"shop", "title":"Cart drawer", "goal":"Checkout",
            "hire":"dev", "state":"queued", "attempts":1, "base_branch":"main", "created_at":0,
            "priority":900, "depends_on":["SHOP-0"],
        }],
        "asks": [], "events": [], "spend": {"SHOP-1": 1.25}, "merged": [], "sizes": {},
        "today": {}, "judges": {},
        "projects": [{"name":"shop","lead":"nora"}],
        "operations": {
            "settings": {"autopilot":false,"review_minutes":15,"autonomy":"supervised"},
            "reviewing": [],
            "reviews": [{"id":"r1","project":"shop","lead":"nora","at":5,"summary":"Checkout needs a next step.","usd":0.04}],
            "actions": [{
                "id":"a2", "review":"r1", "project":"shop", "lead":"nora",
                "title":"Delegate checkout", "reason":"It unblocks the milestone.",
                "act":{"do":"create_task","title":"Checkout total","detail":"Add up the cart and the delivery.","hire":"dev"},
                "state":"pending", "at":6,
            }],
        },
        "rooms": [
            {"id":"company","name":"Company room","count":1,"thinking":false,
             "last":{"id":1,"at":7,"from":{"who":"owner"},"text":"Welcome everyone"}},
            {"id":"shop","name":"shop team","project":"shop","count":0,"thinking":false},
        ],
        "now": 100_000,
    })
}

fn answer(request: &Request) -> Response {
    match request {
        Request::Office => Response::ok(office()),
        Request::Room { room, .. } => Response::ok(json!({
            "messages": [{"id":1,"at":7,"from":{"who":"owner"},"text":format!("Welcome to {room}")}],
            "thinking": false,
        })),
        Request::ReviewOperations { project, .. } => {
            Response::ok(json!({"project":project,"started":true}))
        }
        Request::DecideOperation { action, .. } => {
            Response::ok(json!({"id":action,"state":"executed"}))
        }
        Request::SetOperations { .. } => Response::ok(json!({
            "autopilot":true,"review_minutes":30,"autonomy":"balanced",
        })),
        Request::TalkToRoom { text, .. } => Response::ok(json!({
            "id":2,"at":8,"from":{"who":"owner"},"text":text,
        })),
        _ => Response::ok(Value::Null),
    }
}

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
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Web {
            base,
            http,
            seen,
            _dir: dir,
        }
    }

    async fn page(&self, path: &str) -> String {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        response.text().await.unwrap()
    }

    async fn post(&self, path: &str, form: &[(&str, &str)]) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", &self.base)
            .form(form)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
    }

    fn sent(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .drain(..)
            .filter(|request| !matches!(request, Request::Office | Request::Room { .. }))
            .collect()
    }
}

#[tokio::test]
async fn the_company_page_has_the_map_scorecard_and_decision_log() {
    let web = Web::start().await;
    let html = web.page("/company").await;
    let text = text_of(&html);
    for expected in [
        "Company",
        "Last 24 hours",
        "Live work map",
        "Employee workload",
        "Manager decision log",
        "Cart drawer",
        "Delegate checkout",
        "Cost per delivery",
        "Dependency graph",
        "Blocked by SHOP-0",
        "P900",
        "Nothing assigned yet: Nora",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(html.contains("/leads/shop?look=SHOP-1"), "{html}");
}

#[tokio::test]
async fn rooms_render_and_owner_messages_reach_the_room_api() {
    let web = Web::start().await;
    let html = web.page("/rooms/company").await;
    let text = text_of(&html);
    assert!(text.contains("Company room"), "{text}");
    assert!(text.contains("Welcome to company"), "{text}");
    assert!(text.contains("shop team"), "{text}");
    assert!(
        by_id(&html, "room-composer")
            .inner_html()
            .contains("name=\"text\"")
    );

    web.post(
        "/rooms/company/talk",
        &[("text", "@dev Please review the drawer")],
    )
    .await;
    assert_eq!(
        web.sent(),
        [Request::TalkToRoom {
            room: "company".into(),
            text: "@dev Please review the drawer".into(),
        }]
    );
}

#[tokio::test]
async fn manager_reviews_approvals_and_controls_are_real_requests() {
    let web = Web::start().await;
    let hq = web.page("/").await;
    assert!(text_of(&hq).contains("Delegate checkout"), "{hq}");
    assert!(
        named(&hq, "button")
            .iter()
            .any(|button| button.text() == "Approve")
    );

    web.post("/operations/review", &[("project", "shop")]).await;
    assert_eq!(
        web.sent(),
        [Request::ReviewOperations {
            project: "shop".into(),
        }]
    );

    web.post("/operations/a2/decision", &[("decision", "approve")])
        .await;
    assert_eq!(
        web.sent(),
        [Request::DecideOperation {
            action: "a2".into(),
            approve: true,
            note: None,
        }]
    );

    web.post(
        "/operations/settings",
        &[
            ("autopilot", "true"),
            ("review_minutes", ""),
            ("autonomy", ""),
        ],
    )
    .await;
    assert_eq!(
        web.sent(),
        [Request::SetOperations {
            autopilot: Some(true),
            review_minutes: None,
            autonomy: None,
            mode: None,
        }]
    );

    web.post(
        "/operations/settings",
        &[
            ("autopilot", ""),
            ("review_minutes", ""),
            ("mode", "simulation"),
        ],
    )
    .await;
    assert_eq!(
        web.sent(),
        [Request::SetOperations {
            autopilot: None,
            review_minutes: None,
            autonomy: None,
            mode: Some(studiod::api::RuntimeMode::Simulation),
        }]
    );
}

#[tokio::test]
async fn the_approval_card_names_the_lead_and_says_what_approving_does() {
    let web = Web::start().await;
    let text = text_of(&web.page("/").await);
    assert!(text.contains("proposed by Nora"), "{text}");
    assert!(!text.contains("proposed by nora"), "{text}");
    assert!(text.contains("Add up the cart and the delivery."), "{text}");
    // The decision log says who proposed it, by name, and in plain words.
    let company = text_of(&web.page("/company").await);
    assert!(company.contains("Delegate a task · Nora"), "{company}");
    assert!(company.contains("Waiting for you"), "{company}");
}

#[tokio::test]
async fn the_settings_say_what_each_autonomy_level_does_and_what_simulation_touches() {
    let web = Web::start().await;
    let text = text_of(&web.page("/studio").await);
    for words in [
        "Supervised Every action waits for you.",
        "Balanced Feedback and messages to employees go ahead; everything else waits.",
        "Autonomous Task management goes ahead, and up to 3 new goals or tasks a day per project; cancelling still waits.",
        "Simulation affects only conversations, rooms, standups, setup and reviews, never coding work or approvals.",
    ] {
        assert!(text.contains(words), "{words}: {text}");
    }
}

#[tokio::test]
async fn a_phone_can_switch_rooms_and_the_owner_has_a_picture() {
    let web = Web::start().await;
    let html = web.page("/rooms/shop").await;
    let navs = named(&html, "nav")
        .into_iter()
        .filter(|nav| nav.attr_is("aria-label", "Rooms"))
        .collect::<Vec<_>>();
    // One beside the room on a wide screen, one above it on a phone.
    assert_eq!(navs.len(), 2, "{html}");
    for nav in navs {
        let inner = nav.inner_html();
        assert!(inner.contains("/rooms/company"), "{inner}");
        assert!(inner.contains("/rooms/shop"), "{inner}");
    }
    // Every picture the page draws is one the page has.
    let owner = web.page("/rooms/company").await;
    assert!(owner.contains("#i-user-round"), "{owner}");
    assert!(!owner.contains("#i-user\""), "{owner}");
}

#[tokio::test]
async fn a_person_page_has_their_work_on_a_phone_and_a_lead_has_a_way_to_their_project() {
    let web = Web::start().await;
    let html = web.page("/people/dev").await;
    let text = text_of(&html);
    assert!(
        by_id(&html, "person-work")
            .inner_html()
            .contains("Cart drawer"),
        "{html}"
    );
    assert!(text.contains("Current and recent work"), "{text}");
    assert!(text.contains("shop · Queued"), "{text}");
    assert!(!text.contains("read-only"), "{text}");
    assert!(!text.contains("Open the shop conversation"), "{text}");
    // The two lists have their own ids, so a patch finds the right one.
    assert!(html.contains("id=\"person-rail\""), "{html}");
    assert!(html.contains("id=\"person-rail-phone\""), "{html}");

    let lead = web.page("/people/nora").await;
    assert!(
        named(&lead, "a")
            .iter()
            .any(|a| a.attr_is("href", "/leads/shop")
                && a.text().contains("Open the shop conversation")),
        "{lead}"
    );
}

#[tokio::test]
async fn an_earlier_page_of_a_room_has_a_way_back_to_the_latest() {
    let web = Web::start().await;
    let latest = web.page("/rooms/shop").await;
    assert!(!text_of(&latest).contains("Back to the latest messages"));
    let earlier = web.page("/rooms/shop?before=3").await;
    assert!(
        named(&earlier, "a")
            .iter()
            .any(|a| a.attr_is("href", "/rooms/shop") && a.text() == "Back to the latest messages"),
        "{earlier}"
    );
}
