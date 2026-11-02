//! The team pages against a stand-in studiod: each page answers with its
//! heading inside the team's sidebar, and what the owner posts reaches studiod
//! as the request it should.
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
use probe::{by_id, find, has_id, named, with};

const TOKEN: &str = "0123abcd";

/// What the stand-in studiod says of the office: the shop, which Nora leads
/// and where something waits for the owner, and the docs, which nobody leads.
fn office() -> Value {
    json!({
        "status": {
            "projects": ["shop", "docs"], "hires": ["nora", "dev"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "role": "engineer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.59, "max_runs": 3,
        },
        "tickets": [{
            "id": "SHOP-1", "project": "shop", "title": "Add a cart drawer",
            "goal": "", "hire": "dev", "state": "working", "attempts": 1,
            "base_branch": "main", "created_at": 0,
        }],
        "asks": [{
            "id": "a1", "ticket": "SHOP-1", "hire": "dev", "kind": "permission",
            "tool": "Bash", "input": {"command": "./test.sh"}, "reasons": [],
        }],
        "events": [], "spend": {}, "merged": [], "sizes": {}, "today": {}, "judges": {},
        "projects": [{"name": "shop", "lead": "nora"}, {"name": "docs"}],
        "now": 1_700_000_000_000_u64,
    })
}

fn answer(request: &Request) -> Response {
    match request {
        Request::Office => Response::ok(office()),
        Request::Talk { project, .. } if project == "docs" => Response::err(
            "docs has no lead yet. Give it one in its conversation in the web UI, or with \
                 `studio lead docs <hire>`.",
        ),
        Request::Talk { text, .. } => Response::ok(json!({
            "id": 1, "at": 5, "from": {"who": "owner"}, "text": text,
        })),
        Request::Tell { task, .. } => Response::ok(json!({"task": task, "how": "live"})),
        Request::SetupProject { name, .. } => Response::ok(json!({
            "project": name.clone().unwrap_or_else(|| "garden".into()),
            "lead": "mira", "state": "reading",
        })),
        Request::AcceptSetup { project, .. } => {
            Response::ok(json!({"project": project, "state": "accepted"}))
        }
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

    async fn get(&self, path: &str) -> reqwest::Response {
        self.http
            .get(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
            .await
            .unwrap()
    }

    /// The page at `path`, which must be there.
    async fn page(&self, path: &str) -> String {
        let reply = self.get(path).await;
        assert_eq!(reply.status(), 200, "{path}");
        reply.text().await.unwrap()
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

    /// What studiod was sent besides a look at the office, since the last call.
    fn sent(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .drain(..)
            .filter(|request| *request != Request::Office)
            .collect()
    }
}

/// The text of each `<h1>` on a page.
fn headings(html: &str) -> Vec<String> {
    named(html, "h1").iter().map(|h| h.text()).collect()
}

#[tokio::test]
async fn every_team_page_answers_with_its_heading() {
    let web = Web::start().await;
    for (path, heading) in [
        ("/leads/shop", "Talk to Nora"),
        // A project nobody leads is called by its own name.
        ("/leads/docs", "Talk to docs"),
        ("/office", "The office"),
        ("/new", "New project"),
        ("/studio", "Studio settings"),
    ] {
        let html = web.page(path).await;
        assert_eq!(headings(&html), [heading], "{path}");
    }
    // A project the studio doesn't have has no conversation.
    assert_eq!(web.get("/leads/garden").await.status(), 404);
    // The home page is HQ, which greets the owner.
    let greeting = headings(&web.page("/").await);
    assert!(
        ["Good morning", "Good afternoon", "Good evening"]
            .iter()
            .any(|g| greeting == [*g]),
        "{greeting:?}"
    );
    // The addresses HQ and the office had while they were made lead on.
    for (old, new) in [("/hq", "/"), ("/team/office", "/office")] {
        let reply = web.get(old).await;
        assert_eq!(reply.status(), 308, "{old}");
        assert_eq!(reply.headers()["location"], new, "{old}");
    }
}

#[tokio::test]
async fn every_team_page_sits_in_the_team_sidebar_and_marks_where_it_is() {
    let web = Web::start().await;
    let pages = [
        ("/", "/"),
        ("/leads/shop", "/leads/shop"),
        ("/leads/docs", "/leads/docs"),
        ("/office", "/office"),
        ("/new", "/new"),
        ("/studio", "/studio"),
    ];
    for (path, current) in pages {
        let html = web.page(path).await;
        let sidebar = by_id(&html, "team-sidebar");
        let links: Vec<String> = named(sidebar.inner_html(), "a")
            .iter()
            .filter_map(|a| a.attr("href"))
            .collect();
        for to in [
            "/",
            "/leads/shop",
            "/leads/docs",
            "/new",
            "/office",
            "/studio",
        ] {
            assert!(links.iter().any(|l| l == to), "{path}: no link to {to}");
        }
        // Only where the owner is is marked.
        let marked: Vec<String> = with(&html, "aria-current", "page")
            .iter()
            .filter_map(|a| a.attr("href"))
            .collect();
        assert_eq!(marked, [current], "{path}");
        // The team's sidebar is the only one, not the old one inside it.
        assert!(
            with(&html, "aria-label", "Studio").is_empty(),
            "{path}: the old sidebar"
        );
        assert!(
            has_id(&html, "notice"),
            "{path}: nowhere to answer an action"
        );
    }
}

#[tokio::test]
async fn the_sidebar_tells_each_lead_its_project_its_phase_and_what_waits() {
    let web = Web::start().await;
    let html = web.page("/").await;
    let sidebar = by_id(&html, "team-sidebar");
    let links = named(sidebar.inner_html(), "a");
    // The words of each link that leads to `to`.
    let words = |to: &str| -> Vec<String> {
        links
            .iter()
            .filter(|a| a.attr_is("href", to))
            .map(|a| a.text())
            .collect()
    };
    // The lead's name, its project and phase, and the one thing that waits.
    let shop = words("/leads/shop");
    assert_eq!(shop.len(), 1, "{shop:?}");
    assert!(shop[0].contains("Nora"), "{shop:?}");
    assert!(shop[0].contains("shop · Building"), "{shop:?}");
    assert!(shop[0].ends_with(" 1"), "{shop:?}");
    // A project without a lead has only its name to show.
    let docs = words("/leads/docs");
    assert_eq!(docs.len(), 1, "{docs:?}");
    assert!(docs[0].contains("docs · Idle"), "{docs:?}");
    assert!(!docs[0].ends_with(" 1"), "{docs:?}");
    // HQ counts what waits in every project.
    let hq = words("/");
    assert!(
        hq.iter().any(|t| t.starts_with("HQ") && t.ends_with(" 1")),
        "{hq:?}"
    );
    assert!(words("/new")[0].contains("Hire a lead for it"));
    assert_eq!(find(&html, "data-phase", "building").name, "a");
    let text = sidebar.text();
    assert!(text.contains("Your leads"), "{text}");
    assert!(text.contains("Spent today $6.59 of $10"), "{text}");
}

#[tokio::test]
async fn the_sheet_opens_over_a_conversation_when_work_is_picked() {
    let web = Web::start().await;
    let plain = web.page("/leads/shop").await;
    assert!(!has_id(&plain, "sheet"));
    let html = web.page("/leads/shop?look=SHOP-1").await;
    let sheet = by_id(&html, "sheet");
    assert_eq!(sheet.attr("data-work").as_deref(), Some("SHOP-1"));
    assert_eq!(
        named(sheet.inner_html(), "h2")
            .iter()
            .map(|h| h.text())
            .collect::<Vec<_>>(),
        ["Add a cart drawer"]
    );
    // Work the project does not have opens nothing, and only what is safe in
    // an address is looked for.
    assert!(!has_id(
        &web.page("/leads/shop?look=a%27b%3Cc").await,
        "sheet"
    ));
    // An empty pick opens nothing.
    assert!(!has_id(&web.page("/leads/shop?look=").await, "sheet"));
}

#[tokio::test]
async fn what_the_owner_posts_reaches_studiod_as_its_request() {
    let web = Web::start().await;

    let said = web
        .post("/leads/shop/talk", &[("text", "How's it going?")])
        .await;
    // The message turns up in the thread, so nothing is said over the composer.
    assert!(!said.contains("Sent to"), "{said}");
    assert_eq!(
        web.sent(),
        [Request::Talk {
            project: "shop".into(),
            text: "How's it going?".into()
        }]
    );
    // What studiod refuses, the owner is told.
    let refused = web.post("/leads/docs/talk", &[("text", "Hello")]).await;
    assert!(refused.contains("docs has no lead yet"), "{refused}");
    assert_eq!(web.sent().len(), 1);

    let told = web
        .post("/tasks/SHOP-1/tell", &[("text", "Use three colours.")])
        .await;
    assert!(told.contains("SHOP-1"), "{told}");
    assert_eq!(
        web.sent(),
        [Request::Tell {
            task: "SHOP-1".into(),
            text: "Use three colours.".into()
        }]
    );
}

#[tokio::test]
async fn a_new_project_is_set_up_with_an_existing_lead_or_a_new_one() {
    let web = Web::start().await;

    let hired = web
        .post(
            "/new",
            &[
                ("path", " ~/games/garden "),
                ("name", ""),
                ("lead", ""),
                ("new_lead_name", "Mira"),
                ("new_lead_model", "opus"),
            ],
        )
        .await;
    assert!(hired.contains("garden"), "{hired}");
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "~/games/garden".into(),
            name: None,
            lead: None,
            new_lead: Some(NewLead {
                name: "Mira".into(),
                model: "opus".into()
            }),
        }]
    );
    web.post(
        "/new",
        &[
            ("path", "~/games/garden"),
            ("name", "plants"),
            ("lead", "nora"),
        ],
    )
    .await;
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "~/games/garden".into(),
            name: Some("plants".into()),
            lead: Some("nora".into()),
            new_lead: None,
        }]
    );
    // Without a folder or a lead there is nothing to set up.
    let nothing = web.post("/new", &[("path", ""), ("lead", "nora")]).await;
    assert!(nothing.contains("folder"), "{nothing}");
    let nobody = web.post("/new", &[("path", "~/games/garden")]).await;
    assert!(nobody.contains("lead"), "{nobody}");
    assert!(web.sent().is_empty());
}

#[tokio::test]
async fn the_owners_choice_for_a_setup_reaches_studiod() {
    let web = Web::start().await;
    let choice = SetupChoice {
        verify: vec![vec!["./test.sh".into()]],
        protected: vec!["saves/**".into()],
        daily_usd: Some(10.0),
        plan_first: true,
        ..SetupChoice::default()
    };
    let text = serde_json::to_string(&choice).unwrap();
    let done = web
        .post("/leads/garden/setup/accept", &[("choice", &text)])
        .await;
    assert!(done.contains("garden"), "{done}");
    assert_eq!(
        web.sent(),
        [Request::AcceptSetup {
            project: "garden".into(),
            choice
        }]
    );
    // A choice that can't be read is not sent.
    let unread = web
        .post("/leads/garden/setup/accept", &[("choice", "not json")])
        .await;
    assert!(unread.contains("can't be read"), "{unread}");
    assert!(web.sent().is_empty());
}

#[tokio::test]
async fn the_office_opens_with_what_the_address_picks() {
    let web = Web::start().await;
    // Nothing is picked: no lead's card.
    let bare = web.page("/office").await;
    assert!(!by_id(&bare, "office-peek").text().contains("Talk to"));
    // A project's sign is picked as a click on it picks it.
    let picked = web.page("/office?pick=projects/shop").await;
    let card = by_id(&picked, "office-peek").text();
    assert!(card.contains("Talk to Nora"), "{card}");
    // What a click could not write picks nothing.
    let odd = web.page("/office?pick=nonsense").await;
    assert!(!by_id(&odd, "office-peek").text().contains("Talk to"));
}
