//! The web UI against a stand-in studiod that answers from fixed data.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, Request, Response};

/// The files `SaveFile` was asked to write, as (name, text).
static SAVED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, has_id, matching, named, shows, tags, text_of, with};

const TOKEN: &str = "0123abcd";

fn ui_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn reply(request: &Request, now: u64) -> Value {
    match request {
        Request::Status => json!({
            "asks": 1, "running": 2, "tickets": {"queued": 1, "working": 2}, "daily_usd": 10.0, "spent_24h": 0.25,
            "projects": ["game"], "hires": ["dev"], "max_runs": 3,
            "team": [{"id": "dev", "role": "engineer", "model": "sonnet"},
                     {"id": "approver", "role": "approver", "model": "haiku"}],
        }),
        Request::RecentEvents { kinds, .. } => {
            let at = |minutes_ago: u64| now - minutes_ago * 60_000;
            let events = [
                json!({"seq": 1, "ticket_id": "GAME-4", "at_ms": at(125), "event": {"type": "landed"}}),
                json!({"seq": 2, "ticket_id": "GAME-1", "at_ms": at(2),
                       "event": {"type": "ask_opened", "ask": "ASK-1"}}),
            ];
            let wanted =
                |e: &Value| kinds.is_empty() || kinds.iter().any(|k| e["event"]["type"] == *k);
            json!(events.into_iter().filter(wanted).collect::<Vec<_>>())
        }
        Request::Tickets => json!([{
            "id": "GAME-1", "project": "game", "title": "Add a <b>pause</b> menu",
            "goal": "", "hire": "dev", "state": {"held": {"reason": "budget"}},
            "attempts": 1, "base_branch": "master", "created_at": 0,
        }, {
            "id": "GAME-2", "project": "game", "title": "Add a title screen",
            "goal": "", "hire": "dev", "state": "queued",
            "attempts": 0, "base_branch": "master", "created_at": 1,
        }, {
            "id": "GAME-3", "project": "game", "title": "Plan: ship the menus",
            "goal": "Ship the <i>menus</i>", "hire": "producer", "state": "working",
            "attempts": 1, "base_branch": "master", "created_at": 2, "kind": "plan",
        }, {
            "id": "GAME-4", "project": "game", "title": "Menu art",
            "goal": "", "hire": "dev", "state": "landed",
            "attempts": 1, "base_branch": "master", "created_at": 3, "parent": "GAME-3",
        }, {
            "id": "GAME-5", "project": "game", "title": "Menu sounds",
            "goal": "", "hire": "dev", "state": "ready",
            "attempts": 1, "base_branch": "master", "created_at": 4, "parent": "GAME-3",
        }, {
            "id": "GAME-9", "project": "game", "title": "Menu music",
            "goal": "", "hire": "dev", "state": "cancelled",
            "attempts": 1, "base_branch": "master", "created_at": 5, "parent": "GAME-3",
        }, {
            "id": "GAME-7", "project": "game", "title": "Plan: add saves",
            "goal": "Add saves", "hire": "producer", "state": "queued",
            "attempts": 0, "base_branch": "master", "created_at": 6, "kind": "plan",
        }, {
            "id": "GAME-8", "project": "game", "title": "Plan: add audio",
            "goal": "Add audio", "hire": "producer", "state": "landed",
            "attempts": 0, "base_branch": "master", "created_at": 7, "kind": "plan",
        }, {
            "id": "GAME-10", "project": "game", "title": "Audio engine",
            "goal": "Add audio", "hire": "dev", "state": "landed",
            "attempts": 0, "base_branch": "master", "created_at": 8, "parent": "GAME-8",
        }]),
        Request::MergedTickets => json!(["GAME-10"]),
        Request::CreateGoal { .. } => json!({
            "id": "GAME-6", "project": "game", "title": "Plan: x",
            "goal": "x", "hire": "producer", "state": "queued",
            "attempts": 0, "base_branch": "master", "created_at": 5, "kind": "plan",
        }),
        Request::Events {
            ticket: Some(_), ..
        } => {
            let at = |minutes_ago: u64| now - minutes_ago * 60_000;
            json!([
                {"seq": 1, "ticket_id": "GAME-1", "at_ms": at(10),
                 "event": {"type": "claimed"}},
                {"seq": 2, "ticket_id": "GAME-1", "at_ms": at(3),
                 "event": {"type": "checks_failed", "detail": "<script>boom</script>"}},
                {"seq": 3, "ticket_id": "GAME-1", "at_ms": at(1),
                 "event": {"type": "held", "reason": "budget"}},
            ])
        }
        Request::Activity { .. } => {
            let at = |minutes_ago: u64| now - minutes_ago * 60_000;
            json!([
                {"seq": 1, "ticket_id": "GAME-1", "at_ms": at(30), "event": {"type": "claimed"}},
                {"seq": 2, "ticket_id": "GAME-3", "at_ms": at(20), "event": {"type": "claimed"}},
                {"seq": 3, "ticket_id": "GAME-1", "at_ms": at(5),
                 "event": {"type": "held", "reason": "budget"}},
            ])
        }
        Request::Diff { .. } => json!(
            "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new <i>x</i>\n"
        ),
        Request::Diffstat { .. } => json!(" src/a.rs | 2 +-\n"),
        Request::Land { .. } => json!({"branch": "goal/menus", "sha": "abc1234"}),
        Request::Merge { .. } => {
            json!({"branch": "goal/menus", "into": "master", "sha": "abcdef123456", "open": ["GAME-5"]})
        }
        Request::TicketSpend { .. } => json!(0.0019),
        Request::TicketSpends => json!({"GAME-3": 0.25, "GAME-4": 0.5, "GAME-9": 1.0}),
        Request::Asks => json!([{
            "id": "ASK-1", "ticket": "GAME-1", "hire": "dev", "kind": "permission",
            "tool": "Bash", "input": {"command": "cargo test"}, "reasons": ["no rule allows it"],
        }, {
            "id": "ASK-2", "ticket": "GAME-1", "hire": "dev", "kind": "question",
            "tool": "AskUserQuestion", "input": {"questions": []}, "reasons": [],
        }]),
        Request::Budget => json!({
            "limits": {"daily_usd": 10.0, "ticket_usd": 2.0, "run_usd": null, "min_run_usd": 0.05,
                       "projects": {"game": 3.0}, "hires": {}},
            "spent_24h": {"total": 2.9, "by_project": {"game": 2.9}, "by_hire": {"dev": 2.9},
                          "by_model": {"sonnet": 2.9}, "by_ticket": {"GAME-1": 2.9},
                          "runs_by_hire": {"dev": 4}},
            "runs_24h": 4,
            "reserved": {"GAME-1": 0.5},
        }),
        Request::SpendByDay { .. } => {
            json!((0..14)
            .map(|i| json!({"date": format!("2026-10-{:02}", 18 + i), "start_ms": i * 86_400_000,
                            "usd": if i == 13 { 2.9 } else { 1.0 }}))
            .collect::<Vec<_>>())
        }
        Request::Files {
            kind: FileKind::Hires,
        } => json!([
            {"name": "dev", "problem": null,
             "text": "---\nname: Dev <b>Dana</b>\nrole: engineer\nruntime: claude-code\nmodel: sonnet\nescalate: [opus]\neffort: high\nbudget_per_ticket_usd: 0.5\ntools: [file_ticket]\n---\nCode.\n"},
            {"name": "newbie", "problem": null,
             "text": "---\nname: Newbie\nrole: engineer\nruntime: claude-code\nmodel: haiku\n---\nCode.\n"},
        ]),
        Request::Files { .. } => json!([
            {"name": "good", "text": "---\nkind: guide\nlevel: warn\n---\nBe <i>kind</i>.\n", "problem": null},
            {"name": "bad", "text": "oops", "problem": "missing frontmatter"},
        ]),
        Request::RuleFiles => json!([
            {"origin": "studio", "owner": "", "name": "good", "path": "/home/o/studio/rules/good.md",
             "text": "---\nkind: guide\nlevel: warn\n---\nBe <i>kind</i>.\n", "problem": null},
            {"origin": "studio", "owner": "", "name": "bad", "path": "/home/o/studio/rules/bad.md",
             "text": "oops", "problem": "missing frontmatter"},
            {"origin": "project", "owner": "game", "name": "protect", "path": "/home/o/game/.studio/rules/protect.md",
             "text": "---\nkind: protect\nlevel: ask\n---\nAsk first.\n", "problem": null},
        ]),
        Request::HiresToday => json!({
            "dev": {"done": 2, "reviews": 1},
            "approver": {"allowed": 5, "denied": 1, "to_owner": 2},
        }),
        Request::ApproverCalls { .. } => json!([
            {"ticket": "GAME-1", "at": now - 60_000, "tool": "Bash",
             "what": "echo <b>hi</b>", "verdict": "denied", "reason": "not <i>needed</i>"},
            {"ticket": "GAME-2", "at": now - 120_000, "tool": "Read",
             "what": "src/a.rs", "verdict": "allowed", "reason": "reads"},
        ]),
        Request::TestRule { .. } => json!({
            "rule": "gate, block", "scanned": 3,
            "hits": [{"ticket": "GAME-1", "at": 1, "tool": "Bash",
                      "what": "git push <b>x</b>", "level": "block"}],
        }),
        _ => Value::Null,
    }
}

/// What the stand-in studiod is like.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Open,
    /// The office is closed.
    Closed,
    /// A new studio: the starter team, and no projects, tickets or goals.
    Empty,
    /// The same with one project added: still no tickets or goals.
    Fresh,
    /// Like `Open`, with a clock that stands still, so that studiod says the
    /// same thing every time it is asked.
    Still,
    /// Like `Open`, with forty-five more tickets that finished long ago.
    Old,
}

impl Mode {
    fn bare(self) -> bool {
        matches!(self, Mode::Empty | Mode::Fresh)
    }

    /// The time studiod tells.
    fn now(self) -> u64 {
        static STILL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        match self {
            Mode::Still => *STILL.get_or_init(ui_now),
            _ => ui_now(),
        }
    }
}

/// The socket of each time studiod was asked for the office.
static OFFICE_ASKED: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());

/// Whether the second ticket has started work, for a `Mode::Still` studiod.
static MOVED: AtomicBool = AtomicBool::new(false);

/// The starter team's five.
fn starter_team() -> Value {
    json!([
        {"id": "engineer", "role": "engineer", "model": "sonnet"},
        {"id": "producer", "role": "producer", "model": "sonnet"},
        {"id": "reviewer", "role": "reviewer", "model": "opus"},
        {"id": "approver", "role": "approver", "model": "haiku"},
        {"id": "router", "role": "router", "model": "haiku"},
    ])
}

/// What `Request::Office` answers from the same sample data as the requests
/// it stands for.
fn office(mode: Mode) -> Value {
    let part = |request: Request| answer(&request, mode).data;
    let events = Request::RecentEvents {
        since_ms: 0,
        limit: None,
        kinds: studiod::api::OFFICE_KINDS.map(String::from).to_vec(),
    };
    json!({
        "status": part(Request::Status),
        "tickets": part(Request::Tickets),
        "asks": part(Request::Asks),
        "events": part(events),
        "spend": part(Request::TicketSpends),
        "merged": part(Request::MergedTickets),
        "sizes": {},
        "today": part(Request::HiresToday),
        "judges": {},
        // What GAME-1's run last said, as the office's panel quotes it.
        "runs": if mode.bare() { json!({}) } else {
            json!({"GAME-1": {"said": "<img src=x> done"}})
        },
        "unreviewed": if mode.bare() { json!([]) } else { json!(["GAME-5"]) },
        "now": mode.now(),
    })
}

fn answer(request: &Request, mode: Mode) -> Response {
    let now = mode.now();
    match request {
        Request::Office => Response::ok(office(mode)),
        Request::Status if mode == Mode::Closed => {
            let mut status = reply(request, now);
            status["closed"] = json!(true);
            Response::ok(status)
        }
        Request::Status if mode.bare() => Response::ok(json!({
            "daily_usd": 10.0, "max_runs": 3, "team": starter_team(),
            "projects": if mode == Mode::Fresh { json!(["game"]) } else { json!([]) },
            "hires": ["engineer", "producer", "reviewer", "approver", "router"],
        })),
        Request::Tickets
        | Request::Asks
        | Request::MergedTickets
        | Request::RecentEvents { .. }
        | Request::Trophies
        | Request::ProjectChecks
        | Request::RuleFiles
            if mode.bare() =>
        {
            Response::ok(json!([]))
        }
        Request::Tickets if mode == Mode::Old => {
            let mut tickets = reply(request, now);
            // Landed one after the other, the last of them filed last.
            for n in 0..45 {
                tickets.as_array_mut().unwrap().push(json!({
                    "id": format!("GAME-{}", 100 + n), "project": "game",
                    "title": format!("Old job {n}"), "goal": "", "hire": "dev",
                    "state": if n % 3 == 0 { json!("cancelled") } else { json!("landed") },
                    "attempts": 1, "base_branch": "master", "created_at": 1_000 + n,
                }));
            }
            Response::ok(tickets)
        }
        Request::Tickets if mode == Mode::Still && MOVED.load(Ordering::SeqCst) => {
            let mut tickets = reply(request, now);
            tickets[1]["state"] = json!("working");
            Response::ok(tickets)
        }
        Request::SaveFile { text, .. } if text.contains("BROKEN") => {
            Response::err("validation error: lint rules require pattern")
        }
        Request::SaveFile { name, text, .. } => {
            SAVED.lock().unwrap().push((name.clone(), text.clone()));
            Response::ok(Value::Null)
        }
        _ => Response::ok(reply(request, now)),
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply.
fn fake_studiod(socket: &Path, mode: Mode) {
    let listener = UnixListener::bind(socket).unwrap();
    let socket = socket.to_path_buf();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let socket = socket.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                if request == Request::Office {
                    OFFICE_ASKED.lock().unwrap().push(socket);
                }
                let mut out = serde_json::to_string(&answer(&request, mode)).unwrap();
                out.push('\n');
                write.write_all(out.as_bytes()).await.unwrap();
            });
        }
    });
}

struct Web {
    base: String,
    http: reqwest::Client,
    socket: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Web {
    /// How many times the office was asked of this studiod.
    fn office_asked(&self) -> usize {
        let asked = OFFICE_ASKED.lock().unwrap();
        asked.iter().filter(|s| **s == self.socket).count()
    }

    async fn start() -> Web {
        Web::start_with(Mode::Open).await
    }

    /// A new studio: nothing on the floor yet.
    async fn start_empty() -> Web {
        Web::start_with(Mode::Empty).await
    }

    /// One project and nothing else.
    async fn start_fresh() -> Web {
        Web::start_with(Mode::Fresh).await
    }

    async fn start_with(mode: Mode) -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        fake_studiod(&socket, mode);
        let runs = dir.path().join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(
            runs.join("GAME-1.jsonl"),
            "{\"event\":{\"type\":\"text\",\"text\":\"<img src=x> done\"}}\n",
        )
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = AppConfig {
            socket: socket.clone(),
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
            socket,
            _dir: dir,
        }
    }

    /// A studiod whose clock stands still.
    async fn start_still() -> Web {
        Web::start_with(Mode::Still).await
    }

    async fn get(&self, path: &str, cookie: bool) -> reqwest::Response {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if cookie {
            request = request.header("cookie", format!("studio_token={TOKEN}"));
        }
        request.send().await.unwrap()
    }
}

#[tokio::test]
async fn the_link_sets_a_strict_cookie_and_pages_need_it() {
    let web = Web::start().await;

    assert_eq!(web.get("/", false).await.status(), 401);
    assert_eq!(web.get("/?token=wrong", false).await.status(), 401);

    let link = web.get(&format!("/?token={TOKEN}"), false).await;
    assert_eq!(link.status(), 303);
    assert_eq!(link.headers()["location"], "/");
    let cookie = link.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    // With the cookie already set, a link still drops its token, and keeps
    // the rest of its query.
    let again = web
        .get(&format!("/?token={TOKEN}&sel=tickets/GAME-1"), true)
        .await;
    assert_eq!(again.status(), 303);
    assert_eq!(again.headers()["location"], "/?sel=tickets/GAME-1");
    // The link lands on the office, the home page.
    let home = web.get("/", true).await.text().await.unwrap();
    assert!(home.contains(r#"id="office""#), "{home}");

    let css = web.get("/static/app.css", false).await;
    assert_eq!(css.status(), 200);
    let css = css.text().await.unwrap();
    assert!(css.contains("tailwindcss"));
    assert!(
        css.contains("IBM Plex Sans") && css.contains(".prog"),
        "{css}"
    );

    // The fonts need no token and are kept for a long time.
    let font = web
        .get("/static/fonts/ibm-plex-mono-500.woff2", false)
        .await;
    assert_eq!(font.status(), 200);
    assert_eq!(font.headers()["content-type"], "font/woff2");
    assert!(
        font.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("max-age")
    );
    assert!(font.bytes().await.unwrap().starts_with(b"wOF2"));
}

#[tokio::test]
async fn the_theme_switch_sets_a_cookie_the_page_follows() {
    let web = Web::start().await;
    let post = |theme: &'static str, back: &'static str| {
        web.http
            .post(format!("{}/theme", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .form(&[("theme", theme), ("back", back)])
            .send()
    };
    let light = post("light", "/hires").await.unwrap();
    assert_eq!(light.status(), 303);
    assert_eq!(light.headers()["location"], "/hires");
    let cookie = light.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with("theme=light") && cookie.contains("Path=/"),
        "{cookie}"
    );

    let cleared = post("system", "https://evil.example").await.unwrap();
    assert_eq!(cleared.headers()["location"], "/");
    let cookie = cleared.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("theme=;"), "{cookie}");

    let page = |theme: &str| {
        web.http
            .get(format!("{}/hires", web.base))
            .header("cookie", format!("studio_token={TOKEN}; theme={theme}"))
            .send()
    };
    let html = page("light").await.unwrap().text().await.unwrap();
    let root = named(&html, "html").remove(0);
    assert!(root.attr_is("lang", "en"), "{html}");
    assert!(root.attr_is("data-theme", "light"), "{html}");
    let html = page("purple").await.unwrap().text().await.unwrap();
    let root = named(&html, "html").remove(0);
    assert!(root.attr_is("lang", "en"), "{html}");
    assert!(!root.has("data-theme"), "{html}");
}

#[tokio::test]
async fn pages_render_studio_data_escaped() {
    let web = Web::start().await;

    let hires = web.get("/hires", true).await;
    assert_eq!(hires.status(), 200);
    let html = hires.text().await.unwrap();
    // The shell: a sidebar with what remains, the live status and the last
    // day's spend, and a top bar naming the page.
    let sidebar = named(&html, "aside").remove(0).text();
    for label in ["Office", "Studio", "Hires", "Rules"] {
        assert!(sidebar.contains(label), "{label}: {sidebar}");
    }
    // The pages the office replaced have no link.
    for gone in ["/board", "/asks", "/goals", "/budget"] {
        assert!(
            !html.contains(&format!(r#"href="{gone}""#)),
            "{gone}: {html}"
        );
    }
    // The spend leads to the office's Money tab.
    let spend = with(&html, "href", "/?tab=money");
    assert!(
        spend.iter().any(|t| t.text().contains("$0.25 of $10")),
        "{html}"
    );
    assert!(has_id(&html, "s-attn"), "{html}");
    assert!(html.contains(r#"name="theme""#), "{html}");
    assert!(html.contains("2 running"), "{html}");
    assert!(html.contains(" · 1 queued"), "{html}");
    assert_eq!(named(&html, "h1").remove(0).text(), "Hires", "{html}");
    assert!(!with(&html, "aria-current", "page").is_empty(), "{html}");

    let ticket = web.get("/tickets/GAME-1", true).await.text().await.unwrap();
    assert!(ticket.contains("&lt;img src=x&gt; done"), "{ticket}");
    assert!(ticket.contains("Resume"));
    assert!(ticket.contains("$0.0019"));
    assert_eq!(named(&ticket, "h1").len(), 1, "{ticket}");
    let crumbs = find(&ticket, "aria-label", "Breadcrumb");
    let back = find(crumbs.inner_html(), "href", "/");
    assert_eq!(back.text(), "Office", "{ticket}");
    // The rail: labelled facts, the spend against the ticket's cap, a branch
    // and a try once it has run, and its stages.
    let shown = |text: &str| tags(&ticket).iter().any(|t| t.text() == text);
    for label in [
        "Status", "Hire", "Project", "Branch", "Try", "Spent", "Progress",
    ] {
        assert!(shown(label), "{label}: {ticket}");
    }
    assert!(text_of(&ticket).contains(" of $2"), "{ticket}");
    assert!(shown("studio/game-1"), "{ticket}");
    assert!(shown("2 of 3"), "{ticket}");
    // The top bar exports the ticket; the menu resumes or cancels it.
    assert!(
        ticket.contains(r#"href="/tickets/GAME-1/export""#),
        "{ticket}"
    );
    assert!(ticket.contains("Cancel ticket"), "{ticket}");
    assert!(!shows(&ticket, "Pause"), "{ticket}");
    // The live stream starts after the log the page already shows.
    assert!(
        ticket.contains("/tickets/GAME-1/stream?from=52"),
        "{ticket}"
    );

    // The activity: human labels, relative times, escaped folded detail.
    assert!(ticket.contains("Checks failed"), "{ticket}");
    assert!(ticket.contains("3m ago"), "{ticket}");
    assert!(ticket.contains("Held: out of budget"), "{ticket}");
    assert!(ticket.contains("title=\"20"), "{ticket}");
    assert!(
        ticket.contains("&lt;script&gt;boom&lt;/script&gt;"),
        "{ticket}"
    );
    assert!(!ticket.contains("<script>boom"), "{ticket}");

    // The ticket's ask is answered on its page.
    assert!(ticket.contains("no rule allows it"));
    assert!(ticket.contains("cargo test"));
    let buttons: Vec<String> = named(&ticket, "button").iter().map(|b| b.text()).collect();
    for word in ["Allow", "Deny", "Always allow"] {
        assert!(
            buttons.iter().any(|b| b.starts_with(word)),
            "{word}: {buttons:?}"
        );
    }
}

#[tokio::test]
async fn posts_from_other_sites_are_refused() {
    let web = Web::start().await;
    let post = |origin: &str| {
        web.http
            .post(format!("{}/tickets/GAME-1/cancel", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", origin)
            .send()
    };
    // Topcoat's own cross-origin check answers first; ours backs it up.
    assert_eq!(post("https://evil.example").await.unwrap().status(), 403);
    assert_eq!(post(&web.base).await.unwrap().status(), 200);
}

#[tokio::test]
async fn rules_are_listed_by_origin_and_saved() {
    let web = Web::start().await;
    let page = web.get("/rules", true).await.text().await.unwrap();
    assert!(
        page.contains("Studio") && page.contains("Project game"),
        "{page}"
    );
    assert!(page.contains("missing frontmatter"), "{page}");
    assert!(
        page.contains("/rules?file=protect&amp;from=project%2Fgame"),
        "{page}"
    );
    // The first rule is open, with its kind and level and a confirmed Delete.
    assert!(page.contains("Be &lt;i&gt;kind&lt;/i&gt;."), "{page}");
    assert!(!page.contains("<i>kind</i>"));
    assert!(page.contains("guide · warn"), "{page}");
    assert!(page.contains(r#"popovertarget="delete-file""#), "{page}");
    assert!(page.contains("/rules/good/delete?open=good"), "{page}");
    assert!(shows(&page, "Save"), "{page}");

    let save = |text: &'static str| {
        web.http
            .post(format!("{}/rules/save", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", &web.base)
            .form(&[("name", "good"), ("text", text)])
            .send()
    };
    let broken = save("BROKEN").await.unwrap().text().await.unwrap();
    assert!(broken.contains("lint rules require pattern"), "{broken}");
    assert!(!broken.contains("Saved"));

    let saved = save("fine").await.unwrap().text().await.unwrap();
    assert!(saved.contains("Saved good."), "{saved}");
    assert!(saved.contains("file-list"), "{saved}");
}

#[tokio::test]
async fn a_projects_rule_is_shown_read_only() {
    let web = Web::start().await;
    let page = web
        .get("/rules?file=protect&from=project/game", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("Ask first."), "{page}");
    assert!(page.contains("readonly"), "{page}");
    assert!(page.contains("Lives in the game project"), "{page}");
    assert!(!page.contains("delete?open="), "{page}");
    assert!(!shows(&page, "Save"), "{page}");
}

#[tokio::test]
async fn deleting_a_rule_clears_the_editor_only_if_it_is_open() {
    let web = Web::start().await;
    let other = post_form(&web, "/rules/good/delete?open=bad", &[]).await;
    assert!(other.contains("Deleted good."), "{other}");
    assert!(!other.contains("Pick a rule"), "{other}");
    assert!(other.contains("file-list"), "{other}");

    let same = post_form(&web, "/rules/good/delete?open=good", &[]).await;
    assert!(same.contains("Pick a rule"), "{same}");
}

#[tokio::test]
async fn saving_from_another_site_is_refused() {
    let web = Web::start().await;
    let post = web
        .http
        .post(format!("{}/rules/save", web.base))
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("origin", "https://evil.example")
        .form(&[("name", "good"), ("text", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(post.status(), 403);
}

#[tokio::test]
async fn a_new_goal_starts_from_the_office_and_only_from_this_site() {
    let web = Web::start().await;
    let post = |origin: &str| {
        web.http
            .post(format!("{}/goals", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", origin)
            .form(&[("project", "game"), ("text", "x"), ("detail", "")])
            .send()
    };
    let ok = post(&web.base).await.unwrap();
    assert_eq!(ok.status(), 200);
    assert!(ok.text().await.unwrap().contains("Started GAME-6."));
    assert_eq!(post("https://evil.example").await.unwrap().status(), 403);
}

#[tokio::test]
async fn the_old_addresses_lead_to_the_office() {
    let web = Web::start().await;
    let live = web.get("/live", true).await;
    assert_eq!(live.status(), 308);
    assert_eq!(live.headers()["location"], "/");
    let office = web.get("/office?sel=tickets/GAME-1", true).await;
    assert_eq!(office.status(), 308);
    assert_eq!(office.headers()["location"], "/?sel=tickets/GAME-1");
}

#[tokio::test]
async fn the_pages_the_office_replaced_lead_to_its_tabs() {
    let web = Web::start().await;
    for (old, tab) in [
        ("/board", "/?tab=projects"),
        ("/goals", "/?tab=projects"),
        ("/asks", "/?tab=needs"),
        ("/budget", "/?tab=money"),
    ] {
        let reply = web.get(old, true).await;
        assert_eq!(reply.status(), 308, "{old}");
        assert_eq!(reply.headers()["location"], tab, "{old}");
    }
    // Their streams went with them.
    for stream in ["/board/stream", "/goals/stream", "/asks/stream"] {
        assert!(
            !web.get(stream, true).await.status().is_success(),
            "{stream}"
        );
    }
}

#[tokio::test]
async fn the_office_draws_the_floor_with_its_pins() {
    let web = Web::start().await;
    assert_eq!(web.get("/", false).await.status(), 401);

    let html = web.get("/", true).await.text().await.unwrap();
    // The floor is one drawing, sized by its own box, with things to pick.
    let scene = by_id(&html, "hq-scene");
    assert!(scene.attr_has("style", "--ar: "), "{html}");
    let drawing = named(scene.inner_html(), "svg").remove(0);
    assert!(drawing.attr_is("role", "img"), "{html}");
    assert!(
        drawing
            .attr("aria-label")
            .is_some_and(|l| l.contains("studio floor")),
        "{html}"
    );
    assert!(
        !with(&html, "data-sel", "tickets/GAME-1").is_empty(),
        "{html}"
    );
    assert!(
        !with(&html, "data-sel", "elements/board").is_empty(),
        "{html}"
    );
    // One handler on the floor picks whatever has a `data-sel`.
    assert!(html.contains("closest('[data-sel]')"), "{html}");
    assert!(html.contains("@get('/office/panel')"), "{html}");
    // Held at a desk: a pin with the project's colour and an escaped title.
    let pin = pin_for(&html, "tickets/GAME-1");
    assert!(pin.attr_is("data-pj", "0"), "{html}");
    assert!(
        pin.attr("aria-label").is_some_and(
            |l| l.starts_with("GAME-1: Add a <b>pause</b> menu") && l.contains("on hold")
        ),
        "{html}"
    );
    assert!(pin.text().contains("Add a <b>pause</b> menu"), "{html}");
    assert!(
        html.contains("Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{html}"
    );
    assert!(!html.contains(">Add a <b>"), "{html}");
    assert!(pin.text().contains("II"), "{html}");
    // Ready work lands from its pin; queued work and planning are quiet chips.
    let land = "@post('/tickets/GAME-5/land')";
    let ready = matching(&html, |t| {
        t.attr_is("data-pj", "0") && t.inner_html().contains(land)
    });
    assert!(!ready.is_empty(), "{html}");
    assert_eq!(find(&html, "data-on:click", land).text(), "Land", "{html}");
    assert!(
        pin_for(&html, "elements/queue").text().contains("up next"),
        "{html}"
    );
    assert!(
        pin_for(&html, "elements/board").text().contains("Planning"),
        "{html}"
    );
    // Hints name each game element and show on hover or with "What's what".
    assert!(
        find(&html, "data-for", "board")
            .text()
            .contains("Whiteboard · goals"),
        "{html}"
    );
    assert!(
        html.contains("What&#39;s what") || html.contains("What's what"),
        "{html}"
    );
    assert!(html.contains("data-class:show-hints"), "{html}");
    assert!(html.contains("$zoom"), "{html}");
    // Only hints name what they are for.
    assert!(!html.contains(r#"data-for="""#), "{html}");
    // Open: no sign.
    assert!(!has_id(&html, "closed-sign"), "{html}");
}

/// The label button on the floor that picks `sel`.
fn pin_for<'a>(html: &'a str, sel: &str) -> probe::Tag<'a> {
    matching(html, |t| t.name == "button" && t.attr_is("data-sel", sel))
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no pin for {sel} in {html}"))
}

#[tokio::test]
async fn the_top_bar_lists_projects_and_figures() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    // The page has its own top bar: no sidebar, rail or title bar.
    assert!(!html.contains(r#"aria-label="Studio""#), "{html}");
    assert!(!html.contains(r#"id="office-hud""#), "{html}");
    assert!(html.contains(r#"aria-label="Projects""#), "{html}");
    // Signals: the pick, the tab, the focus, the zoom and the hints.
    assert!(
        html.contains("{sel: '', tab: 'needs', focus: 'all', zoom: 1, hints: false, turn: 0"),
        "{html}"
    );
    // All projects, then each with its tint and its open tickets (GAME-1, 2,
    // 3, 5 and 7; the landed and cancelled are done).
    assert!(shows(&html, "All projects"), "{html}");
    assert!(html.contains("$focus = 0"), "{html}");
    let strip = by_id(&html, "hq-projects").inner_html();
    let game = find(strip, "title", "game: 5 in progress or waiting");
    assert_eq!(game.text(), "game 5", "{html}");
    // One ask and one ticket ready to land wait for the owner; two at work;
    // GAME-4 landed two hours ago, which is still within 24 hours.
    let figures = by_id(&html, "hq-figures").inner_html();
    let needs = matching(figures, |t| {
        t.name == "button" && t.attr_has("data-on:click", "$tab = 'needs'")
    });
    assert_eq!(needs[0].text(), "2 need you", "{html}");
    let working = find(figures, "title", "2 of 3 desks in use");
    assert_eq!(working.text(), "2 at work", "{html}");
    assert_eq!(
        find(&html, "title", "Landed in the last 24 hours").text(),
        "1 shipped",
        "{html}"
    );
    assert!(html.contains("2 of 3 desks in use"), "{html}");
    // The money meter: 24-hour spend against the daily limit.
    assert!(html.contains("$0.25"), "{html}");
    assert!(html.contains(" of $10"), "{html}");
    assert!(html.contains(r#"role="progressbar""#), "{html}");
    // The forms behind Ticket and New goal, and the theme switch.
    assert!(
        html.contains(r#"popovertarget="office-new-ticket""#),
        "{html}"
    );
    assert!(
        html.contains(r#"popovertarget="office-new-goal""#),
        "{html}"
    );
    assert!(html.contains(r#"action="/theme""#), "{html}");
    // Every other page keeps its sidebar, with the office at "/".
    let hires = web.get("/hires", true).await.text().await.unwrap();
    assert!(hires.contains(r#"aria-label="Studio""#), "{hires}");
    assert!(hires.contains(r#"href="/""#), "{hires}");
    assert!(!hires.contains(r#"id="hq-scene""#), "{hires}");
}

#[tokio::test]
async fn the_corners_track_projects_and_tell_the_latest() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    // GAME-3's goal has one of its two tickets landed; the other is ready.
    let corners = by_id(&html, "hq-corners").inner_html();
    let row = matching(corners, |t| {
        t.name == "button" && t.attr_has("data-on:click", "$focus = 0")
    });
    assert!(row[0].text().starts_with("game 1/2"), "{html}");
    assert!(
        html.contains("width: 50%; background: var(--pj-0)"),
        "{html}"
    );
    assert_eq!(find(corners, "title", "Need you").text(), "2", "{html}");
    // The live feed: the newest events in words, and a way to the Log tab.
    let feed = named(corners, "ol").remove(0).text();
    assert!(feed.contains("GAME-1 needs your say"), "{html}");
    assert!(html.contains("$tab = 'log'"), "{html}");
}

#[tokio::test]
async fn a_closed_office_shows_its_sign() {
    let web = Web::start_with(Mode::Closed).await;
    let html = web.get("/", true).await.text().await.unwrap();
    assert_eq!(
        by_id(&html, "closed-sign").text(),
        "Office closed · nothing new starts",
        "{html}"
    );
}

/// Whether the tab pane for `name` is shown when the page first arrives.
fn pane_shown(html: &str, name: &str) -> bool {
    let pane = find(html, "data-show", &format!("$tab == '{name}'"));
    !pane.attr_has("style", "display: none")
}

#[tokio::test]
async fn a_link_can_open_the_money_tab() {
    let web = Web::start().await;
    let html = web.get("/?tab=money", true).await.text().await.unwrap();
    assert!(html.contains("tab: 'money'"), "{html}");
    assert!(pane_shown(&html, "money"), "{html}");
    for other in ["needs", "projects", "team", "rules", "log"] {
        assert!(!pane_shown(&html, other), "{other} is open: {html}");
    }
    let tab = find(&html, "data-on:click", "$tab = 'money'");
    assert!(tab.attr_is("aria-selected", "true"), "{html}");

    // Without a link, the page opens on Needs you.
    let home = web.get("/", true).await.text().await.unwrap();
    assert!(home.contains("tab: 'needs'"), "{home}");
    assert!(pane_shown(&home, "needs"), "{home}");
    assert!(!pane_shown(&home, "money"), "{home}");
}

#[tokio::test]
async fn an_unknown_tab_opens_needs_you() {
    let web = Web::start().await;
    let html = web.get("/?tab=nope", true).await.text().await.unwrap();
    assert!(html.contains("tab: 'needs'"), "{html}");
    assert!(!html.contains("tab: 'nope'"), "{html}");
    assert!(pane_shown(&html, "needs"), "{html}");
    assert!(!pane_shown(&html, "money"), "{html}");
}

#[tokio::test]
async fn a_link_can_open_the_coffee_machine() {
    let web = Web::start().await;
    let html = web
        .get("/?sel=elements/coffee", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(html.contains("sel: 'elements/coffee'"), "{html}");
    assert!(html.contains("Coffee machine"), "{html}");
    assert!(html.contains("The office is open"), "{html}");
    assert!(html.contains("@post('/office/close')"), "{html}");
    // The element is marked on the floor.
    let marked = with(&html, "data-sel", "elements/coffee");
    assert!(marked.iter().any(|t| t.has("data-picked")), "{html}");
}

#[tokio::test]
async fn a_link_to_something_unknown_shows_the_tabs() {
    let web = Web::start().await;
    let html = web
        .get("/?sel=elements/nope", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(html.contains("sel: ''"), "{html}");
    assert!(!html.contains("elements/nope"), "{html}");
    assert!(html.contains(r#"role="tablist""#), "{html}");
    assert!(pane_shown(&html, "needs"), "{html}");
    assert!(!html.contains("The office is open"), "{html}");
    assert!(!html.contains("data-picked"), "{html}");
}

#[tokio::test]
async fn a_link_to_a_ticket_at_a_desk_marks_it_on_the_floor() {
    let web = Web::start().await;
    let html = web
        .get("/?sel=tickets/GAME-1", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(html.contains("sel: 'tickets/GAME-1'"), "{html}");
    // On the drawing, the desk's element carries the mark.
    let drawing = named(by_id(&html, "hq-scene").inner_html(), "svg").remove(0);
    let drawing = drawing.inner_html();
    let marked = matching(drawing, |t| t.has("data-picked"));
    assert_eq!(marked.len(), 1, "{drawing}");
    assert!(marked[0].attr_is("data-sel", "tickets/GAME-1"), "{drawing}");
    // Its label is pressed, and no other is.
    let label = pin_for(&html, "tickets/GAME-1");
    assert!(label.attr_is("aria-pressed", "true"), "{html}");
    assert_eq!(with(&html, "aria-pressed", "true").len(), 1, "{html}");

    // Nothing is marked when nothing is picked.
    let home = web.get("/", true).await.text().await.unwrap();
    assert!(!home.contains("data-picked"), "{home}");
    assert!(!home.contains(r#"aria-pressed="true""#), "{home}");
}

/// A GET as Datastar sends it, with the page's signals in the query.
async fn datastar_get(web: &Web, path: &str, sel: &str) -> reqwest::Response {
    datastar_get_with(web, path, json!({ "sel": sel })).await
}

async fn datastar_get_with(web: &Web, path: &str, signals: Value) -> reqwest::Response {
    let signals = serde_json::to_string(&signals).unwrap();
    let query: String = url::form_urlencoded::byte_serialize(signals.as_bytes()).collect();
    web.http
        .get(format!("{}{path}?datastar={query}", web.base))
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("datastar-request", "true")
        .send()
        .await
        .unwrap()
}

/// The panel with `tab` open and `history` rows of History asked for.
async fn panel_on(web: &Web, sel: &str, tab: &str, history: usize) -> String {
    let signals = json!({"sel": sel, "tab": tab, "history": history});
    datastar_get_with(web, "/office/panel", signals)
        .await
        .text()
        .await
        .unwrap()
}

/// The inspector's panel for `sel`, as the floor asks for it.
async fn panel(web: &Web, sel: &str) -> String {
    datastar_get(web, "/office/panel", sel)
        .await
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn office_panels_answer_through_the_usual_endpoints() {
    let web = Web::start().await;

    // A ticket with an ask: the full card, answered on the asks endpoint.
    let ask = panel(&web, "tickets/GAME-1").await;
    assert!(ask.contains(r#"id="inspector""#), "{ask}");
    assert!(ask.contains("Wants to run a command"), "{ask}");
    assert!(
        ask.contains("@post('/asks/ASK-1', {contentType: 'form'})"),
        "{ask}"
    );
    assert!(ask.contains(r#"value="approve""#) && ask.contains(r#"value="deny""#));
    assert!(ask.contains("Always allow"), "{ask}");
    // The agent's words are quoted, escaped.
    assert!(ask.contains("&lt;img src=x&gt; done"), "{ask}");
    assert!(!ask.contains("<img src=x>"), "{ask}");

    let ready = panel(&web, "tickets/GAME-5").await;
    assert!(ready.contains("@post('/tickets/GAME-5/land')"), "{ready}");
    assert!(ready.contains("it is yours to review"), "{ready}");
    assert!(ready.contains("src/a.rs"), "{ready}");
    assert!(!ready.contains("<i>x</i>"), "{ready}");

    let queued = panel(&web, "tickets/GAME-2").await;
    assert!(
        queued.contains("@post('/tickets/GAME-2/cancel')"),
        "{queued}"
    );
    assert!(queued.contains("when a desk frees up"), "{queued}");

    let planning = panel(&web, "tickets/GAME-3").await;
    assert!(planning.contains(r#"href="/tickets/GAME-3""#), "{planning}");
    assert!(
        planning.contains("@post('/tickets/GAME-3/cancel')"),
        "{planning}"
    );

    // A goal lists what it filed, cancelled work left out.
    let goal = panel(&web, "goals/GAME-3").await;
    assert!(goal.contains("Ship the &lt;i&gt;menus&lt;/i&gt;"), "{goal}");
    assert!(goal.contains("1 of 2 landed"), "{goal}");
    assert!(!goal.contains("GAME-9"), "{goal}");

    // A hire is given work through the New ticket form, with it picked.
    let hire = panel(&web, "hires/dev").await;
    assert!(hire.contains("Give it a ticket"), "{hire}");
    assert!(hire.contains("$pick = 'dev'"), "{hire}");
    assert!(
        hire.contains(r#"popovertarget="office-new-ticket""#),
        "{hire}"
    );
    assert!(hire.contains(r#"href="/hires?file=dev""#), "{hire}");
    assert!(!shows(&hire, "Latest"), "{hire}");

    // The approver lists its latest calls, escaped, each linked to its
    // ticket; a denial says why.
    let approver = panel(&web, "hires/approver").await;
    assert!(shows(&approver, "Latest"), "{approver}");
    assert!(shows(&approver, "echo <b>hi</b>"), "{approver}");
    assert!(!approver.contains("><b>hi"), "{approver}");
    let said = text_of(&approver);
    assert!(said.contains("Denied"), "{approver}");
    assert!(said.contains("not <i>needed</i>"), "{approver}");
    assert!(
        !named(&approver, "i").iter().any(|i| i.text() == "needed"),
        "{approver}"
    );
    assert!(said.contains("Allowed"), "{approver}");
    assert!(
        !with(&approver, "href", "/tickets/GAME-2").is_empty(),
        "{approver}"
    );

    // A merged goal opens like any other, from History.
    let merged = panel(&web, "goals/GAME-8").await;
    assert!(merged.contains("Add audio"), "{merged}");
    assert!(merged.contains("1 of 1 landed"), "{merged}");
    assert!(!merged.contains("Merge into"), "{merged}");

    // An unknown ticket and a forged key show the tabs.
    for sel in ["tickets/NOPE-1", "tickets/GAME-1'); alert(1)//"] {
        let html = panel(&web, sel).await;
        assert!(html.contains(r#"role="tablist""#), "{sel}: {html}");
        assert!(!html.contains("alert("), "{sel}: {html}");
    }
    let overview = panel(&web, "").await;
    assert!(overview.contains(r#"id="ask-ASK-1""#), "{overview}");
    assert!(overview.contains("$tab = 'projects'"), "{overview}");
}

#[tokio::test]
async fn the_office_stream_keeps_the_selected_panel() {
    let web = Web::start().await;
    let mut stream = datastar_get(&web, "/office/stream", "tickets/GAME-5").await;
    let mut text = String::new();
    while !text.contains(r#"id="inspector""#) {
        let chunk = stream.chunk().await.unwrap().unwrap();
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    // The projects, the figures, the floor with its pins, the corners and
    // the panel are all re-sent, each with the id Datastar morphs it by.
    for id in [
        "hq-projects",
        "hq-figures",
        "hq-scene",
        "hq-corners",
        "hq-panel",
    ] {
        assert!(text.contains(&format!(r#"id="{id}""#)), "{id}: {text}");
    }
    // Ticket figures keep their ids, so a figure walks when its ticket moves.
    assert!(text.contains(r#"id="w-GAME-1""#), "{text}");
    assert!(
        pin_for(&text, "tickets/GAME-1").attr_has("aria-label", "on hold"),
        "{text}"
    );
    assert!(text.contains("@post('/tickets/GAME-5/land')"), "{text}");
}

#[tokio::test]
async fn the_ticket_page_puts_its_ask_under_the_title() {
    let web = Web::start().await;
    let html = web.get("/tickets/GAME-1", true).await.text().await.unwrap();
    let title = html.find("<h1").unwrap();
    let ask = html.find(r#"id="ask-ASK-1""#).unwrap();
    let activity = html.find(r#"id="activity""#).unwrap();
    assert!(title < ask && ask < activity, "{html}");
    assert!(html.contains("Always allow"), "{html}");
    assert!(html.contains("Wants to run a command"), "{html}");
    // The ticket id is in the page's own header, not repeated on the card.
    assert!(!html.contains(r#"href="/tickets/GAME-1""#), "{html}");
}

#[tokio::test]
async fn the_ticket_page_labels_its_goal_and_shows_its_activity() {
    let web = Web::start().await;
    let html = web.get("/tickets/GAME-3", true).await.text().await.unwrap();
    assert!(shows(&html, "Goal"), "{html}");
    assert!(html.contains("Ship the &lt;i&gt;menus&lt;/i&gt;"), "{html}");
    // The full goal is only in the quoted title attribute, never as markup.
    assert!(!html.contains(">Ship the <i>"), "{html}");
    assert!(html.contains("You set this goal"), "{html}");
    assert!(html.contains(r#"id="activity""#), "{html}");
    // A working ticket can be paused.
    assert!(html.contains("/tickets/GAME-3/pause"), "{html}");
}

#[tokio::test]
async fn a_ticket_exports_as_markdown_and_pauses() {
    let web = Web::start().await;
    let reply = web.get("/tickets/GAME-1/export", true).await;
    assert_eq!(reply.status(), 200);
    let header = |name: &str| reply.headers()[name].to_str().unwrap().to_string();
    assert_eq!(header("content-type"), "text/markdown; charset=utf-8");
    assert!(header("content-disposition").contains("GAME-1.md"));
    let text = reply.text().await.unwrap();
    assert!(text.starts_with("# Add a <b>pause</b> menu\n"), "{text}");
    assert!(text.contains("- Ticket: GAME-1"), "{text}");
    assert!(
        text.contains("checks failed: <script>boom</script>"),
        "{text}"
    );
    let missing = web.get("/tickets/NOPE-1/export", true).await;
    assert_eq!(missing.status(), 404);

    let reply = post_form(&web, "/tickets/GAME-3/pause", &[]).await;
    assert!(reply.contains("Pausing"), "{reply}");
}

#[tokio::test]
async fn always_allow_approves_and_saves_a_rule_that_loads() {
    let web = Web::start().await;
    let reply = web
        .http
        .post(format!("{}/asks/ASK-1", web.base))
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("origin", &web.base)
        .form(&[("decision", "always")])
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(reply.contains("Allowed ASK-1."), "{reply}");
    assert!(reply.contains("allow-game-cargo-test"), "{reply}");
    let saved = SAVED.lock().unwrap().clone();
    let (name, text) = saved
        .iter()
        .find(|(name, _)| name == "allow-game-cargo-test")
        .unwrap();
    studio_config::check_rule(name, text).unwrap();
    assert!(text.contains(r#"action: "bash:cargo test""#), "{text}");
}

/// Posts a form to the web UI as the owner would.
async fn post_form(web: &Web, path: &str, form: &[(&str, &str)]) -> String {
    web.http
        .post(format!("{}{path}", web.base))
        .header("cookie", format!("studio_token={TOKEN}"))
        .header("origin", &web.base)
        .form(form)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio::test]
async fn the_new_rule_editor_can_be_prefilled() {
    let web = Web::start().await;
    let page = web
        .get(
            "/rules?new=1&name=block-game-cargo&text=---%0Akind%3A+gate%0A---%0A%3Cscript%3Ex%3C%2Fscript%3E",
            true,
        )
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains(r#"value="block-game-cargo""#), "{page}");
    assert!(page.contains("kind: gate"), "{page}");
    assert!(page.contains("&lt;script&gt;x&lt;/script&gt;"), "{page}");
    assert!(!page.contains("<script>x"), "{page}");
    assert!(page.contains("/rules/test") && page.contains(r#"id="rule-test""#));
}

#[tokio::test]
async fn testing_a_rule_lists_what_it_would_have_matched() {
    let web = Web::start().await;
    let reply = post_form(&web, "/rules/test", &[("name", "x"), ("text", "---")]).await;
    assert!(
        reply.contains("Would have matched 1 of 3 tool calls"),
        "{reply}"
    );
    assert!(reply.contains("/tickets/GAME-1"), "{reply}");
    assert!(reply.contains("git push &lt;b&gt;x&lt;/b&gt;"), "{reply}");
}

#[tokio::test]
async fn denying_offers_to_make_a_rule() {
    let web = Web::start().await;
    let reply = post_form(
        &web,
        "/asks/ASK-1",
        &[("decision", "deny"), ("message", "no <b>tests</b>")],
    )
    .await;
    assert!(reply.contains("Denied ASK-1."), "{reply}");
    assert!(reply.contains("Make this a rule"), "{reply}");
    assert!(
        reply.contains("/rules?new=1&amp;name=block-game-cargo-test"),
        "{reply}"
    );
    assert!(!reply.contains("<b>tests"), "{reply}");
}

#[tokio::test]
async fn denying_a_question_offers_no_rule() {
    let web = Web::start().await;
    let reply = post_form(&web, "/asks/ASK-2", &[("decision", "deny")]).await;
    assert!(reply.contains("Denied ASK-2."), "{reply}");
    assert!(!reply.contains("Make this a rule"), "{reply}");
}

#[tokio::test]
async fn sending_back_offers_a_rule_for_the_project() {
    let web = Web::start().await;
    let reply = post_form(
        &web,
        "/tickets/GAME-5/changes",
        &[("feedback", "Keep the HUD out of the menu")],
    )
    .await;
    assert!(reply.contains("Sent back with your feedback."), "{reply}");
    assert!(
        reply.contains("Make it a rule for every ticket in game"),
        "{reply}"
    );
    assert!(
        reply.contains("name=guide-game-keep-the-hud-out-of-the"),
        "{reply}"
    );
}

#[tokio::test]
async fn hires_show_as_a_table() {
    let web = Web::start().await;
    let html = web.get("/hires", true).await.text().await.unwrap();
    assert!(html.contains("sonnet") && html.contains("opus"), "{html}");
    assert!(html.contains("High"), "{html}");
    assert!(html.contains("/hires?file=dev"), "{html}");
    // What it did today and what that cost; a hire with nothing yet says so.
    assert!(html.contains("2 done · 1 review"), "{html}");
    assert!(html.contains("$2.90"), "{html}");
    assert!(html.contains("Nothing yet"), "{html}");
    assert_eq!(html.matches(">Idle<").count(), 2, "{html}");
    // Opening a file brings the editor, with a confirmed Delete.
    let html = web.get("/hires?file=dev", true).await.text().await.unwrap();
    assert!(!named(&html, "textarea").is_empty(), "{html}");
    assert!(html.contains("Delete this hire?"), "{html}");
    assert!(html.contains("&lt;b&gt;Dana&lt;/b&gt;"), "{html}");
}

#[tokio::test]
async fn the_office_and_the_hires_render_without_the_run_logs() {
    let web = Web::start().await;
    std::fs::remove_dir_all(web._dir.path().join("runs")).unwrap();

    let home = web.get("/", true).await.text().await.unwrap();
    assert!(home.contains(r#"id="office""#), "{home}");
    // The panel for a ticket with a run still quotes what the studio said.
    let ask = panel(&web, "tickets/GAME-1").await;
    assert!(ask.contains("Wants to run a command"), "{ask}");
    let working = panel(&web, "tickets/GAME-3").await;
    assert!(working.contains("Plan: ship the"), "{working}");
    let hires = web.get("/hires", true).await.text().await.unwrap();
    assert!(hires.contains("2 done · 1 review"), "{hires}");
}

#[tokio::test]
async fn ready_work_shows_its_diff_and_landed_work_can_merge() {
    let web = Web::start().await;
    let ready = web.get("/tickets/GAME-5", true).await.text().await.unwrap();
    assert!(ready.contains("src/a.rs"), "{ready}");
    assert!(ready.contains("+new &lt;i&gt;x&lt;/i&gt;"), "{ready}");
    assert!(!ready.contains("<i>x</i>"), "{ready}");
    let landed = web.get("/tickets/GAME-4", true).await.text().await.unwrap();
    assert!(landed.contains("Merge into master"), "{landed}");
    assert!(landed.contains("nothing is pushed"), "{landed}");
    let done = web
        .get("/tickets/GAME-10", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(done.contains("Merged into master."), "{done}");
    assert!(!done.contains("Merge into master"), "{done}");
    // Finished first time: the first try, not the zeroth.
    assert!(shows(&done, "1 of 3"), "{done}");
    // Ready work lands from its outcome, or goes back with notes.
    assert!(ready.contains("/tickets/GAME-5/land"), "{ready}");
    assert!(ready.contains("/tickets/GAME-5/changes"), "{ready}");

    let post = |path: &str| {
        web.http
            .post(format!("{}{path}", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", &web.base)
            .send()
    };
    let merged = post("/tickets/GAME-4/merge")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        merged.contains("Merged goal/menus into master (abcdef1)"),
        "{merged}"
    );
    let land = post("/tickets/GAME-5/land")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        land.contains("Landed on") && land.contains("/tickets/GAME-5/merge"),
        "{land}"
    );
}

/// The floor's element, as the page and its updates draw it.
fn scene_of(html: &str) -> &str {
    let from = html.find(r#"id="hq-scene""#).expect("the floor");
    let to = html[from..]
        .find(r#"id="hq-corners""#)
        .map_or(html.len(), |n| from + n);
    &html[from..to]
}

#[tokio::test]
async fn the_page_starts_from_the_corner_the_link_names() {
    let web = Web::start().await;
    let home = web.get("/", true).await.text().await.unwrap();
    assert!(home.contains("turn: 0"), "{home}");
    assert!(home.contains(r#"data-turn="0""#), "{home}");
    let turned = web.get("/?turn=2", true).await.text().await.unwrap();
    assert!(turned.contains("turn: 2"), "{turned}");
    assert!(turned.contains(r#"data-turn="2""#), "{turned}");
    assert_ne!(scene_of(&home), scene_of(&turned));
    // Any whole number wraps to a corner: 5 is 1, -1 is 3.
    for (link, corner) in [("5", 1), ("-1", 3), ("4", 0)] {
        let html = web
            .get(&format!("/?turn={link}"), true)
            .await
            .text()
            .await
            .unwrap();
        assert!(html.contains(&format!("turn: {corner},")), "{link}: {html}");
    }
}

#[tokio::test]
async fn the_scene_route_draws_the_floor_for_the_turn_in_the_signals() {
    let web = Web::start().await;
    let at = |turn: i64| {
        let web = &web;
        async move {
            datastar_get_with(web, "/office/scene", json!({"sel": "", "turn": turn}))
                .await
                .text()
                .await
                .unwrap()
        }
    };
    let (first, third) = (at(0).await, at(2).await);
    // Only the floor comes back, as the element Datastar morphs by its id.
    for floor in [&first, &third] {
        assert!(floor.contains(r#"id="hq-scene""#), "{floor}");
        let drawing = named(by_id(floor, "hq-scene").inner_html(), "svg");
        assert!(!drawing.is_empty(), "{floor}");
        assert!(!floor.contains(r#"id="hq-panel""#), "{floor}");
    }
    assert!(first.contains(r#"data-turn="0""#), "{first}");
    assert!(third.contains(r#"data-turn="2""#), "{third}");
    assert_ne!(first, third);
    // The pick stays marked, and the turn wraps.
    let picked = datastar_get_with(
        &web,
        "/office/scene",
        json!({"sel": "tickets/GAME-1", "turn": 5}),
    )
    .await
    .text()
    .await
    .unwrap();
    assert!(picked.contains(r#"data-turn="1""#), "{picked}");
    assert!(picked.contains("data-picked"), "{picked}");
}

#[tokio::test]
async fn the_live_stream_draws_the_floor_from_the_turn_it_was_opened_with() {
    let web = Web::start().await;
    let mut stream = datastar_get_with(&web, "/office/stream", json!({"sel": "", "turn": 3})).await;
    let mut text = String::new();
    while !text.contains(r#"id="inspector""#) {
        let chunk = stream.chunk().await.unwrap().unwrap();
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(text.contains(r#"data-turn="3""#), "{text}");
    // The effect that opens the stream runs again when the turn changes.
    let home = web.get("/", true).await.text().await.unwrap();
    assert!(
        home.contains("data-effect=\"$sel; $turn; $history; @get('/office/stream')\""),
        "{home}"
    );
}

/// Reads a stream until an update that says `needle` is whole; everything
/// read.
async fn read_until(stream: &mut reqwest::Response, needle: &str) -> String {
    let mut text = String::new();
    while !(text.contains(needle) && text.ends_with("\n\n")) {
        let chunk = stream.chunk().await.unwrap().unwrap();
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
}

#[tokio::test]
async fn the_office_page_is_one_request_to_studiod_with_all_its_parts() {
    let web = Web::start().await;
    let home = web.get("/", true).await.text().await.unwrap();
    assert_eq!(web.office_asked(), 1);
    // The top bar, the floor with its figures and pins, the corners and the
    // side panel with what needs the owner.
    for id in [
        "hq-projects",
        "hq-figures",
        "hq-scene",
        "hq-corners",
        "hq-panel",
    ] {
        assert!(home.contains(&format!(r#"id="{id}""#)), "{id}: {home}");
    }
    assert!(home.contains(r#"id="w-GAME-1""#), "{home}");
    assert!(home.contains(r#"id="ask-ASK-1""#), "{home}");
}

#[tokio::test]
async fn the_office_stream_sends_again_only_when_something_changed() {
    let web = Web::start_still().await;
    let mut stream = datastar_get(&web, "/office/stream", "").await;
    let first = read_until(&mut stream, r#"id="inspector""#).await;
    assert!(!first.contains(r#"id="w-GAME-2""#), "{first}");

    // Nothing moves: studiod is asked again, but nothing is sent.
    let quiet = tokio::time::timeout(Duration::from_millis(2500), stream.chunk()).await;
    assert!(quiet.is_err(), "{quiet:?}");
    assert!(web.office_asked() >= 3, "{}", web.office_asked());

    // A ticket starts work: the next update shows its figure.
    MOVED.store(true, Ordering::SeqCst);
    let next = tokio::time::timeout(
        Duration::from_secs(5),
        read_until(&mut stream, r#"id="inspector""#),
    )
    .await
    .expect("an update after the change");
    assert!(next.contains(r#"id="w-GAME-2""#), "{next}");
}

#[tokio::test]
async fn open_offices_share_one_read_of_studiod() {
    let web = Web::start_still().await;
    let mut streams = Vec::new();
    for _ in 0..3 {
        streams.push(datastar_get(&web, "/office/stream", "").await);
    }
    for stream in &mut streams {
        read_until(stream, r#"id="inspector""#).await;
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    // Three streams, about three ticks: one read each tick, not three.
    let asked = web.office_asked();
    assert!((1..=5).contains(&asked), "{asked}");
}

#[tokio::test]
async fn the_view_controls_turn_the_floor_by_buttons_and_keys() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    for (label, step) in [("Turn left", "$turn + 3"), ("Turn right", "$turn + 1")] {
        let tag = find(&html, "aria-label", label).attributes();
        assert!(tag.contains(step), "{label}: {tag}");
        assert!(tag.contains("@get('/office/scene')"), "{label}: {tag}");
    }
    // Q and E anywhere on the page, but not while typing or with a modifier.
    let keys = matching(&html, |t| t.has("data-on:keydown__window"))[0].attributes();
    for part in [
        "k === 'q'",
        "k === 'e'",
        "ctrlKey",
        "metaKey",
        "altKey",
        "input, textarea, select, [contenteditable]",
        "@get('/office/scene')",
    ] {
        assert!(keys.contains(part), "{part}: {keys}");
    }
}

#[tokio::test]
async fn the_floor_can_be_dragged_when_zoomed_in_without_picking_what_is_under_it() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    let wrap = by_id(&html, "hq-wrap").attributes();
    // Only the mouse drags; touch keeps scrolling.
    assert!(wrap.contains("pointerType === 'mouse'"), "{wrap}");
    assert!(wrap.contains("$zoom > 1"), "{wrap}");
    assert!(wrap.contains("data-on:pointermove__window"), "{wrap}");
    assert!(wrap.contains("scrollLeft"), "{wrap}");
    // A drag past 4 pixels ends in a click nobody sees.
    assert!(wrap.contains("Math.hypot(dx, dy) > 4"), "{wrap}");
    assert!(wrap.contains("data-on:click__capture"), "{wrap}");
    assert!(wrap.contains("stopPropagation"), "{wrap}");
    // A grab cursor while zoomed, a grabbing one while dragging.
    assert!(wrap.contains("cursor-grab"), "{wrap}");
    assert!(wrap.contains("cursor-grabbing"), "{wrap}");
    // Dragging is local to the page: signals with an underscore stay home.
    assert!(html.contains("_dragging: false"), "{html}");
}

#[tokio::test]
async fn a_new_studio_starts_with_the_first_steps() {
    let web = Web::start_empty().await;
    let html = web.get("/", true).await.text().await.unwrap();
    // A bare floor: still the room, its things to pick and no tickets.
    assert!(html.contains(r#"id="hq-scene""#), "{html}");
    assert!(
        html.contains(r#"data-sel="elements/add-project""#),
        "{html}"
    );
    assert!(html.contains(r#"data-sel="elements/hire""#), "{html}");
    // The Needs you tab leads with what to do first.
    assert!(shows(&html, "Start here"), "{html}");
    assert!(html.contains("Add a project"), "{html}");
    assert!(html.contains("Pin a goal on the whiteboard"), "{html}");
    assert!(html.contains("@get('/office/panel')"), "{html}");
    assert!(html.contains("$sel = 'elements/add-project'"), "{html}");
    assert!(
        html.contains(r#"popovertarget="office-new-goal""#),
        "{html}"
    );
    assert!(html.contains("No projects yet."), "{html}");

    // With a project but no work yet, only the goal step is left.
    let fresh = Web::start_fresh().await;
    let html = fresh.get("/", true).await.text().await.unwrap();
    assert!(shows(&html, "Start here"), "{html}");
    assert!(html.contains("Pin a goal on the whiteboard"), "{html}");
    assert!(!html.contains("$sel = 'elements/add-project'"), "{html}");
    // Once there is work, the list is gone.
    let busy = Web::start().await;
    let html = busy.get("/", true).await.text().await.unwrap();
    assert!(!shows(&html, "Start here"), "{html}");
}

#[tokio::test]
async fn the_add_project_and_hire_views_hold_their_forms() {
    let web = Web::start_empty().await;
    let add = panel(&web, "elements/add-project").await;
    assert!(
        add.contains("@post('/office/projects', {contentType: 'form'})"),
        "{add}"
    );
    for field in ["path", "name", "branch"] {
        assert!(
            add.contains(&format!(r#"name="{field}""#)),
            "{field}: {add}"
        );
    }
    assert!(add.contains("Add project"), "{add}");
    // What studiod found is shown to be confirmed before anything is added:
    // Add it posts the same form with the confirmation, Cancel drops it.
    assert!(
        add.contains(
            "@post('/office/projects?add=yes', {contentType: 'form', selector: '#add-project'})"
        ),
        "{add}"
    );
    for found in [
        "_found.name",
        "_found.path",
        "_found.branch",
        "_found.checks",
    ] {
        assert!(
            add.contains(&format!(r#"data-text="${found}""#)),
            "{found}: {add}"
        );
    }
    let buttons: Vec<String> = named(&add, "button").iter().map(|b| b.text()).collect();
    for word in ["Add it", "Cancel"] {
        assert!(
            buttons.iter().any(|b| b.starts_with(word)),
            "{word}: {buttons:?}"
        );
    }
    // Coming back to the panel starts from the form again.
    assert!(add.contains(r#"data-init="$_found.path = ''""#), "{add}");
    assert!(add.contains("Projects now"), "{add}");
    assert!(add.contains("No projects yet."), "{add}");
    assert!(!add.contains("Coming soon"), "{add}");

    // With projects, each is listed.
    let busy = Web::start().await;
    let add = panel(&busy, "elements/add-project").await;
    let listed = named(&add, "section")
        .into_iter()
        .map(|s| s.text())
        .find(|t| t.starts_with("Projects now"))
        .unwrap_or_default();
    assert!(listed.contains("game"), "{add}");
    assert!(!listed.contains("No projects yet."), "{add}");

    let hire = panel(&web, "elements/hire").await;
    assert!(
        hire.contains("@post('/office/hires', {contentType: 'form'})"),
        "{hire}"
    );
    for field in ["name", "role", "model", "about"] {
        assert!(
            hire.contains(&format!(r#"name="{field}""#)),
            "{field}: {hire}"
        );
    }
    for option in [
        "engineer", "designer", "writer", "producer", "reviewer", "critic", "approver", "router",
        "sonnet", "opus", "haiku",
    ] {
        assert!(
            named(&hire, "option")
                .iter()
                .any(|o| o.attr_is("value", option)),
            "{option}: {hire}"
        );
    }
    assert!(hire.contains(r#"href="/hires?new""#), "{hire}");
    assert!(!hire.contains("Coming soon"), "{hire}");

    // Checks with no projects say so, and offer to add one.
    let racks = panel(&web, "elements/racks").await;
    assert!(racks.contains("No projects yet."), "{racks}");
    assert!(racks.contains("$sel = 'elements/add-project'"), "{racks}");
    let racks = panel(&busy, "elements/racks").await;
    assert!(!racks.contains("$sel = 'elements/add-project'"), "{racks}");
}

/// The rows under History in a panel, as (id, the words the row shows).
fn history_rows(html: &str) -> Vec<(String, String)> {
    let section = named(html, "section")
        .into_iter()
        .find(|s| s.text().starts_with("History"))
        .expect("History");
    matching(section.inner_html(), |t| {
        t.name == "button" && t.attr_has("data-on:click", "$sel = '")
    })
    .iter()
    .map(|row| {
        let click = row.attr("data-on:click").unwrap_or_default();
        let key = click.split('\'').nth(1).unwrap_or_default().to_string();
        let id = key.rsplit('/').next().unwrap_or_default().to_string();
        (id, row.text())
    })
    .collect()
}

/// The ids of the rows under History in a panel.
fn history_ids(html: &str) -> Vec<String> {
    history_rows(html).into_iter().map(|(id, _)| id).collect()
}

#[tokio::test]
async fn history_lists_merged_goals_and_old_tickets() {
    let web = Web::start().await;
    let html = panel_on(&web, "", "projects", 20).await;
    // The merged goal is a row of its own, selecting its panel; so are the
    // old tickets the rows above leave out.
    assert!(
        text_of(&html).contains("History 2 · newest first"),
        "{html}"
    );
    let rows = history_ids(&html);
    assert_eq!(rows, ["GAME-8", "GAME-9"], "{html}");
    assert!(html.contains("$sel = 'goals/GAME-8'"), "{html}");
    assert!(html.contains("$sel = 'tickets/GAME-9'"), "{html}");
    let words = history_rows(&html);
    assert!(words[0].1.contains("Add audio"), "{words:?}");
    assert!(words[0].1.ends_with("merged"), "{words:?}");
    assert!(words[1].1.ends_with("cancelled"), "{words:?}");
    // Nothing more to show.
    assert!(!html.contains("Show more"), "{html}");
    // The goal's tickets are in its row, not in rows of their own.
    assert!(!rows.contains(&"GAME-10".to_string()), "{rows:?}");
    // The office's other rows keep what they had: a landed ticket of an open
    // goal, which History leaves out.
    assert!(!rows.contains(&"GAME-4".to_string()), "{rows:?}");
}

#[tokio::test]
async fn history_pages_with_show_more() {
    let web = Web::start_with(Mode::Old).await;
    let first = panel_on(&web, "", "projects", 20).await;
    let rows = history_ids(&first);
    assert_eq!(rows.len(), 20, "{first}");
    // Newest first: the last ticket filed comes first.
    assert_eq!(rows[0], "GAME-144", "{first}");
    assert_eq!(rows[1], "GAME-143", "{first}");
    assert!(first.contains("47 · newest first"), "{first}");
    assert!(first.contains("Show more"), "{first}");
    // The button asks for twenty more, through the panel route.
    assert!(
        first.contains("data-on:click=\"$history = 40; @get('/office/panel')\""),
        "{first}"
    );

    let more = panel_on(&web, "", "projects", 40).await;
    let rows = history_ids(&more);
    assert_eq!(rows.len(), 40, "{more}");
    assert!(more.contains("$history = 60;"), "{more}");

    // The last of them: all forty-seven, the oldest filed last, and no button.
    let all = panel_on(&web, "", "projects", 60).await;
    let rows = history_ids(&all);
    assert_eq!(rows.len(), 47, "{all}");
    assert_eq!(rows[45..], ["GAME-8", "GAME-9"], "{all}");
    assert!(!all.contains("Show more"), "{all}");

    // The live stream keeps what was asked for.
    let mut stream = datastar_get_with(
        &web,
        "/office/stream",
        json!({"sel": "", "tab": "projects", "history": 40}),
    )
    .await;
    let mut text = String::new();
    while !text.contains("Show more") {
        let chunk = stream.chunk().await.unwrap().unwrap();
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(text.contains("$history = 60;"), "{text}");
}
