//! The team pages together, against a stand-in studiod: what only shows once
//! the frame, HQ, the conversation, the office and the setup are in one app.
//! There is one live update for each page, what the owner types survives it,
//! the composer is filled by the buttons that say to, the setup card names its
//! lead, every face is a real one, times are the owner's clock, and every link
//! and call of every page has a route.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, has_id, matching, named, text_of, with};

const TOKEN: &str = "0123abcd";
const NOW: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;
const DAY: u64 = 24 * 60 * MINUTE;

/// What the stand-in is told to say differently.
struct Say {
    /// How the checks of the goal that is ready to ship went.
    checks: &'static str,
    /// How the setup of the garden stands.
    setup: &'static str,
}

fn ticket(id: &str, title: &str, state: Value, hire: &str, extra: Value) -> Value {
    let mut fields = json!({
        "id": id, "project": "shop", "title": title, "goal": "", "hire": hire,
        "state": state, "attempts": 0, "base_branch": "main", "created_at": NOW - 3 * 60 * MINUTE,
    });
    fields
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    fields
}

/// The office: the shop, which Nora leads, with work at every stage; the docs,
/// which nobody leads; and the garden, which Mira (whose hire is `m-17`) is
/// setting up.
fn office(say: &Say) -> Value {
    let check = match say.checks {
        "failed" => json!({"state": "failed", "summary": "cargo test: 2 failed", "at": 1}),
        "running" => json!({"state": "running", "summary": "", "at": 1}),
        _ => json!({"state": "passed", "summary": "47 tests pass", "at": 1}),
    };
    let proposal = json!({
        "summary": "A small 2D game with tests.",
        "verify": [{"command": ["./test.sh"], "why": "",
                    "tried": {"passed": true, "seconds": 3, "summary": "112 pass", "output_tail": ""}}],
        "release": [], "release_source": "", "protected": [], "daily_usd": 10.0,
        "first_goal": {"text": "Fix the three crashes", "version": "0.9", "why": "They block play."},
    });
    let setup = match say.setup {
        "failed" => json!({
            "project": "garden", "lead": "m-17", "state": "failed",
            "error": "The repository has no commits yet.",
        }),
        // Once it is ready the lead has a proposal, and the studio has tried it.
        state @ ("ready" | "accepted") => {
            json!({"project": "garden", "lead": "m-17", "state": state, "proposal": proposal})
        }
        state => json!({"project": "garden", "lead": "m-17", "state": state}),
    };
    json!({
        "status": {
            "projects": ["shop", "docs", "garden"], "hires": ["nora", "m-17", "dev", "designer"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "m-17", "name": "Mira", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
                {"id": "designer", "name": "Designer", "role": "designer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.59, "max_runs": 3, "rules": 2,
        },
        "tickets": [
            ticket("SHOP-1", "Cart drawer", json!("working"), "dev", json!({})),
            ticket("SHOP-2", "Save carts between sessions", json!("landed"), "nora",
                json!({"kind": "plan", "goal": "Save carts between sessions", "version": "0.4"})),
            ticket("SHOP-3", "Store carts in the database", json!("landed"), "dev",
                json!({"parent": "SHOP-2", "goal": "Save carts between sessions"})),
            ticket("SHOP-4", "Bring the cart back on sign-in", json!("landed"), "dev",
                json!({"parent": "SHOP-2", "goal": "Save carts between sessions"})),
            ticket("SHOP-5", "Add a discount field", json!("ready"), "dev", json!({})),
            ticket("SHOP-6", "Product cards", json!({"asking": {"ask": "a1"}}), "designer", json!({})),
        ],
        "asks": [{
            "id": "a1", "ticket": "SHOP-6", "hire": "designer", "kind": "permission",
            "tool": "Bash", "input": {"command": "./mail.sh"}, "reasons": [],
            "at_ms": NOW - 5 * MINUTE, "title": "Send the launch email?",
        }],
        "events": [
            {"seq": 1, "ticket_id": "SHOP-1", "at_ms": NOW - 25 * MINUTE, "event": {"type": "claimed"}},
            {"seq": 2, "ticket_id": "SHOP-3", "at_ms": NOW - 40 * MINUTE, "event": {"type": "landed"}},
            {"seq": 3, "ticket_id": "SHOP-4", "at_ms": NOW - 30 * MINUTE, "event": {"type": "landed"}},
        ],
        "spend": {}, "merged": [], "today": {}, "judges": {}, "sizes": {"SHOP-5": [12, 3]},
        "projects": [
            {"name": "shop", "lead": "nora", "path": "/work/shop"},
            {"name": "docs"},
            {"name": "garden", "lead": "m-17", "path": "/work/garden"},
        ],
        "ship_checks": {"SHOP-2": check},
        "results": {
            "SHOP-3": "Stored carts in the database.",
            "SHOP-4": "Brought the cart back when a shopper signs in.",
            "SHOP-5": "Added a discount field to the cart.",
        },
        "standups": {"shop": {"at": NOW - 2 * 60 * MINUTE, "text": "The logo's done. Two things wait for you."}},
        "setups": {"garden": setup},
        "now": NOW,
    })
}

/// The conversation of a project: the shop's has an owner's message from three
/// days ago, the lead's plan from an hour and a half ago and the owner's
/// answer; the garden's only its lead's greeting.
fn conversation(project: &str, say: &Say) -> Value {
    // The lead posts the card of the garden's setup once it is ready.
    let card = |at: u64| {
        json!({"id": 2, "at": at, "from": {"who": "lead", "hire": "m-17"},
               "text": "Here is how I'd run it.", "card": {"type": "setup", "project": "garden"}})
    };
    let messages = match project {
        "shop" => json!([
            {"id": 1, "at": NOW - 3 * DAY, "from": {"who": "owner"}, "text": "Plan the carts."},
            {"id": 2, "at": NOW - 90 * MINUTE, "from": {"who": "lead", "hire": "nora"},
             "text": "Checkout's next:",
             "card": {"type": "plan", "goal": "SHOP-2", "title": "Save carts between sessions", "version": "0.4",
                      "tasks": [{"title": "Store carts in the database", "hire": "dev"},
                                {"title": "Design the order confirmation", "hire": "designer"}]}},
            {"id": 3, "at": NOW - 80 * MINUTE, "from": {"who": "owner"}, "text": "Looks good."},
        ]),
        "garden" => {
            let mut said = vec![json!(
                {"id": 1, "at": NOW - 10 * MINUTE, "from": {"who": "lead", "hire": "m-17"},
                 "text": "Hi, I'm Mira. I'm reading garden to see how it builds, tests and ships."})];
            if matches!(say.setup, "ready" | "accepted") {
                said.push(card(NOW - 5 * MINUTE));
            }
            json!(said)
        }
        _ => json!([]),
    };
    json!({"messages": messages, "thinking": false})
}

fn answer(request: &Request, say: &Say) -> Response {
    match request {
        Request::Office => Response::ok(office(say)),
        Request::Conversation { project, .. } => Response::ok(conversation(project, say)),
        Request::Budget => Response::ok(json!({
            "limits": {"projects": {"shop": 4.0}, "ticket_usd": 2.5},
            "spent_24h": {"by_ticket": {"SHOP-1": 1.5, "shop/talk": 0.25}},
        })),
        Request::Diffstats { tickets } => Response::ok(
            tickets
                .iter()
                .map(|id| {
                    (
                        id.clone(),
                        "1 file changed, 30 insertions(+), 2 deletions(-)",
                    )
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
        Request::Events { .. } => Response::ok(json!([])),
        Request::SetupProject { path, name, .. } => Response::ok(json!({
            "project": name.clone().unwrap_or_else(|| path.rsplit('/').next().unwrap_or("x").into()),
            "lead": "m-17", "state": "reading",
        })),
        Request::Talk { text, .. } => Response::ok(json!({
            "id": 9, "at": 5, "from": {"who": "owner"}, "text": text,
        })),
        _ => Response::ok(Value::Null),
    }
}

fn fake_studiod(socket: &Path, seen: Arc<Mutex<Vec<Request>>>, say: Arc<Mutex<Say>>) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (seen, say) = (Arc::clone(&seen), Arc::clone(&say));
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let reply = answer(&request, &say.lock().unwrap());
                let mut out = serde_json::to_string(&reply).unwrap();
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
    say: Arc<Mutex<Say>>,
    _dir: tempfile::TempDir,
}

impl Web {
    async fn start() -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let say = Arc::new(Mutex::new(Say {
            checks: "passed",
            setup: "failed",
        }));
        fake_studiod(&socket, Arc::clone(&seen), Arc::clone(&say));
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
            say,
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

    async fn post(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.http
            .post(format!("{}{path}", self.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", &self.base)
            .form(form)
            .send()
            .await
            .unwrap()
    }

    /// What studiod was sent besides looks at the office, the conversations
    /// and the budget, since the last call.
    fn sent(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .drain(..)
            .filter(|request| {
                !matches!(
                    request,
                    Request::Office
                        | Request::Conversation { .. }
                        | Request::Budget
                        | Request::Diffstats { .. }
                )
            })
            .collect()
    }
}

/// The pages of the team, as the owner meets them.
const PAGES: [&str; 8] = [
    "/",
    "/leads/shop",
    "/leads/shop?look=SHOP-2",
    "/leads/garden",
    // Nobody leads the docs: the card that gives them a lead.
    "/leads/docs",
    "/office",
    "/new",
    "/studio",
];

/// The next patch of a stream that has `needle` in it.
async fn patch_with(reply: &mut reqwest::Response, needle: &str) -> String {
    let mut text = String::new();
    while !text.contains(needle) {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), reply.chunk())
            .await
            .expect("a patch within five seconds")
            .unwrap()
            .expect("a chunk");
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
}

// ------------------------------------------------------------ one stream

#[tokio::test]
async fn every_team_page_has_one_live_update_and_no_other_stream() {
    let web = Web::start().await;
    for path in PAGES {
        let html = web.page(path).await;
        // However the page opens the one stream, it is the page's stream of the
        // team pages, and the page opens it once.
        let streams: Vec<&str> = html.matches("stream").collect();
        assert_eq!(
            html.matches("/team/stream?at=").count(),
            1,
            "{path} opens the team stream once"
        );
        assert_eq!(streams.len(), 1, "{path} has no other stream");
    }
    // The routes of the region streams are gone.
    for gone in [
        "/leads/shop/stream",
        "/team/office/stream",
        "/team/office/peek",
    ] {
        assert_eq!(web.get(gone).await.status(), 404, "{gone}");
    }
    // The layout opens the stream of every page but the office, which opens it
    // again whenever the pick or the corner changes.
    assert!(has_id(&web.page("/leads/shop").await, "team-live"));
    let office = web.page("/office").await;
    assert!(!has_id(&office, "team-live"));
    assert!(by_id(&office, "office-live").attr_has("data-effect", "$sel; $turn;"));
}

#[tokio::test]
async fn the_stream_draws_the_office_as_the_owner_has_it() {
    let web = Web::start().await;
    let datastar = json!({"sel": "projects/shop", "turn": 2}).to_string();
    let mut reply = web
        .http
        .get(format!("{}/team/stream", web.base))
        .query(&[("at", "/office"), ("datastar", datastar.as_str())])
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("datastar-request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    let patch = patch_with(&mut reply, r#"id="office-peek""#).await;
    // The whole page, with the floor from the corner and the pick it was
    // opened with, and the card of the lead of what is picked.
    assert!(has_id(&patch, "team-sidebar"), "{patch}");
    assert_eq!(
        by_id(&patch, "hq-scene").attr("data-turn").as_deref(),
        Some("2")
    );
    assert!(patch.contains("data-picked"));
    let card = text_of(by_id(&patch, "office-peek").inner_html());
    assert!(card.contains("Talk to Nora"), "{card}");
    // The signals the page was opened with are not put back by a patch: they
    // are only ever set once, so the zoom and the hints stay.
    assert!(by_id(&patch, "office-page").has("data-signals__ifmissing"));
    assert!(!by_id(&patch, "office-page").has("data-signals"));
}

#[tokio::test]
async fn what_the_owner_types_or_opens_survives_a_patch() {
    let web = Web::start().await;
    // The field of the composer has no value of the page's own, so what is
    // typed in it is left as it is.
    let page = web.page("/leads/shop").await;
    let field = find(by_id(&page, "composer").inner_html(), "name", "text");
    assert!(!field.has("value") && field.has("data-bind:_draft"));
    // The tell field on the rail, and the note that sends a task back, stay
    // open as they were left.
    let tell = by_id(&page, "tell-SHOP-1");
    assert!(tell.attr_is("data-preserve-attr", "open"));
    assert!(tell.inner_html().contains(r#"name="text""#));
    let sheet = web.page("/leads/shop?look=SHOP-2").await;
    assert!(by_id(&sheet, "send-back").attr_is("data-preserve-attr", "open"));
    // So are the forms that give a project a lead: the card of a project that
    // has none, and the menu that changes the one it has.
    let docs = web.page("/leads/docs").await;
    for form in ["hire-lead", "pick-lead"] {
        assert!(by_id(&docs, form).has("data-ignore-morph"), "{form}");
    }
    assert!(by_id(&page, "change-lead-nora").has("data-ignore-morph"));
    // So is the form for a new project, whole.
    let new = web.page("/new").await;
    assert!(by_id(&new, "new-project").has("data-ignore-morph"));
    // And the part of the settings that is opened to hire someone.
    let studio = web.page("/studio").await;
    let hire = by_id(&studio, "hire");
    assert_eq!(hire.name, "details");
    assert!(hire.attr_is("data-preserve-attr", "open"));
    // What the setup card shows or hides by what the owner picked keeps its
    // style.
    web.say.lock().unwrap().setup = "ready";
    let garden = web.page("/leads/garden").await;
    for hidden in [".setup-file", ".setup-offer"] {
        let class = hidden.trim_start_matches('.');
        let toggled = matching(&garden, |t| {
            t.attr("class")
                .is_some_and(|c| c.split_whitespace().any(|c| c == class))
        });
        assert!(!toggled.is_empty(), "the card has {hidden}");
        assert!(
            toggled
                .iter()
                .all(|t| t.attr_is("data-preserve-attr", "style")),
            "{hidden}: {toggled:?}"
        );
    }
}

// -------------------------------------------------------------- composer

#[tokio::test]
async fn change_it_and_change_something_fill_the_composer_and_move_to_it() {
    let web = Web::start().await;
    // The composer is `#composer`, with a field named `text`.
    let page = web.page("/leads/shop").await;
    let composer = by_id(&page, "composer");
    assert_eq!(composer.name, "form");
    assert!(composer.inner_html().contains(r#"name="text""#));
    // The plan card's "Change it".
    let change_it = named(&page, "button")
        .into_iter()
        .find(|b| b.text() == "Change it")
        .expect("the plan has a Change it");
    let script = change_it.attr("data-on:click").unwrap();
    assert!(script.contains("#composer [name=text]"), "{script}");
    assert!(script.contains(r#""Change the plan: ""#), "{script}");
    assert!(script.contains("c.focus()"), "{script}");

    // The setup card's "Change something", once the setup is ready: the same
    // field, with the setup in mind.
    web.say.lock().unwrap().setup = "ready";
    let garden = web.page("/leads/garden").await;
    assert!(has_id(&garden, "composer"));
    let change_something = named(&garden, "button")
        .into_iter()
        .find(|b| b.text() == "Change something")
        .expect("a ready setup can be changed");
    let script = change_something.attr("data-on:click").unwrap();
    assert!(script.contains("#composer [name=text]"), "{script}");
    assert!(script.contains(r#""About the setup: ""#), "{script}");
    assert!(script.contains("c.focus()"), "{script}");
}

// ----------------------------------------------------------------- setup

#[tokio::test]
async fn a_failed_setup_shows_its_card_by_the_leads_name_and_try_again_asks_again() {
    let web = Web::start().await;
    let page = web.page("/leads/garden").await;
    let thread = by_id(&page, "talk-thread");
    let text = thread.text();
    // Mira, not the id of its hire.
    assert!(text.contains("Mira"), "{text}");
    assert!(text.contains("Could not set up garden"), "{text}");
    assert!(
        text.contains("The repository has no commits yet."),
        "{text}"
    );
    assert!(!text.contains("m-17") && !text.contains("M-17"), "{text}");
    let again = named(thread.inner_html(), "button")
        .into_iter()
        .find(|b| b.text() == "Try again")
        .expect("a failed setup can be tried again");
    assert_eq!(
        again.attr("data-on:click").as_deref(),
        Some("@post('/leads/garden/setup/retry')")
    );

    // Trying again asks studiod to set the project up again, in its folder,
    // with the lead it has.
    web.sent();
    let reply = web.post("/leads/garden/setup/retry", &[]).await;
    assert_eq!(reply.status(), 200);
    assert!(reply.text().await.unwrap().contains("Trying garden again."));
    assert_eq!(
        web.sent(),
        [Request::SetupProject {
            path: "/work/garden".into(),
            name: Some("garden".into()),
            lead: Some("m-17".into()),
            new_lead: None,
        }]
    );

    // While the lead reads it, the card says who reads; once it tries the
    // checks, that it does.
    for (state, words) in [
        ("reading", "Mira is reading garden…"),
        ("checking", "Trying the checks…"),
    ] {
        web.say.lock().unwrap().setup = state;
        let page = web.page("/leads/garden").await;
        let text = by_id(&page, "talk-thread").text();
        assert!(text.contains(words), "{state}: {text}");
        assert!(!text.contains("m-17"), "{state}: {text}");
    }
    // A project that was never set up through its lead has no card, and a
    // lead that is not being asked anything has none to show.
    let shop = web.page("/leads/shop").await;
    assert!(!by_id(&shop, "talk-thread").text().contains("is reading"));
}

// ----------------------------------------------------------------- faces

#[tokio::test]
async fn every_face_on_every_page_is_a_real_one() {
    let web = Web::start().await;
    for path in PAGES {
        let html = web.page(path).await;
        let faces: Vec<_> = named(&html, "svg")
            .into_iter()
            .filter(|svg| svg.has("data-face"))
            .collect();
        assert!(!faces.is_empty(), "{path} has faces");
        // Head, hair, eyes and a smile, in a circle, not initials.
        for face in &faces {
            let inner = face.inner_html();
            for part in [
                "class=\"sk\"",
                "class=\"hr\"",
                "class=\"ey\"",
                "class=\"mo\"",
            ] {
                assert!(inner.contains(part), "{path}: {part} in {inner}");
            }
        }
        let initials = matching(&html, |t| {
            t.attr("class")
                .is_some_and(|c| c.split_whitespace().next() == Some("av"))
        });
        assert!(
            initials.is_empty(),
            "{path} still has initials: {initials:?}"
        );
    }
    // A hire is the same face wherever it is, by its id: its skin and hair.
    let shop = web.page("/leads/shop").await;
    let looks = |html: &str| -> Option<String> {
        let face = with(html, "data-face", "nora").into_iter().next()?;
        let class = face.attr("class")?;
        let looks: Vec<&str> = class
            .split_whitespace()
            .filter(|c| c.starts_with("s-") || c.starts_with("h-"))
            .collect();
        Some(looks.join(" "))
    };
    let in_sidebar = looks(by_id(&shop, "team-sidebar").inner_html());
    let in_header = looks(by_id(&shop, "talk-head").inner_html());
    assert!(in_sidebar.is_some() && in_sidebar == in_header);
}

// ----------------------------------------------------------------- times

/// Whether `text` is a clock time such as "9:15".
fn is_clock(text: &str) -> bool {
    text.split_once(':').is_some_and(|(hours, minutes)| {
        (1..=2).contains(&hours.len())
            && minutes.len() == 2
            && hours
                .chars()
                .chain(minutes.chars())
                .all(|c| c.is_ascii_digit())
    })
}

#[tokio::test]
async fn times_in_the_conversation_are_clock_times_today_and_dates_before() {
    let web = Web::start().await;
    let page = web.page("/leads/shop").await;
    let thread = by_id(&page, "talk-thread");
    let stamps: Vec<(String, String)> = named(thread.inner_html(), "time")
        .iter()
        .map(|t| (t.text(), t.attr("title").unwrap()))
        .collect();
    assert!(!stamps.is_empty());
    let clocks = stamps.iter().filter(|(shown, _)| is_clock(shown)).count();
    let dates: Vec<&String> = stamps
        .iter()
        .map(|(shown, _)| shown)
        .filter(|shown| !is_clock(shown))
        .collect();
    // The plan and the answer are from today, so are clock times; the owner's
    // first message was three days ago, so it is a date, never "3d ago".
    assert!(clocks >= 2, "{stamps:?}");
    assert_eq!(dates.len(), 1, "{stamps:?}");
    assert!(dates[0].ends_with(" Nov"), "{stamps:?}");
    assert!(
        !stamps.iter().any(|(shown, _)| shown.contains("ago")),
        "{stamps:?}"
    );
    // The tooltip says how long ago it was, and when.
    let old = stamps.iter().find(|(shown, _)| !is_clock(shown)).unwrap();
    assert!(old.1.starts_with("3d ago · "), "{old:?}");
    let recent = stamps.iter().find(|(shown, _)| is_clock(shown)).unwrap();
    assert!(recent.1.contains("ago"), "{recent:?}");

    // HQ gives the time of a lead's line the same way.
    let hq = web.page("/").await;
    let line = text_of(find(&hq, "data-lead", "shop").inner_html());
    assert!(
        line.split_whitespace().any(is_clock),
        "the standup has a clock time: {line}"
    );
    // The standup says the same time in the conversation: when it was
    // written, though the thread stands it at the start of the day.
    let said = |text: &str| {
        text.split_whitespace()
            .find(|word| is_clock(word))
            .map(String::from)
    };
    let first = by_id(&page, "live-stats").text();
    assert!(
        said(&line).is_some() && said(&first) == said(&line),
        "{first} / {line}"
    );
}

// ---------------------------------------------------------------- routes

/// Every address a page links to or calls, as (method, path): its links, and
/// the `@get('…')` and `@post('…')` of its scripts.
fn calls(html: &str) -> BTreeSet<(&'static str, String)> {
    let path = |address: &str| {
        address
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let mut found = BTreeSet::new();
    for a in named(html, "a") {
        if let Some(href) = a.attr("href").filter(|href| href.starts_with('/')) {
            found.insert(("GET", path(&href)));
        }
    }
    for (opening, method) in [("@get('", "GET"), ("@post('", "POST")] {
        let mut rest = html;
        while let Some(at) = rest.find(opening) {
            rest = &rest[at + opening.len()..];
            let Some(end) = rest.find('\'') else { break };
            found.insert((method, path(&rest[..end])));
        }
    }
    found
}

#[tokio::test]
async fn every_link_and_call_of_every_team_page_and_the_floor_has_a_route() {
    let web = Web::start().await;
    let mut pages: Vec<(String, String)> = Vec::new();
    for path in PAGES {
        pages.push((path.to_string(), web.page(path).await));
    }
    // What a pick on the floor draws, and the other states of the pages.
    let datastar = json!({"sel": "projects/shop", "turn": 1}).to_string();
    let scene = web
        .http
        .get(format!("{}/team/office/scene", web.base))
        .query(&[("datastar", datastar.as_str())])
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("datastar-request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(scene.status(), 200);
    pages.push(("the floor, picked".into(), scene.text().await.unwrap()));
    web.say.lock().unwrap().setup = "ready";
    pages.push((
        "/leads/garden, ready".into(),
        web.page("/leads/garden").await,
    ));
    web.say.lock().unwrap().checks = "failed";
    pages.push((
        "/leads/shop, failed checks".into(),
        web.page("/leads/shop").await,
    ));

    let mut checked = BTreeSet::new();
    for (name, html) in &pages {
        for (method, path) in calls(html) {
            if path.is_empty() || !checked.insert((method, path.clone())) {
                continue;
            }
            let reply = match method {
                "GET" => web.get(&path).await,
                _ => web.post(&path, &[]).await,
            };
            let status = reply.status().as_u16();
            assert!(
                status != 404 && status != 405,
                "{name} calls {method} {path}, which has no route ({status})"
            );
        }
    }
    // The old office's own panel is not called by anything of the team pages.
    for (name, html) in &pages {
        let at = html.find("/office/panel").unwrap_or(html.len());
        let near = &html[at.saturating_sub(160)..(at + 60).min(html.len())];
        assert!(
            !html.contains("/office/panel"),
            "{name} calls the old panel: {near}"
        );
        assert!(!html.contains("/hq\""), "{name} links to /hq");
    }
    // The pages worth checking were all read.
    assert!(checked.len() > 15, "{checked:?}");
}

// ------------------------------------------------- what the engine gives

#[tokio::test]
async fn hq_and_the_conversation_show_what_the_engine_provides() {
    let web = Web::start().await;
    let hq = web.page("/").await;
    let shop = web.page("/leads/shop").await;
    let thread = by_id(&shop, "talk-thread").text();
    let hq_line = text_of(find(&hq, "data-lead", "shop").inner_html());

    // The plain question the approver wrote is the question's title.
    assert!(hq_line.contains("Send the launch email?"), "{hq_line}");
    assert!(thread.contains("Send the launch email?"), "{thread}");
    assert!(!thread.contains("Run a command"), "{thread}");

    // What the lead says: its standup, as its line on HQ and as its words with
    // the day's numbers in the conversation.
    let standup = "The logo's done. Two things wait for you.";
    assert!(hq_line.contains(standup), "{hq_line}");
    assert!(thread.contains(standup), "{thread}");

    // What done work came to, in a line: of the goal, from its tasks, and of
    // the task that is ready.
    let goal = "Stored carts in the database. Brought the cart back when a shopper signs in.";
    assert!(hq_line.contains(goal), "{hq_line}");
    assert!(
        hq_line.contains("Added a discount field to the cart."),
        "{hq_line}"
    );
    assert!(thread.contains(goal), "{thread}");
    assert!(
        thread.contains("Added a discount field to the cart."),
        "{thread}"
    );
    // And of each task of a plan once it is done.
    let plan = by_id(&shop, "m-2").text();
    assert!(plan.contains("Stored carts in the database."), "{plan}");

    // The checks of the goal that is ready to ship, in each of their states.
    for (checks, words) in [
        ("passed", "checks passed"),
        ("failed", "checks failed: cargo test: 2 failed"),
        ("running", "checks running"),
    ] {
        web.say.lock().unwrap().checks = checks;
        let hq = web.page("/").await;
        let shop = web.page("/leads/shop").await;
        let hq_line = text_of(find(&hq, "data-lead", "shop").inner_html());
        assert!(hq_line.contains(words), "{checks}: {hq_line}");
        assert!(
            by_id(&shop, "talk-thread").text().contains(words),
            "{checks}"
        );
        let ship = by_id(&shop, "ship-SHOP-2");
        let button = named(ship.inner_html(), "button")
            .into_iter()
            .next()
            .unwrap()
            .text();
        // A goal whose checks failed is still shipped, and the page says so.
        assert_eq!(
            button.trim(),
            if checks == "failed" {
                "Ship anyway"
            } else {
                "Ship"
            },
            "{checks}"
        );
    }
}
