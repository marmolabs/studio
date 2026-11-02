//! The team frame and HQ against a stand-in studiod: what the sidebar and the
//! round-up say, what their buttons post, and the stream that keeps them up to
//! date.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, has_id, matching, named};

const TOKEN: &str = "0123abcd";

fn ticket(id: &str, project: &str, title: &str, state: Value, extra: Value) -> Value {
    let mut fields = json!({
        "id": id, "project": project, "title": title, "goal": "", "hire": "dev",
        "state": state, "attempts": 1, "base_branch": "main", "created_at": 0,
    });
    fields
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    fields
}

/// The stand-in's office: the shop, led by Nora, has two questions, a goal
/// whose checks failed and a task that is ready; the docs, led by Theo, have a
/// release that waits; the api has no lead and nothing going on. The api is
/// listed first, so HQ has to put the others before it.
fn office() -> Value {
    json!({
        "status": {
            "projects": ["api", "shop", "docs"], "hires": ["nora", "theo", "dev"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "theo", "name": "Theo", "role": "lead", "model": "opus"},
                {"id": "dev", "role": "engineer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.59, "max_runs": 3,
        },
        "tickets": [
            ticket("SHOP-1", "shop", "Add a cart drawer", json!({"asking": {"ask": "a1"}}), json!({})),
            ticket("SHOP-2", "shop", "Plan: checkout", json!("landed"),
                json!({"kind": "plan", "goal": "Checkout flow\nMore words."})),
            ticket("SHOP-3", "shop", "Pay form", json!("landed"), json!({"parent": "SHOP-2"})),
            ticket("SHOP-4", "shop", "Save carts", json!("ready"), json!({})),
        ],
        "asks": [
            {"id": "a1", "ticket": "SHOP-1", "hire": "dev", "kind": "permission", "tool": "Bash",
             "input": {"command": "./send.sh"}, "reasons": ["Lead: sends an email (project/shop/protect-mail)"],
             "title": "Send the launch email?"},
            {"id": "a2", "ticket": "SHOP-1", "hire": "dev", "kind": "question",
             "tool": "AskUserQuestion", "input": [{"question": "Which palette?", "options": ["A", "B"]}],
             "reasons": []},
        ],
        "events": [], "spend": {}, "merged": [], "today": {}, "judges": {},
        "sizes": {"SHOP-4": [12, 3]},
        "ship_checks": {"SHOP-2": {"state": "failed", "summary": "1 failed", "at": 1}},
        "standups": {"shop": {"at": 1_700_000_000_000_u64, "text": "The logo is done. Cards went back."}},
        "projects": [
            {"name": "api"},
            {"name": "shop", "lead": "nora"},
            {"name": "docs", "lead": "theo", "release": {
                "project": "docs", "version": "1.2", "state": "waiting",
                "steps": [{"name": "Final checks", "done": true}], "started_at": 0}},
        ],
        "now": 1_700_000_100_000_u64,
    })
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
                let answer = match request {
                    Request::Office => Response::ok(office()),
                    _ => Response::ok(Value::Null),
                };
                let mut out = serde_json::to_string(&answer).unwrap();
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
}

/// The projects HQ has a line for, in the order it shows them.
fn lines(html: &str) -> Vec<String> {
    with_attr(html, "data-lead")
        .iter()
        .filter_map(|t| t.attr("data-lead"))
        .collect()
}

fn with_attr<'a>(html: &'a str, attr: &str) -> Vec<probe::Tag<'a>> {
    matching(html, |t| t.has(attr))
}

/// The HQ line of `project`.
fn line<'a>(html: &'a str, project: &str) -> probe::Tag<'a> {
    find(html, "data-lead", project)
}

#[tokio::test]
async fn hq_greets_dates_counts_and_puts_leads_with_something_for_the_owner_first() {
    let web = Web::start().await;
    let html = web.page("/").await;
    // A greeting for the time of day, the date and what waits: two asks, a
    // goal and a task to ship, and a release.
    let h1 = named(&html, "h1");
    assert_eq!(h1.len(), 1, "{html}");
    assert!(
        ["Good morning", "Good afternoon", "Good evening"].contains(&h1[0].text().as_str()),
        "{html}"
    );
    let main = by_id(&html, "team-main").text();
    assert!(main.contains("5 need you"), "{main}");
    // 2023-11-14 22:15 UTC is the 14th or the 15th wherever the machine is.
    assert!(
        main.contains("14 November") || main.contains("15 November"),
        "{main}"
    );
    // The api waits for nothing, so it comes last, though it is first in the
    // studio.
    assert_eq!(lines(&html), ["shop", "docs", "api"]);
    // The lead's own words, else the facts in plain words.
    assert!(
        line(&html, "shop")
            .text()
            .contains("The logo is done. Cards went back.")
    );
    assert!(
        line(&html, "docs").text().contains("1 waits for you."),
        "{}",
        line(&html, "docs").text()
    );
    // Nobody leads the api: the facts, and a row that says so, with a link to
    // where it is given a lead.
    let api = line(&html, "api");
    assert!(api.text().contains("Idle."), "{}", api.text());
    assert!(api.text().contains("No lead yet"), "{}", api.text());
    assert!(
        named(api.inner_html(), "a")
            .iter()
            .any(|a| a.attr_is("href", "/leads/api") && a.text() == "Give api a lead")
    );
    // The footer: the last day in a line.
    assert!(
        main.contains("The last 24 hours: 0 tasks done, 0 sent back, $6.59 of $10 spent"),
        "{main}"
    );
}

#[tokio::test]
async fn the_cards_under_a_lead_wait_with_their_buttons() {
    let web = Web::start().await;
    let html = web.page("/").await;
    let shop = line(&html, "shop");
    let inner = shop.inner_html();
    let cards: Vec<String> = with(inner, "data-card", "ask")
        .iter()
        .map(|c| c.text())
        .collect();
    // The lead's question in its own words; one without says what it is.
    assert!(cards[0].starts_with("Send the launch email?"), "{cards:?}");
    assert!(cards[0].contains("Sends an email"), "{cards:?}");
    assert!(
        !cards[0].contains("Lead:") && !cards[0].contains("protect-mail"),
        "{cards:?}"
    );
    assert!(cards[1].starts_with("Which palette?"), "{cards:?}");
    // An allow-or-deny call is decided where it is, with the ask's own form;
    // a question the agent asks is answered on its task's page.
    let allow = find(inner, "value", "approve");
    assert_eq!(allow.text(), "Allow");
    let deny = find(inner, "value", "deny");
    assert_eq!(deny.text(), "Deny");
    assert!(
        html.contains("@post(&#39;/asks/a1&#39;, {contentType: &#39;form&#39;})")
            || html.contains("@post('/asks/a1', {contentType: 'form'})"),
        "{html}"
    );
    let answer = find(inner, "href", "/tickets/SHOP-1");
    assert!(["Answer", "Details"].contains(&answer.text().as_str()));

    // Work to ship: size, how the checks went, Ship and a closer look.
    let ships: Vec<probe::Tag> = with(inner, "data-card", "ship");
    assert_eq!(ships.len(), 2, "{inner}");
    let goal = ships[0].text();
    assert!(goal.starts_with("Checkout flow is ready to ship"), "{goal}");
    assert!(goal.contains("checks failed"), "{goal}");
    let task = ships[1].text();
    assert!(task.contains("Save carts is ready to ship"), "{task}");
    assert!(task.contains("+12 −3"), "{task}");
    assert!(task.contains("reviewed"), "{task}");
    let look = find(inner, "href", "/leads/shop?look=SHOP-2");
    assert_eq!(look.text(), "Look closer");
    find(inner, "href", "/leads/shop?look=SHOP-4");
    // Shipping a goal whose checks failed says so first; the other does not.
    let ship = |card: &probe::Tag| {
        let buttons = matching(card.inner_html(), |t| {
            t.name == "button" && t.has("data-on:click")
        });
        buttons[0].attr("data-on:click").unwrap()
    };
    let goal_ship = ship(&ships[0]);
    assert!(goal_ship.contains("confirm("), "{goal_ship}");
    assert!(
        goal_ship.contains("@post('/tickets/SHOP-3/merge')"),
        "{goal_ship}"
    );
    assert_eq!(ship(&ships[1]), "@post('/tickets/SHOP-4/land')");

    // A release that waits: Publish and Not yet, to the release routes.
    let docs = line(&html, "docs");
    let release = with(docs.inner_html(), "data-card", "release").remove(0);
    let text = release.text();
    assert!(text.starts_with("Publish docs v1.2?"), "{text}");
    let forms = named(release.inner_html(), "form");
    let posts: Vec<String> = forms
        .iter()
        .filter_map(|f| f.attr("data-on:submit"))
        .collect();
    assert!(posts[0].contains("/office/releases/approve"), "{posts:?}");
    assert!(posts[1].contains("/office/releases/cancel"), "{posts:?}");
    let project = find(release.inner_html(), "name", "project");
    assert_eq!(project.attr("value").as_deref(), Some("docs"));
    assert!(
        text.contains("Publish") && text.contains("Not yet"),
        "{text}"
    );
    // The lead without cards has none.
    assert!(with(line(&html, "api").inner_html(), "data-card", "ask").is_empty());
}

fn with<'a>(html: &'a str, attr: &str, value: &str) -> Vec<probe::Tag<'a>> {
    probe::with(html, attr, value)
}

#[tokio::test]
async fn the_sidebar_has_faces_the_floor_and_what_was_spent() {
    let web = Web::start().await;
    let html = web.page("/").await;
    let sidebar = by_id(&html, "team-sidebar");
    let inner = sidebar.inner_html();
    // A face for each lead, drawn from its hire id: the floor's skin and hair
    // and, for a lead, the colour of its project.
    let nora = find(inner, "data-face", "nora");
    assert_eq!(nora.name, "svg");
    let class = nora.attr("class").unwrap();
    for part in ["fc", "r-lead", "pj-1"] {
        assert!(class.split(' ').any(|c| c == part), "{class}");
    }
    assert!(class.contains(" s-") && class.contains(" h-"), "{class}");
    assert_eq!(nora.attr("width").as_deref(), Some("40"));
    // The same hire looks the same everywhere.
    let again = web.page("/").await;
    assert_eq!(
        find(
            by_id(&again, "team-sidebar").inner_html(),
            "data-face",
            "nora"
        )
        .attr("class"),
        nora.attr("class")
    );
    // The floor at thumbnail size: its own drawing, not an image, and a way
    // into the office.
    let office = find(inner, "href", "/office");
    assert!(
        office.inner_html().contains(r#"<svg class="iso""#),
        "{inner}"
    );
    assert!(named(office.inner_html(), "img").is_empty());
    assert!(office.text().contains("The office"));
    // The gear leads to the settings.
    let gear = find(inner, "aria-label", "Studio settings");
    assert_eq!(gear.attr("href").as_deref(), Some("/studio"));
    // A bar for each project in its colour, and what was spent against the
    // limit.
    let bars = matching(inner, |t| t.attr_is("role", "progressbar"));
    assert_eq!(bars.len(), 4, "three leads and the money");
    assert!(sidebar.text().contains("Spent today $6.59 of $10"));
    // Waiting counts: shop has four things, docs one.
    let count = |to: &str| find(inner, "href", to).text();
    assert!(
        count("/leads/shop").ends_with(" 4"),
        "{}",
        count("/leads/shop")
    );
    assert!(count("/leads/docs").ends_with(" 1"));
    let hq = matching(inner, |t| {
        t.name == "a" && t.attr_is("href", "/") && t.text().starts_with("HQ")
    });
    assert!(hq[0].text().ends_with(" 5"), "{}", hq[0].text());
    // The floor's duplicate ids don't matter, but the main area and the rail
    // have places of their own.
    assert!(has_id(&html, "team-main") && has_id(&html, "team-rail"));
}

#[tokio::test]
async fn every_team_page_opens_the_one_stream_naming_where_it_is() {
    let web = Web::start().await;
    for path in ["/", "/leads/shop", "/new", "/studio"] {
        let html = web.page(path).await;
        let live = by_id(&html, "team-live");
        let open = live.attr("data-init").unwrap();
        assert!(open.contains("/team/stream?at="), "{path}: {open}");
        assert!(open.contains("location.pathname"), "{path}: {open}");
        // Outside the page that is patched, so a patch never replaces it.
        assert!(!by_id(&html, "team").inner_html().contains("team-live"));
    }
    // The office opens the same stream itself, again whenever the pick or the
    // corner of the floor changes, so the layout does not open one for it.
    let office = web.page("/office").await;
    assert!(!has_id(&office, "team-live"));
    let open = by_id(&office, "office-live").attr("data-effect").unwrap();
    assert!(open.contains("$sel; $turn;"), "{open}");
    assert!(open.contains("/team/stream?at="), "{open}");
}

#[tokio::test]
async fn the_stream_sends_the_page_it_was_opened_from() {
    let web = Web::start().await;
    let mut reply = web.get("/team/stream?at=%2F").await;
    assert_eq!(reply.status(), 200);
    let mut text = String::new();
    // The patch arrives in pieces: read until the page's end.
    while !text.contains("need you") {
        let chunk = tokio::time::timeout(std::time::Duration::from_secs(10), reply.chunk())
            .await
            .expect("a patch within ten seconds")
            .unwrap()
            .expect("a chunk");
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    // A Datastar patch with the page's frame, sidebar and HQ.
    assert!(text.contains("datastar-patch-elements"), "{text}");
    assert!(text.contains(r#"id="team""#), "{text}");
    assert!(text.contains("team-sidebar"), "{text}");
    assert!(text.contains("need you"), "{text}");
    // The conversation of a project the studio doesn't have is nothing.
    let mut none = web.get("/team/stream?at=%2Fleads%2Fgarden").await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(2500), none.chunk())
            .await
            .is_err(),
        "a patch for no page"
    );
}

#[tokio::test]
async fn old_home_addresses_lead_to_where_they_are_now() {
    let web = Web::start().await;
    for (old, now) in [
        ("/?tab=money", "/studio"),
        ("/?sel=projects/shop", "/leads/shop"),
        ("/?sel=goals/SHOP-2", "/leads/shop?look=SHOP-2"),
        ("/?sel=tickets/SHOP-1", "/tickets/SHOP-1"),
        ("/asks", "/"),
        ("/budget", "/studio"),
        ("/office/classic", "/office"),
        ("/office/classic?tab=money", "/studio"),
        ("/office/classic?sel=projects/shop", "/leads/shop"),
    ] {
        let reply = web.get(old).await;
        assert_eq!(reply.status(), 308, "{old}");
        assert_eq!(reply.headers()["location"], now, "{old}");
    }
}
