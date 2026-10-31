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
            "asks": 1, "running": 2, "daily_usd": 10.0, "spent_24h": 0.25,
            "projects": ["game"], "hires": ["dev"],
        }),
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
        }]),
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
        Request::Estimates => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            json!([
                {"ticket": "GAME-1", "quote_usd": 0.42, "eta_ms": now + 12 * 60_000 + 30_000},
                {"ticket": "GAME-2", "quote_usd": 0.42, "eta_ms": now + 12 * 60_000 + 30_000},
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
            "spent_24h": {"total": 2.9, "by_project": {"game": 2.9}, "by_hire": {"dev": 2.9}},
            "runs_24h": 4,
            "reserved": {"GAME-1": 0.5},
        }),
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
        Request::TestRule { .. } => json!({
            "rule": "gate, block", "scanned": 3,
            "hits": [{"ticket": "GAME-1", "at": 1, "tool": "Bash",
                      "what": "git push <b>x</b>", "level": "block"}],
        }),
        _ => Value::Null,
    }
}

fn answer(request: &Request) -> Response {
    match request {
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
fn fake_studiod(socket: &Path) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let mut out = serde_json::to_string(&answer(&request)).unwrap();
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

    let css = web.get("/static/app.css", false).await;
    assert_eq!(css.status(), 200);
    assert!(css.text().await.unwrap().contains("tailwindcss"));
}

#[tokio::test]
async fn pages_render_studio_data_escaped() {
    let web = Web::start().await;

    let board = web.get("/", true).await;
    assert_eq!(board.status(), 200);
    let html = board.text().await.unwrap();
    assert!(
        html.contains("Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{html}"
    );
    assert!(html.contains("out of budget"));
    assert!(html.contains("/board/stream"));
    // The header: spend as plain text linking to the budget, a running count,
    // and the current page marked. Phones drop only the word "today".
    assert!(html.contains("$0.25 of $10.00"), "{html}");
    assert!(
        html.contains(r#"<span class="hidden sm:inline"> today</span>"#),
        "{html}"
    );
    assert!(!html.contains("hidden shrink-0"), "{html}");
    assert!(html.contains("2 running"), "{html}");
    assert!(html.contains(r#"aria-current="page""#), "{html}");
    // The summary strip: two asks and one held ticket, one ready, one working, all five
    // lanes present, and no empty-lane placeholder.
    assert!(html.contains(r#"id="board-summary""#), "{html}");
    for count in [
        r#"data-count="running">1<"#,
        r#"data-count="needs-you">3<"#,
        r#"data-count="ready">1<"#,
    ] {
        assert!(html.contains(count), "{count}: {html}");
    }
    assert!(html.contains("$0.25 of $10.00 today"), "{html}");
    assert!(html.contains(r##"href="#lane-ready""##), "{html}");
    assert!(!html.contains("Nothing here"), "{html}");
    let needs = html.find(r#"id="lane-needs-you""#).unwrap();
    let ready = html.find(r#"id="lane-ready""#).unwrap();
    let queued = html.find(r#"id="lane-queued""#).unwrap();
    assert!(needs < ready && ready < queued, "{html}");
    // A plan ticket is tagged, and a filed ticket links to its parent.
    assert!(html.contains(">plan<"), "{html}");
    assert!(html.contains("from GAME-3"), "{html}");
    assert!(html.contains(r#"href="/tickets/GAME-3""#), "{html}");
    // The queued card shows its ETA (the held one does not).
    assert_eq!(html.matches("~13m").count(), 1, "{html}");

    let ticket = web.get("/tickets/GAME-1", true).await.text().await.unwrap();
    assert!(ticket.contains("&lt;img src=x&gt; done"), "{ticket}");
    assert!(ticket.contains("Resume"));
    assert!(ticket.contains("$0.0019"));
    // The facts are labelled chips.
    for chip in [
        "Project", "Hire", "Base", "Attempts", "Spent", "Quote", "ETA",
    ] {
        assert!(
            ticket.contains(&format!(">{chip}</span>")),
            "{chip}: {ticket}"
        );
    }
    assert!(ticket.contains(">~$0.42</span>"), "{ticket}");
    assert!(ticket.contains(">~13m</span>"), "{ticket}");
    // The live stream starts after the log the page already shows.
    assert!(
        ticket.contains("/tickets/GAME-1/stream?from=52"),
        "{ticket}"
    );

    // The timeline: human labels, relative times, escaped folded detail.
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
    // The buttons stack full width on a phone and sit in a row on desktop.
    let row = asks
        .split_once(r#"<div class="flex flex-col gap-2 sm:flex-row sm:flex-wrap">"#)
        .unwrap()
        .1;
    let row = row.split_once("</form>").unwrap().0;
    assert!(
        row.contains("min-h-10 w-full sm:min-h-0 sm:w-auto"),
        "{row}"
    );
    assert!(row.contains(">Allow</button>"), "{row}");
    assert!(row.contains(">Deny</button>"), "{row}");
    // A badge can show an ellipsis, and its title has the full text.
    assert!(
        asks.contains(r#"title="dev" class="inline-block max-w-full truncate"#),
        "{asks}"
    );

    let budget = web.get("/budget", true).await.text().await.unwrap();
    // 2.9 of 3.0 is over 90%: the project's bar turns red.
    assert!(budget.contains("bg-rose-500"), "{budget}");
    assert!(budget.contains("no limit"));
    // The headline adds the held allowance to the spend: $2.90 + $0.50.
    assert!(budget.contains("$3.40"), "{budget}");
    assert!(budget.contains("$6.60"), "{budget}");
    assert!(budget.contains("spent by finished runs"), "{budget}");
    assert!(budget.contains("held for live runs"), "{budget}");
    // An unset per-run cap is worded, and the four stat cards are gone.
    assert!(budget.contains("Per run: no cap"), "{budget}");
    assert!(!budget.contains("<dt"), "{budget}");
    // A held ticket links with its (escaped) title.
    assert!(budget.contains(r#"href="/tickets/GAME-1""#), "{budget}");
    assert!(
        budget.contains("Add a &lt;b&gt;pause&lt;/b&gt; menu"),
        "{budget}"
    );
    // Money is set in the body font, not mono.
    assert!(!budget.contains("font-mono tabular-nums"), "{budget}");

    // No page falls back to the low-contrast pairs.
    for page in [&html, &ticket, &asks, &budget] {
        assert!(!page.contains(" text-zinc-400 "), "{page}");
        assert!(!page.contains("dark:text-zinc-600"), "{page}");
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
async fn rules_are_listed_and_saved() {
    let web = Web::start().await;
    let page = web.get("/rules", true).await.text().await.unwrap();
    assert!(page.contains("good") && page.contains("broken"), "{page}");
    assert!(page.contains("missing frontmatter"));
    // Each rule has an Edit link and a Delete button in the list.
    assert!(page.contains("/rules?file=good"), "{page}");
    assert!(page.contains("/rules/good/delete?open="), "{page}");
    assert!(page.contains("Delete rule good?"), "{page}");

    let page = web
        .get("/rules?file=good", true)
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("Be &lt;i&gt;kind&lt;/i&gt;."), "{page}");
    assert!(!page.contains("<i>kind</i>"));

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
async fn deleting_a_rule_clears_the_editor_only_if_it_is_open() {
    let web = Web::start().await;
    let open = web.get("/rules?file=bad", true).await.text().await.unwrap();
    assert!(open.contains("/rules/good/delete?open=bad"), "{open}");

    let other = post_form(&web, "/rules/good/delete?open=bad", &[]).await;
    assert!(other.contains("Deleted good."), "{other}");
    assert!(!other.contains("Pick a file"), "{other}");
    assert!(other.contains("/rules/bad/delete?open=bad"), "{other}");

    let same = post_form(&web, "/rules/good/delete?open=good", &[]).await;
    assert!(same.contains("Pick a file"), "{same}");
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
    let page = web.get("/goals", true).await.text().await.unwrap();
    assert!(page.contains("Ship the &lt;i&gt;menus&lt;/i&gt;"), "{page}");
    assert!(page.contains("1 of 2 landed"), "{page}");
    // Spend includes the cancelled ticket; the chip is the goal's own branch,
    // even before anything has landed on it.
    assert!(page.contains("$1.75 spent"), "{page}");
    assert!(
        page.contains(">studio/goal/ship-the-i-menus-i</code>"),
        "{page}"
    );
    assert!(page.contains(">studio/goal/add-saves</code>"), "{page}");
    assert!(!page.contains(">master</code>"), "{page}");
    assert!(page.contains(r#"href="/tickets/GAME-3""#), "{page}");
    assert!(
        page.contains("Menu art") && page.contains("Menu sounds"),
        "{page}"
    );
    assert!(page.contains("/goals/stream"), "{page}");

    let post = |origin: &str| {
        web.http
            .post(format!("{}/goals", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", origin)
            .form(&[("project", "game"), ("hire", ""), ("text", "x")])
            .send()
    };
    let ok = post(&web.base).await.unwrap();
    assert_eq!(ok.status(), 200);
    assert!(ok.text().await.unwrap().contains("Started GAME-6."));
    assert_eq!(post("https://evil.example").await.unwrap().status(), 403);
}

#[tokio::test]
async fn the_office_shows_hires_and_takes_layout_changes() {
    let web = Web::start().await;
    assert_eq!(web.get("/office", false).await.status(), 401);

    let page = web.get("/office", true).await;
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains("<svg"), "{html}");
    assert!(html.contains("dev"));
    // "dev" has a held ticket, so it links to the asks page.
    assert!(html.contains(r#"href="/asks""#));
    assert!(html.contains("Open plan"));

    let post = |path: &str, body: String| {
        web.http
            .post(format!("{}{path}", web.base))
            .header("cookie", format!("studio_token={TOKEN}"))
            .header("origin", web.base.clone())
            .header("content-type", "application/json")
            .header("datastar-request", "true")
            .body(body)
            .send()
    };
    let reply = post("/office/preset/loft", String::new()).await.unwrap();
    assert_eq!(reply.status(), 200);
    let saved = std::fs::read_to_string(web._dir.path().join("office.json")).unwrap();
    assert!(saved.contains("bookshelf"));

    let reply = post("/office/place", r#"{"x":9,"y":9,"item":"plant"}"#.into())
        .await
        .unwrap();
    assert_eq!(reply.status(), 200);
    assert!(reply.text().await.unwrap().contains(r#"id="office""#));
    let saved = std::fs::read_to_string(web._dir.path().join("office.json")).unwrap();
    assert!(saved.contains(r#""x": 9"#));

    let reply = post("/office/place", r#"{"x":99,"y":9,"item":"plant"}"#.into())
        .await
        .unwrap();
    assert!(reply.text().await.unwrap().contains("outside"));
}

#[tokio::test]
async fn the_board_shows_asks_inline_and_lands_ready_work() {
    let web = Web::start().await;
    let html = web.get("/", true).await.text().await.unwrap();
    // The held ticket's pending ask is answerable from its card.
    assert!(html.contains(r#"id="ask-ASK-1""#), "{html}");
    assert!(html.contains("cargo test"), "{html}");
    assert!(html.contains("/asks/ASK-1"), "{html}");
    // The ready ticket has a Land button.
    assert!(html.contains("/tickets/GAME-5/land"), "{html}");
    // Long titles carry a tooltip.
    assert!(html.contains(r#"title="Add a title screen""#), "{html}");
}

#[tokio::test]
async fn the_ticket_page_puts_its_ask_under_the_title() {
    let web = Web::start().await;
    let html = web.get("/tickets/GAME-1", true).await.text().await.unwrap();
    let title = html.find("<h1").unwrap();
    let ask = html.find(r#"id="ask-ASK-1""#).unwrap();
    let timeline = html.find(r#"id="timeline""#).unwrap();
    assert!(title < ask && ask < timeline, "{html}");
    assert!(html.contains("Always allow"), "{html}");
    // The ticket id is in the page's own header, not repeated on the card.
    assert!(!html.contains(r#"href="/tickets/GAME-1""#), "{html}");
}

#[tokio::test]
async fn the_ticket_page_labels_its_goal_and_groups_its_log() {
    let web = Web::start().await;
    let html = web.get("/tickets/GAME-3", true).await.text().await.unwrap();
    assert!(html.contains(">Goal</span>"), "{html}");
    assert!(html.contains("Ship the &lt;i&gt;menus&lt;/i&gt;"), "{html}");
    // The full goal is only in the quoted title attribute, never as markup.
    assert!(!html.contains(">Ship the <i>"), "{html}");
    // The log opens at its newest entry.
    assert!(html.contains(r#"id="log-body""#), "{html}");
    assert!(html.contains("flex-col-reverse"), "{html}");
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
async fn hires_show_as_cards() {
    let web = Web::start().await;
    let html = web.get("/hires", true).await.text().await.unwrap();
    assert!(html.contains("Dev &lt;b&gt;Dana&lt;/b&gt;"), "{html}");
    assert!(html.contains("sonnet") && html.contains("opus"), "{html}");
    assert!(html.contains("$0.50 per ticket"), "{html}");
    assert!(html.contains("file_ticket"), "{html}");
    assert!(html.contains("/hires?file=dev"), "{html}");
    // What it has done: two done (landed, ready), today's spend, no failures.
    assert!(html.contains("done · "), "{html}");
    assert!(html.contains("$2.90") && html.contains("100%"), "{html}");
    // Nothing running for dev, and a hire with no history gets one quiet line.
    assert_eq!(html.matches("Idle").count(), 1, "{html}");
    assert!(html.contains("No tickets yet"), "{html}");
    assert!(!html.contains("dark:text-zinc-600"), "{html}");
    // Opening a file brings the editor back.
    let html = web.get("/hires?file=dev", true).await.text().await.unwrap();
    assert!(html.contains("<textarea"), "{html}");
}

#[tokio::test]
async fn a_goal_reads_as_in_progress_not_as_its_plan_ticket() {
    let web = Web::start().await;
    let page = web.get("/goals", true).await.text().await.unwrap();
    assert!(page.contains("in progress"), "{page}");
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
    assert!(goals.contains("/tickets/GAME-4/merge"), "{goals}");

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
