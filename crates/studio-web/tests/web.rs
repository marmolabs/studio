//! The web UI against a stand-in studiod that answers from fixed data.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, Request, Response};

/// The files `SaveFile` was asked to write, as (name, text).
static SAVED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TOKEN: &str = "0123abcd";

fn ui_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn reply(request: &Request) -> Value {
    match request {
        Request::Status => json!({
            "asks": 1, "running": 2, "tickets": {"queued": 1, "working": 2}, "daily_usd": 10.0, "spent_24h": 0.25,
            "projects": ["game"], "hires": ["dev"], "max_runs": 3,
            "team": [{"id": "dev", "role": "engineer", "model": "sonnet"},
                     {"id": "approver", "role": "approver", "model": "haiku"}],
        }),
        Request::RecentEvents { kinds, .. } => {
            let at = |minutes_ago: u64| ui_now() - minutes_ago * 60_000;
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
            let at = |minutes_ago: u64| ui_now() - minutes_ago * 60_000;
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
            let at = |minutes_ago: u64| ui_now() - minutes_ago * 60_000;
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
        Request::UpdatedAt => {
            json!({"GAME-3": ui_now() - 4 * 60_000, "GAME-5": ui_now() - 2 * 60_000})
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
            {"ticket": "GAME-1", "at": ui_now() - 60_000, "tool": "Bash",
             "what": "echo <b>hi</b>", "verdict": "denied", "reason": "not <i>needed</i>"},
            {"ticket": "GAME-2", "at": ui_now() - 120_000, "tool": "Read",
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
}

impl Mode {
    fn bare(self) -> bool {
        matches!(self, Mode::Empty | Mode::Fresh)
    }
}

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

fn answer(request: &Request, mode: Mode) -> Response {
    match request {
        Request::Status if mode == Mode::Closed => {
            let mut status = reply(request);
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
        Request::SaveFile { text, .. } if text.contains("BROKEN") => {
            Response::err("validation error: lint rules require pattern")
        }
        Request::SaveFile { name, text, .. } => {
            SAVED.lock().unwrap().push((name.clone(), text.clone()));
            Response::ok(Value::Null)
        }
        _ => Response::ok(reply(request)),
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply.
fn fake_studiod(socket: &Path, mode: Mode) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
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
    _dir: tempfile::TempDir,
}

impl Web {
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
    let light = post("light", "/goals").await.unwrap();
    assert_eq!(light.status(), 303);
    assert_eq!(light.headers()["location"], "/goals");
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
            .get(format!("{}/goals", web.base))
            .header("cookie", format!("studio_token={TOKEN}; theme={theme}"))
            .send()
    };
    let html = page("light").await.unwrap().text().await.unwrap();
    assert!(
        html.contains(r#"<html lang="en" data-theme="light">"#),
        "{html}"
    );
    let html = page("purple").await.unwrap().text().await.unwrap();
    assert!(html.contains(r#"<html lang="en">"#), "{html}");
}

#[tokio::test]
async fn pages_render_studio_data_escaped() {
    let web = Web::start().await;

    let board = web.get("/board", true).await;
    assert_eq!(board.status(), 200);
    let html = board.text().await.unwrap();
    assert!(
        html.contains("Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{html}"
    );
    assert!(html.contains("out of budget"));
    assert!(html.contains("/board/stream"));
    // The shell: a sidebar with the nav groups, the asks badge, the live
    // status and the last day's spend, and a top bar naming the page.
    assert!(html.contains("<aside"), "{html}");
    for label in [
        "Office", "Board", "Asks", "Goals", "Studio", "Hires", "Rules", "Budget",
    ] {
        assert!(html.contains(&format!("{label}<")), "{label}: {html}");
    }
    // The Office/Board switch, with the board picked.
    assert!(html.contains(r#"aria-label="View""#), "{html}");
    assert!(
        html.contains(r#"href="/board" class="inline-flex h-[26px]"#),
        "{html}"
    );
    assert!(
        html.contains(r#"$0.25<span class="text-fg-3"> of $10</span>"#),
        "{html}"
    );
    assert!(html.contains(r#"aria-label="1 waiting""#), "{html}");
    assert!(html.contains(r##"<use href="#i-inbox">"##), "{html}");
    assert!(html.contains(r#"<symbol id="s-attn""#), "{html}");
    assert!(html.contains(r#"name="theme""#), "{html}");
    assert!(html.contains("2 running"), "{html}");
    assert!(html.contains(" · 1 queued"), "{html}");
    assert!(html.contains(">Board</h1>"), "{html}");
    assert!(html.contains(">New ticket</span>"), "{html}");
    assert!(html.contains(r#"popovertarget="new-goal""#), "{html}");
    assert!(html.contains(">Let the router pick</option>"), "{html}");
    assert!(html.contains(r#"aria-current="page""#), "{html}");
    assert!(html.contains(r#"role="progressbar""#), "{html}");
    assert!(!html.contains("max-w-6xl"), "{html}");
    // The groups, in order, with their counts: two asks and one held ticket,
    // one running, one ready and two queued.
    let count = |title: &str, n: usize| {
        format!(r#"{title}<span class="font-normal text-fg-3 tabular-nums">{n}<"#)
    };
    // A ticket held for budget waits with the queue, not on the owner.
    let groups = [
        count("Needs you", 2),
        count("Running", 1),
        count("Ready to land", 1),
        count("Up next", 3),
    ];
    let at: Vec<usize> = groups
        .iter()
        .map(|g| {
            html.find(g.as_str())
                .unwrap_or_else(|| panic!("{g}: {html}"))
        })
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{html}");
    assert!(html.contains("of $10</small>"), "{html}");
    assert!(html.contains("Waiting for a free desk"), "{html}");
    // A plan at work says so instead of showing the steps.
    assert!(html.contains(">Planning</span>"), "{html}");
    assert!(html.contains(r#"href="/tickets/GAME-3""#), "{html}");
    // What landed in the last day is in the table.
    assert!(html.contains("Done, last 24h"), "{html}");
    assert!(html.contains(">2h ago<"), "{html}");

    let ticket = web.get("/tickets/GAME-1", true).await.text().await.unwrap();
    assert!(ticket.contains("&lt;img src=x&gt; done"), "{ticket}");
    assert!(ticket.contains("Resume"));
    assert!(ticket.contains("$0.0019"));
    assert_eq!(ticket.matches("<h1").count(), 1, "{ticket}");
    assert!(ticket.contains(r#"aria-label="Breadcrumb""#), "{ticket}");
    assert!(!ticket.contains("← Board"), "{ticket}");
    // The rail: labelled facts, the spend against the ticket's cap, a branch
    // and a try once it has run, and its stages.
    for label in [
        "Status", "Hire", "Project", "Branch", "Try", "Spent", "Progress",
    ] {
        assert!(ticket.contains(&format!(">{label}</")), "{label}: {ticket}");
    }
    assert!(ticket.contains(" of $2<"), "{ticket}");
    assert!(ticket.contains(">studio/game-1<"), "{ticket}");
    assert!(ticket.contains(">2 of 3<"), "{ticket}");
    // The top bar exports the ticket; the menu resumes or cancels it.
    assert!(
        ticket.contains(r#"href="/tickets/GAME-1/export""#),
        "{ticket}"
    );
    assert!(ticket.contains("Cancel ticket"), "{ticket}");
    assert!(!ticket.contains(">Pause<"), "{ticket}");
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

    let asks = web.get("/asks", true).await.text().await.unwrap();
    assert!(asks.contains("no rule allows it"));
    assert!(asks.contains("cargo test"));
    // Each card names its ticket and links to it; a question is answered.
    assert!(
        asks.contains("GAME-1</span> · Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{asks}"
    );
    assert!(asks.contains(r#"href="/tickets/GAME-1""#), "{asks}");
    assert!(asks.contains("Allow</button>"), "{asks}");
    assert!(asks.contains("Deny</button>"), "{asks}");
    assert!(asks.contains("Answer</button>"), "{asks}");
    assert!(asks.contains("Always allow"), "{asks}");

    let budget = web.get("/budget", true).await.text().await.unwrap();
    assert!(budget.contains("<table"), "{budget}");
    // Spent of the daily limit, what live runs hold, and the other limits.
    assert!(
        budget.contains("$2.90") && budget.contains("of $10"),
        "{budget}"
    );
    assert!(
        budget.contains("$0.50") && budget.contains("1 run, released when it finishes"),
        "{budget}"
    );
    assert!(budget.contains("$2 a ticket · game $3"), "{budget}");
    // The last 7 days: six of $1 and today's $2.90.
    assert!(
        budget.contains("$8.90") && budget.contains("$1.27 a day on average"),
        "{budget}"
    );
    // Bars on a dollar axis up to the daily limit, today's last.
    assert!(
        budget.contains("Daily limit") && budget.contains(">$7.50<"),
        "{budget}"
    );
    assert!(budget.contains("height:29.0%"), "{budget}");
    assert!(budget.contains(">Today<"), "{budget}");
    // Each bar is a day, by its date.
    assert!(budget.contains(r#"title="$1.00 on Oct 18""#), "{budget}");
    assert!(budget.contains(r#"title="$2.90 today so far""#), "{budget}");
    assert!(budget.contains(">Oct <"), "{budget}");
    // Runs and spend by hire and model.
    assert!(budget.contains(">dev<"), "{budget}");
    assert!(budget.contains(">sonnet<"), "{budget}");
    // The costliest ticket links with its (escaped) title and its outcome: a
    // budget hold, which reads as on hold.
    assert!(budget.contains(">On hold<"), "{budget}");
    assert!(budget.contains(r#"href="/tickets/GAME-1""#), "{budget}");
    assert!(
        budget.contains("Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{budget}"
    );
    // Money is set in the body font, not mono.
    assert!(!budget.contains("font-mono tabular-nums"), "{budget}");

    // Pages use the theme tokens, not raw palette colours.
    for page in [&html, &ticket, &asks, &budget] {
        assert!(!page.contains("zinc-"), "{page}");
        assert!(!page.contains("dark:"), "{page}");
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
    assert!(page.contains(">Save<"), "{page}");

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
    assert!(!page.contains(">Save<"), "{page}");
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
async fn goals_show_what_they_filed() {
    let web = Web::start().await;
    let page = web
        .get("/goals?open=GAME-3", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("Ship the &lt;i&gt;menus&lt;/i&gt;"), "{page}");
    assert!(page.contains("1 of 2"), "{page}");
    // Spend includes the cancelled ticket; the branch is the goal's own,
    // even before anything has landed on it.
    assert!(page.contains("$1.75"), "{page}");
    assert!(page.contains(">studio/goal/ship-the-i-menus-i<"), "{page}");
    assert!(page.contains(">studio/goal/add-saves<"), "{page}");
    // Updated is the latest change across the plan and its tickets.
    assert!(page.contains("2m ago") && page.contains("Queued"), "{page}");
    // The open goal nests its plan and tickets.
    assert!(page.contains(r#"href="/tickets/GAME-3""#), "{page}");
    assert!(
        page.contains("Menu art") && page.contains("Menu sounds"),
        "{page}"
    );
    assert!(page.contains("/goals/stream?open=GAME-3"), "{page}");
    // A value that isn't a goal never reaches the stream's expression.
    let page = web
        .get("/goals?open=x%27);alert(1);(%27", true)
        .await
        .text()
        .await
        .unwrap();
    let inits: Vec<&str> = page
        .split("data-init=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    assert!(inits.iter().all(|i| !i.contains("alert")), "{inits:?}");
    assert!(page.contains("/goals/stream?open=GAME-7"), "{page}");
    // Without a choice, the newest goal still under way is open.
    let page = web.get("/goals", true).await.text().await.unwrap();
    assert!(page.contains(r#"href="/tickets/GAME-7""#), "{page}");
    assert!(!page.contains("Menu art"), "{page}");

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
async fn the_office_draws_the_floor_with_its_pins() {
    let web = Web::start().await;
    assert_eq!(web.get("/", false).await.status(), 401);

    let html = web.get("/", true).await.text().await.unwrap();
    // The floor is one drawing, sized by its own box, with things to pick.
    assert!(html.contains(r#"<svg class="iso""#), "{html}");
    assert!(html.contains(r#"id="hq-scene""#), "{html}");
    assert!(html.contains(r#"class="scene""#), "{html}");
    assert!(html.contains("--ar: "), "{html}");
    assert!(html.contains(r#"data-sel="tickets/GAME-1""#), "{html}");
    assert!(html.contains(r#"data-sel="elements/board""#), "{html}");
    // One handler on the floor picks whatever has a `data-sel`.
    assert!(html.contains("closest('[data-sel]')"), "{html}");
    assert!(html.contains("@get('/office/panel')"), "{html}");
    // Held at a desk: a pin with the project's colour and an escaped title.
    assert!(
        html.contains(
            r#"<button type="button" class="tagc held" data-sel="tickets/GAME-1" data-pj="0""#
        ),
        "{html}"
    );
    assert!(
        html.contains(r#"<span class="t">Add a &lt;b&gt;pause&lt;/b&gt; menu</span>"#),
        "{html}"
    );
    assert!(!html.contains(">Add a <b>"), "{html}");
    assert!(html.contains(r#"<span class="zz">II</span>"#), "{html}");
    // Ready work lands from its pin; queued work and planning are quiet chips.
    assert!(
        html.contains(r#"<div class="ready" data-pj="0">"#),
        "{html}"
    );
    assert!(html.contains("@post('/tickets/GAME-5/land')"), "{html}");
    assert!(
        html.contains(r#"class="tagc quiet" data-sel="elements/queue""#),
        "{html}"
    );
    assert!(
        html.contains(r#"class="tagc quiet" data-sel="elements/board""#),
        "{html}"
    );
    // Hints name each game element and show on hover or with "What's what".
    assert!(
        html.contains(r#"<div class="pin hint" data-for="board""#),
        "{html}"
    );
    assert!(html.contains("<span>Whiteboard · goals</span>"), "{html}");
    assert!(
        html.contains("What&#39;s what") || html.contains("What's what"),
        "{html}"
    );
    assert!(html.contains("data-class:show-hints"), "{html}");
    assert!(html.contains("$zoom"), "{html}");
    // Only hints name what they are for.
    assert!(!html.contains(r#"data-for="""#), "{html}");
    // Open: no sign, and nothing dims.
    assert!(!html.contains("closed-sign"), "{html}");
    assert!(!html.contains(r#"class="scene closed""#), "{html}");
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
    assert!(html.contains(">All projects<"), "{html}");
    assert!(html.contains("$focus = 0"), "{html}");
    assert!(html.contains(r#"<i class="pjd pj-0"></i>"#), "{html}");
    assert!(html.contains(r#"<span class="n">5</span>"#), "{html}");
    // One ask and no pause held for the owner; two at work; none shipped
    // today (GAME-4 landed two hours ago, which is still within 24 hours).
    assert!(html.contains(r#"class="stat needs""#), "{html}");
    assert!(html.contains(r#"<span class="ping"></span>"#), "{html}");
    assert!(html.contains("<b>1</b>"), "{html}");
    assert!(
        html.contains(r#"title="Landed in the last 24 hours""#),
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
    let board = web.get("/board", true).await.text().await.unwrap();
    assert!(board.contains(r#"aria-label="Studio""#), "{board}");
    assert!(board.contains(r#"href="/""#), "{board}");
    assert!(!board.contains(r#"id="hq-scene""#), "{board}");
}

#[tokio::test]
async fn the_corners_track_projects_and_tell_the_latest() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    // GAME-3's goal has one of its two tickets landed; the other is ready.
    assert!(html.contains(r#"id="hq-corners""#), "{html}");
    assert!(html.contains(r#"<span class="num">1/2</span>"#), "{html}");
    assert!(
        html.contains("width: 50%; background: var(--pj-0)"),
        "{html}"
    );
    assert!(html.contains(r#"<span class="flag needs""#), "{html}");
    // The live feed: the newest events in words, and a way to the Log tab.
    assert!(html.contains(r#"<ol class="mini-feed">"#), "{html}");
    assert!(html.contains("<b>GAME-1</b> needs your say"), "{html}");
    assert!(html.contains("$tab = 'log'"), "{html}");
}

#[tokio::test]
async fn a_closed_office_shows_its_sign() {
    let web = Web::start_with(Mode::Closed).await;
    let html = web.get("/", true).await.text().await.unwrap();
    assert!(html.contains(r#"class="scene closed""#), "{html}");
    assert!(html.contains(r#"class="closed-sign""#), "{html}");
    assert!(
        html.contains("Office closed · nothing new starts"),
        "{html}"
    );
}

/// The opening tag of the element whose tag contains `needle`.
fn tag_with<'a>(html: &'a str, open: &str, needle: &str) -> &'a str {
    let at = html.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    let from = html[..at]
        .rfind(open)
        .unwrap_or_else(|| panic!("no {open} before {needle}"));
    // The tag ends at the first `>` outside a quoted value.
    let mut quote = None;
    let mut to = from;
    for (i, c) in html[from..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => {
                to = from + i;
                break;
            }
            _ => {}
        }
    }
    &html[from..=to]
}

/// Whether the tab pane for `name` is shown when the page first arrives.
fn pane_shown(html: &str, name: &str) -> bool {
    let tag = tag_with(html, "<div", &format!("data-show=\"$tab == '{name}'\""));
    !tag.contains("display: none")
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
    let tab = tag_with(&html, "<button", "data-on:click=\"$tab = 'money'\"");
    assert!(tab.contains(r#"aria-selected="true""#), "{tab}");

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
    assert!(
        html.contains(r#"data-sel="elements/coffee" data-picked="""#),
        "{html}"
    );
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
    let drawing = &html[html.find("<svg class=\"iso\"").unwrap()..];
    let drawing = &drawing[..drawing.find("</svg>").unwrap()];
    assert!(
        drawing.contains(r#"data-sel="tickets/GAME-1" data-picked="""#),
        "{drawing}"
    );
    assert_eq!(drawing.matches("data-picked").count(), 1, "{drawing}");
    // Its label is pressed, and no other is.
    let label = tag_with(&html, "<button", r#"data-sel="tickets/GAME-1" data-pj"#);
    assert!(label.contains(r#"aria-pressed="true""#), "{label}");
    assert_eq!(html.matches(r#"aria-pressed="true""#).count(), 1, "{html}");

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
    assert!(!hire.contains(">Latest<"), "{hire}");

    // The approver lists its latest calls, escaped, each linked to its
    // ticket; a denial says why.
    let approver = panel(&web, "hires/approver").await;
    assert!(approver.contains(">Latest<"), "{approver}");
    assert!(
        approver.contains(">echo &lt;b&gt;hi&lt;/b&gt;<"),
        "{approver}"
    );
    assert!(!approver.contains("><b>hi"), "{approver}");
    assert!(approver.contains("Denied <code"), "{approver}");
    assert!(
        approver.contains(": not &lt;i&gt;needed&lt;/i&gt;<"),
        "{approver}"
    );
    assert!(approver.contains("Allowed <code"), "{approver}");
    assert!(approver.contains(r#"href="/tickets/GAME-2""#), "{approver}");

    // A merged goal, an unknown ticket and a forged key show the tabs.
    for sel in [
        "goals/GAME-8",
        "tickets/NOPE-1",
        "tickets/GAME-1'); alert(1)//",
    ] {
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
    assert!(text.contains(r#"class="tagc held""#), "{text}");
    assert!(text.contains("@post('/tickets/GAME-5/land')"), "{text}");
}

#[tokio::test]
async fn the_board_shows_asks_inline_and_lands_ready_work() {
    let web = Web::start().await;
    let html = web.get("/board", true).await.text().await.unwrap();
    // Each ask is a row that says what it wants and can be answered there.
    assert!(html.contains("Wants to run <span"), "{html}");
    assert!(html.contains(">cargo test<"), "{html}");
    assert!(html.contains(">no rule allows it<"), "{html}");
    assert!(html.contains("/asks/ASK-1"), "{html}");
    assert!(html.contains(r#"value="approve""#), "{html}");
    // A question opens its ticket to be answered.
    assert!(html.contains(">Answer</a>"), "{html}");
    // The held ticket says why and can be resumed.
    assert!(html.contains(">out of budget<"), "{html}");
    assert!(html.contains("/tickets/GAME-1/resume"), "{html}");
    // The ready ticket has a Land button.
    assert!(html.contains("/tickets/GAME-5/land"), "{html}");
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
    assert!(html.contains(">Goal</dt>"), "{html}");
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
    assert!(html.contains("<textarea"), "{html}");
    assert!(html.contains("Delete this hire?"), "{html}");
    assert!(html.contains("&lt;b&gt;Dana&lt;/b&gt;"), "{html}");
}

#[tokio::test]
async fn a_goal_reads_as_in_progress_not_as_its_plan_ticket() {
    let web = Web::start().await;
    let page = web.get("/goals", true).await.text().await.unwrap();
    assert!(page.contains("Running</span>"), "{page}");
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
    let goals = web.get("/goals", true).await.text().await.unwrap();
    // Merge waits until a goal is done; a merged goal shows a quiet label.
    assert!(!goals.contains("/tickets/GAME-4/merge"), "{goals}");
    assert!(!goals.contains("/tickets/GAME-10/merge"), "{goals}");
    assert_eq!(goals.matches("merged into master").count(), 1, "{goals}");
    let done = web
        .get("/tickets/GAME-10", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(done.contains("Merged into master."), "{done}");
    assert!(!done.contains("Merge into master"), "{done}");
    // Finished first time: the first try, not the zeroth.
    assert!(done.contains(">1 of 3<"), "{done}");
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
        assert!(floor.contains(r#"<svg class="iso""#), "{floor}");
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
        home.contains("data-effect=\"$sel; $turn; @get('/office/stream')\""),
        "{home}"
    );
}

#[tokio::test]
async fn the_view_controls_turn_the_floor_by_buttons_and_keys() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    for (label, step) in [("Turn left", "$turn + 3"), ("Turn right", "$turn + 1")] {
        let tag = tag_with(&html, "<button", &format!(r#"aria-label="{label}""#));
        assert!(tag.contains(step), "{label}: {tag}");
        assert!(tag.contains("@get('/office/scene')"), "{label}: {tag}");
    }
    // Q and E anywhere on the page, but not while typing or with a modifier.
    let keys = tag_with(&html, "<div", "data-on:keydown__window");
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
    let wrap = tag_with(&html, "<div", r#"id="hq-wrap""#);
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
    assert!(html.contains(">Start here<"), "{html}");
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
    assert!(html.contains(">Start here<"), "{html}");
    assert!(html.contains("Pin a goal on the whiteboard"), "{html}");
    assert!(!html.contains("$sel = 'elements/add-project'"), "{html}");
    // Once there is work, the list is gone.
    let busy = Web::start().await;
    let html = busy.get("/", true).await.text().await.unwrap();
    assert!(!html.contains(">Start here<"), "{html}");
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
    assert!(add.contains("Projects now"), "{add}");
    assert!(add.contains("No projects yet."), "{add}");
    assert!(!add.contains("Coming soon"), "{add}");

    // With projects, each shows with its colour dot.
    let busy = Web::start().await;
    let add = panel(&busy, "elements/add-project").await;
    assert!(
        add.contains(r#"<i class="pjd pj-0"></i><b class="font-medium">game</b>"#),
        "{add}"
    );

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
            hire.contains(&format!(r#"<option value="{option}">"#)),
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
