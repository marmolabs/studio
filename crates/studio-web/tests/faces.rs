//! The faces and the sidebar's phone header against a stand-in studiod: each
//! hire has a face of its own that stays the same everywhere, and the header
//! keeps one link for each lead, with the count on it, and moves what was spent
//! to the foot of HQ.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, has_id, matching, named, with};

const TOKEN: &str = "0123abcd";

/// The hires that lead a project each, and the project they lead.
const LEADS: [(&str, &str); 8] = [
    ("nora", "shop"),
    ("sam", "editor"),
    ("ines", "admin"),
    ("theo", "docs"),
    ("mira", "garden"),
    ("leo", "deck"),
    ("ada", "maps"),
    ("kit", "bank"),
];

/// The stand-in's office: eight projects with a lead each; the shop has a
/// question waiting.
fn office() -> Value {
    let team: Vec<Value> = LEADS
        .iter()
        .map(|(id, _)| json!({"id": id, "role": "lead", "model": "opus"}))
        .collect();
    let projects: Vec<Value> = LEADS
        .iter()
        .map(|(id, name)| json!({"name": name, "lead": id}))
        .collect();
    let names: Vec<&str> = LEADS.iter().map(|(_, name)| *name).collect();
    let hires: Vec<&str> = LEADS.iter().map(|(id, _)| *id).collect();
    json!({
        "status": {
            "projects": names, "hires": hires, "team": team,
            "daily_usd": 10.0, "spent_24h": 6.59, "max_runs": 3,
        },
        "tickets": [{
            "id": "SHOP-1", "project": "shop", "title": "Add a cart drawer", "goal": "",
            "hire": "nora", "state": {"asking": {"ask": "a1"}}, "attempts": 1,
            "base_branch": "main", "created_at": 0,
        }],
        "asks": [{
            "id": "a1", "ticket": "SHOP-1", "hire": "nora", "kind": "permission", "tool": "Bash",
            "input": {"command": "./send.sh"}, "reasons": [], "title": "Send the launch email?",
        }],
        "events": [], "spend": {}, "merged": [], "sizes": {}, "today": {}, "judges": {},
        "projects": projects,
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
}

/// The classes of the sidebar's face of `seed` that say how it looks: its skin,
/// its hair tone and its hair style.
fn looks(html: &str, seed: &str) -> String {
    let class = find(by_id(html, "team-sidebar").inner_html(), "data-face", seed)
        .attr("class")
        .unwrap();
    let mut parts: Vec<&str> = class
        .split_whitespace()
        .filter(|c| c.starts_with("s-") || c.starts_with("h-") || c.starts_with("hs-"))
        .collect();
    parts.sort_unstable();
    parts.join(" ")
}

#[tokio::test]
async fn each_hire_has_a_face_of_its_own_that_looks_the_same_on_every_page() {
    let web = Web::start().await;
    let hq = web.page("/").await;
    let mut seen = BTreeSet::new();
    for (id, _) in LEADS {
        let look = looks(&hq, id);
        // A skin, a hair tone and a hair style.
        assert_eq!(look.split(' ').count(), 3, "{id}: {look}");
        seen.insert(look);
    }
    // Eight people, and no more than two of them look alike.
    assert!(seen.len() >= 6, "{seen:?}");
    // Hair is short, long or in a bun, and all three turn up.
    let styles: BTreeSet<&str> = seen
        .iter()
        .flat_map(|look| look.split(' '))
        .filter(|c| c.starts_with("hs-"))
        .collect();
    assert_eq!(styles.len(), 3, "{styles:?}");
    // The same people on another page.
    let shop = web.page("/leads/shop").await;
    for (id, _) in LEADS {
        assert_eq!(looks(&shop, id), looks(&hq, id), "{id}");
    }
    // The face is a drawing of a head with hair, shoulders, eyes and a smile.
    let nora = find(by_id(&hq, "team-sidebar").inner_html(), "data-face", "nora");
    let drawing = nora.inner_html();
    for part in ["sh", "nk", "sk", "hr", "ey", "mo"] {
        assert!(
            drawing.contains(&format!(r#"class="{part}""#)),
            "{part}: {drawing}"
        );
    }
}

#[tokio::test]
async fn the_header_has_one_link_for_each_lead_with_its_count_on_it() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let sidebar = by_id(&html, "team-sidebar");
    let links = named(sidebar.inner_html(), "a");
    for (_, project) in LEADS {
        let to = format!("/leads/{project}");
        let these: Vec<_> = links.iter().filter(|a| a.attr_is("href", &to)).collect();
        assert_eq!(these.len(), 1, "{to}");
        // The face is inside the link, and the link has words for a screen
        // reader.
        let inner = these[0].inner_html();
        assert!(inner.contains("data-face"), "{to}");
        assert!(these[0].text().contains(project), "{to}");
    }
    // Where the owner is is marked on the lead's own link, and only there.
    let marked = with(&html, "aria-current", "page");
    assert_eq!(marked.len(), 1, "{marked:?}");
    assert!(marked[0].attr_is("href", "/leads/shop"));
    // The shop's question counts on its link; the others have no count.
    let count = |to: &str| {
        let link = find(sidebar.inner_html(), "href", to);
        matching(link.inner_html(), |t| {
            t.attr("class")
                .is_some_and(|c| c.split_whitespace().any(|c| c == "tm-count"))
        })
        .len()
    };
    assert_eq!(count("/leads/shop"), 1);
    assert_eq!(count("/leads/editor"), 0);
    // The office and the settings are links of the header too, and so is HQ,
    // with its count.
    for to in ["/office", "/studio", "/new"] {
        assert!(
            links.iter().any(|a| a.attr_is("href", to)),
            "no link to {to}"
        );
    }
    let hq: Vec<_> = links
        .iter()
        .filter(|a| a.attr_is("href", "/") && a.text().starts_with("HQ"))
        .collect();
    assert_eq!(hq.len(), 1);
    assert!(hq[0].text().ends_with(" 1"), "{}", hq[0].text());
}

#[tokio::test]
async fn what_was_spent_sits_at_the_foot_of_hq_for_a_phone() {
    let web = Web::start().await;
    let hq = web.page("/").await;
    // HQ's foot has it, hidden except on a phone.
    let foot = by_id(&hq, "team-spend");
    assert!(
        foot.text().contains("Spent today $6.59 of $10"),
        "{}",
        foot.text()
    );
    assert!(foot.attr("class").unwrap().contains("max-[760px]:block"));
    assert!(
        foot.attr("class")
            .unwrap()
            .split(' ')
            .any(|c| c == "hidden")
    );
    // The sidebar keeps its own for a desktop.
    assert!(
        by_id(&hq, "team-sidebar")
            .text()
            .contains("Spent today $6.59 of $10")
    );
    // No other page has the foot.
    for path in ["/leads/shop", "/studio", "/office", "/new"] {
        let html = web.page(path).await;
        assert!(!has_id(&html, "team-spend"), "{path}");
    }
}
