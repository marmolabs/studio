//! Giving a project a lead from its conversation, against a stand-in studiod
//! that keeps its hires and its projects' leads: the card of a project that has
//! none, the menu that changes a lead, the routes they post to, what the pages
//! turn into, and what HQ says of a project that has no lead.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, NewLead, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, has_id, named, text_of, with};

const TOKEN: &str = "0123abcd";
const NOW: u64 = 1_700_000_000_000;

/// What the stand-in studio holds: a shop that Nora leads, and the docs and
/// the api, which nobody does.
struct Studio {
    /// Every hire, as (id, name, role).
    hires: Vec<(String, String, String)>,
    /// Each project's lead.
    leads: BTreeMap<String, Option<String>>,
    /// The lead of the shop has said something in its conversation.
    spoken: bool,
    /// The shop has been set up through its lead.
    set_up: bool,
    /// Naming a lead is refused.
    refuse_lead: bool,
    /// Every request, as it came.
    seen: Vec<Request>,
}

impl Studio {
    fn new() -> Studio {
        let hire = |id: &str, name: &str, role: &str| (id.into(), name.into(), role.into());
        Studio {
            hires: vec![
                hire("nora", "Nora", "lead"),
                hire("theo", "Theo", "lead"),
                hire("dev", "Dev", "engineer"),
            ],
            leads: [("shop", Some("nora")), ("docs", None), ("api", None)]
                .into_iter()
                .map(|(project, lead)| (project.to_string(), lead.map(String::from)))
                .collect(),
            spoken: true,
            set_up: false,
            refuse_lead: false,
            seen: Vec::new(),
        }
    }

    fn status(&self) -> Value {
        json!({
            "projects": self.leads.keys().collect::<Vec<_>>(),
            "hires": self.hires.iter().map(|(id, ..)| id).collect::<Vec<_>>(),
            "team": self.hires.iter().map(|(id, name, role)| {
                json!({"id": id, "name": name, "role": role, "model": "opus"})
            }).collect::<Vec<_>>(),
            "daily_usd": 10.0, "spent_24h": 1.5, "max_runs": 3,
        })
    }

    fn office(&self) -> Value {
        json!({
            "status": self.status(),
            "tickets": [], "asks": [], "events": [], "spend": {}, "merged": [],
            "sizes": {}, "today": {}, "judges": {},
            "projects": self.leads.iter().map(|(name, lead)| {
                json!({"name": name, "lead": lead, "path": format!("/work/{name}")})
            }).collect::<Vec<_>>(),
            "setups": if self.set_up {
                json!({"shop": {"project": "shop", "lead": "nora", "state": "accepted"}})
            } else {
                json!({})
            },
            "now": NOW,
        })
    }

    fn answer(&mut self, request: &Request) -> Response {
        self.seen.push(request.clone());
        match request {
            Request::Office => Response::ok(self.office()),
            Request::Status => Response::ok(self.status()),
            Request::Budget => Response::ok(json!({"limits": {}, "spent_24h": {}})),
            Request::Conversation { project, .. } => {
                let said = (project == "shop" && self.spoken).then(|| {
                    json!({"id": 1, "at": NOW - 60_000, "from": {"who": "lead", "hire": "nora"},
                           "text": "All quiet."})
                });
                Response::ok(json!({"messages": said.into_iter().collect::<Vec<_>>(),
                                    "thinking": false}))
            }
            Request::SaveFile {
                kind: FileKind::Hires,
                name,
                text,
            } => {
                // The name and the role the file gives, as the engine loads it.
                let field = |key: &str| {
                    text.lines()
                        .find_map(|line| line.strip_prefix(key))
                        .map(|value| serde_json::from_str(value).unwrap_or_else(|_| value.into()))
                        .unwrap_or_else(|| Value::String(String::new()))
                };
                let (called, role) = (field("name: "), field("role: "));
                self.hires.retain(|(id, ..)| id != name);
                self.hires.push((
                    name.clone(),
                    called.as_str().unwrap_or_default().to_string(),
                    role.as_str().unwrap_or_default().to_string(),
                ));
                Response::ok(json!({"name": name}))
            }
            Request::DeleteFile {
                kind: FileKind::Hires,
                name,
            } => {
                self.hires.retain(|(id, ..)| id != name);
                Response::ok(json!({"name": name}))
            }
            Request::SetProjectLead { project, hire } => {
                if self.refuse_lead {
                    return Response::err("the studio can't change its projects right now");
                }
                self.leads.insert(project.clone(), hire.clone());
                Response::ok(json!({"project": project, "lead": hire}))
            }
            Request::SetupProject { .. } => Response::ok(json!({
                "project": "shop", "lead": "nora", "state": "reading",
            })),
            _ => Response::ok(Value::Null),
        }
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply.
fn fake_studiod(socket: &Path, studio: Arc<Mutex<Studio>>) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let studio = Arc::clone(&studio);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let reply = studio.lock().unwrap().answer(&request);
                let mut out = serde_json::to_string(&reply).unwrap();
                out.push('\n');
                write.write_all(out.as_bytes()).await.unwrap();
            });
        }
    });
}

struct Web {
    base: String,
    http: reqwest::Client,
    studio: Arc<Mutex<Studio>>,
    _dir: tempfile::TempDir,
}

impl Web {
    async fn start() -> Web {
        Web::with(Studio::new()).await
    }

    async fn with(studio: Studio) -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let studio = Arc::new(Mutex::new(studio));
        fake_studiod(&socket, Arc::clone(&studio));
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
            studio,
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

    async fn page(&self, path: &str) -> String {
        let reply = self.get(path).await;
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

    /// What studiod was asked besides to say how things stand, since the last
    /// call.
    fn sent(&self) -> Vec<Request> {
        std::mem::take(&mut self.studio.lock().unwrap().seen)
            .into_iter()
            .filter(|request| {
                !matches!(
                    request,
                    Request::Office | Request::Budget | Request::Conversation { .. }
                )
            })
            .collect()
    }

    fn lead_of(&self, project: &str) -> Option<String> {
        self.studio.lock().unwrap().leads[project].clone()
    }
}

/// The next patch of a stream that has `needle` in it.
async fn patch_with(reply: &mut reqwest::Response, needle: &str) -> String {
    let mut text = String::new();
    while !text.contains(needle) {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(8), reply.chunk())
            .await
            .expect("a patch within eight seconds")
            .unwrap()
            .expect("a chunk");
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
}

/// The hire file `name` makes, as `Mira` the strong model's lead.
fn lead_file(name: &str) -> (String, String) {
    NewLead {
        name: name.into(),
        model: "opus".into(),
    }
    .hire_file()
    .unwrap()
}

// ------------------------------------------------------------------ the card

#[tokio::test]
async fn a_project_without_a_lead_has_a_card_where_the_message_box_would_be() {
    let web = Web::start().await;
    let html = web.page("/leads/docs").await;
    assert!(!has_id(&html, "composer"));
    let card = by_id(&html, "lead-card");
    assert!(
        card.text().starts_with(
            "docs has no lead yet. A lead plans the work, hands it out and answers the team's routine questions."
        ),
        "{}",
        card.text()
    );
    // The studio's settings are not where a lead is picked.
    assert!(!html.contains("the studio settings"), "{html}");

    // Hire a lead: a name field with the first name nobody has, a strong model,
    // and the route that hires and makes the hire the lead.
    let hire = by_id(&html, "hire-lead");
    assert!(hire.text().contains("Hire a lead"), "{}", hire.text());
    assert!(hire.attr_has("data-on:submit", "@post('/office/projects/lead/hire'"));
    let fields: Vec<(String, String)> = named(hire.inner_html(), "input")
        .iter()
        .map(|input| (input.attr("name").unwrap(), input.attr("value").unwrap()))
        .collect();
    assert_eq!(
        fields,
        [
            ("project".to_string(), "docs".to_string()),
            ("name".to_string(), "Mira".to_string())
        ]
    );
    assert!(hire.inner_html().contains("strong model (Opus)"));

    // Pick one you have: the hires that lead, whichever project they lead now,
    // and the route that names a lead.
    let pick = by_id(&html, "pick-lead");
    assert!(pick.text().contains("Pick one you have"), "{}", pick.text());
    assert!(pick.attr_has("data-on:submit", "@post('/office/projects/lead'"));
    assert!(!pick.attr_has("data-on:submit", "/hire"));
    let hidden = named(pick.inner_html(), "input");
    assert!(hidden[0].attr_is("name", "project") && hidden[0].attr_is("value", "docs"));
    let select = named(pick.inner_html(), "select");
    assert!(select[0].attr_is("name", "hire"));
    let picks: Vec<(String, String)> = named(pick.inner_html(), "option")
        .iter()
        .map(|option| (option.attr("value").unwrap(), option.text()))
        .collect();
    assert_eq!(
        picks,
        [
            ("nora".to_string(), "Nora".to_string()),
            ("theo".to_string(), "Theo".to_string())
        ]
    );
}

#[tokio::test]
async fn the_name_suggested_is_the_first_nobody_on_the_team_has() {
    let mut studio = Studio::new();
    studio
        .hires
        .push(("mira".into(), "Mira".into(), "engineer".into()));
    let web = Web::with(studio).await;
    let html = web.page("/leads/docs").await;
    // Mira and Nora are taken, the one by an engineer.
    let name = find(&html, "name", "name");
    assert_eq!(name.attr("value").as_deref(), Some("Sam"));
}

#[tokio::test]
async fn without_a_lead_to_pick_the_card_only_hires() {
    let mut studio = Studio::new();
    studio.hires.retain(|(_, _, role)| role != "lead");
    let web = Web::with(studio).await;
    let html = web.page("/leads/docs").await;
    assert!(has_id(&html, "hire-lead"));
    assert!(!has_id(&html, "pick-lead"));
    assert!(!text_of(&html).contains("Pick one you have"));
}

#[tokio::test]
async fn hire_a_lead_makes_the_hire_and_sets_the_lead() {
    let web = Web::start().await;
    let said = web
        .post(
            "/office/projects/lead/hire",
            &[("project", "docs"), ("name", "  Mira ")],
        )
        .await;
    // The form is told, under its name field.
    assert!(said.contains("datastar-patch-signals"), "{said}");
    assert!(said.contains("Hired Mira to lead docs."), "{said}");
    assert!(said.contains(r#""_bad":false"#), "{said}");
    // The hire's file first, a lead's on a strong model with the tool to plan
    // goals, then the lead.
    let (id, text) = lead_file("Mira");
    assert_eq!(id, "mira");
    assert_eq!(
        web.sent(),
        [
            Request::Status,
            Request::SaveFile {
                kind: FileKind::Hires,
                name: "mira".into(),
                text: text.clone(),
            },
            Request::SetProjectLead {
                project: "docs".into(),
                hire: Some("mira".into()),
            },
        ]
    );
    for line in [
        "name: \"Mira\"",
        "role: lead",
        "model: \"opus\"",
        "tools: [file_ticket]",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}: {text}");
    }
    studio_config::check_hire("mira", &text).unwrap();
    assert_eq!(web.lead_of("docs").as_deref(), Some("mira"));
}

#[tokio::test]
async fn a_hire_that_cannot_be_made_is_refused_under_the_name_field_and_nothing_is_written() {
    let web = Web::start().await;
    for (project, name, said) in [
        // The id nora has, or the name it goes by, in any case.
        (
            "docs",
            "nora",
            "A hire called nora already exists. Pick another name.",
        ),
        (
            "docs",
            " NORA ",
            "A hire called NORA already exists. Pick another name.",
        ),
        ("docs", "", "Give the lead a short name."),
        ("docs", "!!!", "Use letters or digits in the lead's name."),
        ("garden", "Mira", "The studio has no project garden."),
        ("", "Mira", "Pick a project."),
    ] {
        let reply = web
            .post(
                "/office/projects/lead/hire",
                &[("project", project), ("name", name)],
            )
            .await;
        assert!(reply.contains(said), "{name:?}: {reply}");
        assert!(reply.contains(r#""_bad":true"#), "{name:?}: {reply}");
        // Nothing was hired and nobody was made a lead.
        assert!(
            web.sent()
                .iter()
                .all(|r| !matches!(r, Request::SaveFile { .. } | Request::SetProjectLead { .. })),
            "{name:?}"
        );
    }
    assert_eq!(web.lead_of("docs"), None);
}

#[tokio::test]
async fn a_hire_that_cannot_lead_is_let_go() {
    let mut studio = Studio::new();
    studio.refuse_lead = true;
    let web = Web::with(studio).await;
    let reply = web
        .post(
            "/office/projects/lead/hire",
            &[("project", "docs"), ("name", "Mira")],
        )
        .await;
    assert!(
        reply.contains("the studio can't change its projects right now"),
        "{reply}"
    );
    assert!(reply.contains(r#""_bad":true"#), "{reply}");
    let sent = web.sent();
    assert!(matches!(&sent[1], Request::SaveFile { name, .. } if name == "mira"));
    assert!(matches!(&sent[2], Request::SetProjectLead { .. }));
    // The file it wrote is deleted again, so a second try is not "already
    // hired".
    assert_eq!(
        sent[3],
        Request::DeleteFile {
            kind: FileKind::Hires,
            name: "mira".into()
        }
    );
    assert!(
        web.studio
            .lock()
            .unwrap()
            .hires
            .iter()
            .all(|(id, ..)| id != "mira")
    );
}

#[tokio::test]
async fn picking_a_lead_you_have_names_it_through_the_existing_route() {
    let web = Web::start().await;
    let said = web
        .post(
            "/office/projects/lead",
            &[("project", "docs"), ("hire", "theo")],
        )
        .await;
    assert!(said.contains("Theo now leads docs."), "{said}");
    assert!(web.sent().contains(&Request::SetProjectLead {
        project: "docs".into(),
        hire: Some("theo".into()),
    }));
    assert_eq!(web.lead_of("docs").as_deref(), Some("theo"));
}

#[tokio::test]
async fn the_page_turns_into_the_conversation_with_the_new_lead_as_the_stream_patches_it() {
    let web = Web::start().await;
    let mut reply = web.get("/team/stream?at=%2Fleads%2Fdocs").await;
    assert_eq!(reply.status(), 200);
    // As it is now: the card, no message box, and the project's own face in the
    // sidebar.
    let first = patch_with(&mut reply, r#"id="lead-card""#).await;
    assert!(!first.contains(r#"id="composer""#), "{first}");
    assert!(first.contains("Nobody leads docs yet"), "{first}");

    web.post(
        "/office/projects/lead/hire",
        &[("project", "docs"), ("name", "Mira")],
    )
    .await;
    // The next patch has the lead: its message box, its face and name in the
    // header and the sidebar, and a menu to change it; the card is gone.
    let next = patch_with(&mut reply, r#"id="composer""#).await;
    assert!(!next.contains(r#"id="lead-card""#), "{next}");
    let composer = by_id(&next, "composer");
    assert!(composer.attr_has("data-on:submit", "/leads/docs/talk"));
    assert_eq!(
        find(composer.inner_html(), "name", "text")
            .attr("placeholder")
            .as_deref(),
        Some("Message Mira")
    );
    let head = by_id(&next, "talk-head");
    assert!(head.text().contains("Leads docs"), "{}", head.text());
    assert!(head.inner_html().contains(r#"data-face="mira""#));
    assert!(
        by_id(&next, "team-sidebar")
            .inner_html()
            .contains(r#"data-face="mira""#)
    );
    assert!(has_id(&next, "change-lead-mira"));
}

// ------------------------------------------------------------------ the menu

#[tokio::test]
async fn change_lead_offers_another_lead_a_new_hire_or_none() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let menu = by_id(&html, "change-lead-nora");
    assert_eq!(menu.name, "details");
    assert!(menu.text().starts_with("Change lead"), "{}", menu.text());
    // Another lead: the others the studio has, not the lead it has.
    let swap = by_id(&html, "swap-lead");
    assert!(swap.attr_has("data-on:submit", "@post('/office/projects/lead'"));
    let others: Vec<String> = named(swap.inner_html(), "option")
        .iter()
        .map(|option| option.attr("value").unwrap())
        .collect();
    assert_eq!(others, ["theo"]);
    // A new hire, to the route that hires.
    let hire = by_id(&html, "hire-another-lead");
    assert!(hire.attr_has("data-on:submit", "@post('/office/projects/lead/hire'"));
    let name = find(hire.inner_html(), "name", "name");
    assert_eq!(name.attr("value").as_deref(), Some("Mira"));
    // No lead: the same route as picking, with no hire, once the owner is sure.
    let leave = by_id(&html, "leave-lead");
    assert!(leave.attr_has("data-on:submit", "confirm("));
    assert!(leave.attr_has("data-on:submit", "@post('/office/projects/lead'"));
    let fields: Vec<(String, String)> = named(leave.inner_html(), "input")
        .iter()
        .map(|input| (input.attr("name").unwrap(), input.attr("value").unwrap()))
        .collect();
    assert_eq!(
        fields,
        [
            ("project".to_string(), "shop".to_string()),
            ("hire".to_string(), String::new())
        ]
    );
    // A project that has no lead has the card and no menu.
    let docs = web.page("/leads/docs").await;
    assert!(!docs.contains("change-lead-"), "{docs}");
    assert!(!by_id(&docs, "talk-head").text().contains("Change lead"));
}

#[tokio::test]
async fn change_lead_works_through_the_same_routes() {
    let web = Web::start().await;
    // Another lead.
    web.post(
        "/office/projects/lead",
        &[("project", "shop"), ("hire", "theo")],
    )
    .await;
    assert_eq!(web.lead_of("shop").as_deref(), Some("theo"));
    // The page has the new lead, and its menu is one of its own: Nora is now
    // the other lead to pick.
    let html = web.page("/leads/shop").await;
    assert!(by_id(&html, "talk-head").text().contains("Talk to Theo"));
    let swap = by_id(&html, "swap-lead");
    let others: Vec<String> = named(swap.inner_html(), "option")
        .iter()
        .map(|option| option.attr("value").unwrap())
        .collect();
    assert_eq!(others, ["nora"]);
    assert!(has_id(&html, "change-lead-theo"));
    assert!(!has_id(&html, "change-lead-nora"));
    // The message box says whose it is. It is drawn with the page, so it follows
    // the lead; what is typed in it stays, as its field has no value of the
    // page's own to put back.
    let composer = by_id(&html, "composer");
    let field = find(composer.inner_html(), "name", "text");
    assert_eq!(field.attr("placeholder").as_deref(), Some("Message Theo"));
    assert!(!field.has("value"));
    assert!(field.has("data-bind:_draft"));

    // A new hire.
    web.sent();
    web.post(
        "/office/projects/lead/hire",
        &[("project", "shop"), ("name", "Mira")],
    )
    .await;
    assert_eq!(web.lead_of("shop").as_deref(), Some("mira"));
    assert!(web.sent().contains(&Request::SetProjectLead {
        project: "shop".into(),
        hire: Some("mira".into())
    }));

    // No lead: the card is back, with a message box nowhere.
    let said = web
        .post(
            "/office/projects/lead",
            &[("project", "shop"), ("hire", "")],
        )
        .await;
    assert!(said.contains("shop has no lead now."), "{said}");
    assert_eq!(web.lead_of("shop"), None);
    let html = web.page("/leads/shop").await;
    assert!(has_id(&html, "lead-card"));
    assert!(!has_id(&html, "composer"));
}

// ------------------------------------------------------------------------ HQ

#[tokio::test]
async fn hq_links_a_project_without_a_lead_to_its_conversation() {
    let web = Web::start().await;
    let html = web.page("/").await;
    let line = |project: &str| find(&html, "data-lead", project);
    let docs = line("docs");
    let row = find(docs.inner_html(), "data-card", "lead");
    assert!(row.text().starts_with("No lead yet"), "{}", row.text());
    let link = named(row.inner_html(), "a");
    assert_eq!(link.len(), 1);
    assert_eq!(link[0].text(), "Give docs a lead");
    assert!(link[0].attr_is("href", "/leads/docs"));
    // The api is the same, with its own name on the link.
    let api = named(line("api").inner_html(), "a")
        .into_iter()
        .find(|a| a.text().starts_with("Give "))
        .unwrap();
    assert!(api.attr_is("href", "/leads/api") && api.text() == "Give api a lead");
    // A project that has a lead has no such row.
    let shop = line("shop");
    assert!(with(shop.inner_html(), "data-card", "lead").is_empty());
    assert!(!shop.text().contains("No lead yet"));
    // A project nobody leads is named once at the head of its line, not twice
    // (the project's name is the only one it has), and its words are the day's
    // facts, not a sentence that sends the owner elsewhere.
    assert!(!docs.text().contains("docs docs"), "{}", docs.text());
    assert!(docs.text().contains("Idle."), "{}", docs.text());
    assert!(
        !docs.text().contains("Nobody leads it yet"),
        "{}",
        docs.text()
    );
}

// ----------------------------------------------------------- what a patch keeps

#[tokio::test]
async fn what_the_owner_types_in_the_card_and_the_menu_survives_a_patch() {
    let web = Web::start().await;
    // The card's forms are the owner's, whole.
    let mut docs = web.get("/team/stream?at=%2Fleads%2Fdocs").await;
    let patch = patch_with(&mut docs, r#"id="lead-card""#).await;
    for form in ["hire-lead", "pick-lead"] {
        assert!(by_id(&patch, form).has("data-ignore-morph"), "{form}");
    }
    // So is the menu, and with the lead in its id it is a fresh one once the
    // lead changes.
    let mut shop = web.get("/team/stream?at=%2Fleads%2Fshop").await;
    let patch = patch_with(&mut shop, r#"id="change-lead-nora""#).await;
    let menu = by_id(&patch, "change-lead-nora");
    assert!(menu.has("data-ignore-morph"));
    // The name it has, typed in a field of its own, and an open menu stay
    // wherever the page is patched.
    assert!(menu.inner_html().contains(r#"id="hire-another-lead-name""#));
    // What a hire form is told shows in the page's own signals, which a patch of
    // the page leaves as they are.
    let page = by_id(&patch, "talk-page");
    assert!(page.attr_has("data-signals", "_said"));
}

// ------------------------------------------------------------- the offer

#[tokio::test]
async fn a_lead_that_has_not_looked_at_the_project_offers_to_and_the_chip_starts_the_setup() {
    let mut studio = Studio::new();
    studio.spoken = false;
    let web = Web::with(studio).await;
    let html = web.page("/leads/shop").await;
    let chip = by_id(&html, "look-at-project");
    assert_eq!(chip.text(), "Ask Nora to look at how shop runs");
    assert!(chip.attr_has("data-on:click", "@post('/leads/shop/setup/start')"));
    // It is one of the composer's, and asked once: it goes as it is clicked, and
    // the page leaves it gone for as long as it is offered.
    assert!(
        by_id(&html, "composer")
            .inner_html()
            .contains("look-at-project")
    );
    assert!(chip.attr_has("data-on:click", "display = 'none'"));
    assert!(chip.attr_is("data-preserve-attr", "style"));

    let said = web.post("/leads/shop/setup/start", &[]).await;
    assert!(said.contains("Nora is looking at how shop runs."), "{said}");
    // Its lead sets up the project that is there, in its own folder, as it does
    // for a setup that failed; nothing is written to the project's files here.
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "/work/shop".into(),
            name: Some("shop".into()),
            lead: Some("nora".into()),
            new_lead: None,
        }]
    );
    // A project with no lead has none to ask.
    let none = web.post("/leads/docs/setup/start", &[]).await;
    assert!(none.contains("docs has no lead to set it up."), "{none}");
    assert!(web.sent().is_empty());
}

#[tokio::test]
async fn the_offer_is_for_a_lead_that_has_not_spoken_and_a_project_not_set_up() {
    // The lead has said something already: nothing to offer.
    let talked = Web::start().await;
    assert!(!has_id(
        &talked.page("/leads/shop").await,
        "look-at-project"
    ));
    // A project that was set up through its lead has been looked at.
    let mut studio = Studio::new();
    studio.spoken = false;
    studio.set_up = true;
    let done = Web::with(studio).await;
    assert!(!has_id(&done.page("/leads/shop").await, "look-at-project"));
    // A project nobody leads has no message box to put it in.
    assert!(!has_id(&done.page("/leads/docs").await, "look-at-project"));
}
