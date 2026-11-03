//! The conversation with a lead, Look closer and the office view against a
//! stand-in studiod: what the pages say and link to, and what the owner's
//! clicks send.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

#[path = "../src/probe.rs"]
mod probe;
use probe::{by_id, find, find_containing, has_id, named, text_of, with};

const TOKEN: &str = "0123abcd";
const NOW: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;

fn tickets() -> Value {
    let ticket = |id: &str, title: &str, state: Value, hire: &str, extra: Value| {
        let mut fields = json!({
            "id": id, "project": "shop", "title": title, "goal": "", "hire": hire,
            "state": state, "attempts": 0, "base_branch": "main", "created_at": NOW - 3_000_000,
        });
        fields
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        fields
    };
    json!([
        ticket("SHOP-1", "Cart drawer", json!("working"), "dev", json!({})),
        ticket(
            "SHOP-2",
            "Save carts between sessions",
            json!("landed"),
            "nora",
            json!({"kind": "plan", "goal": "Save carts between sessions", "version": "0.4"}),
        ),
        ticket(
            "SHOP-3",
            "Store carts in the database",
            json!("landed"),
            "dev",
            json!({"parent": "SHOP-2", "goal": "Save carts between sessions"}),
        ),
        ticket(
            "SHOP-4",
            "Bring the cart back on sign-in",
            json!("landed"),
            "dev",
            json!({"parent": "SHOP-2", "goal": "Save carts between sessions"}),
        ),
        ticket(
            "SHOP-5",
            "Add a discount field",
            json!("ready"),
            "dev",
            json!({})
        ),
        ticket(
            "SHOP-6",
            "Product cards",
            json!({"asking": {"ask": "a1"}}),
            "designer",
            json!({}),
        ),
    ])
}

/// What the stand-in studiod says of the office: the shop, which Nora leads,
/// with work at every stage, and the docs, which nobody leads.
fn office(failing: bool) -> Value {
    let check = if failing {
        json!({"state": "failed", "summary": "cargo test: 2 failed", "at": 1})
    } else {
        json!({"state": "passed", "summary": "47 tests pass", "at": 1})
    };
    json!({
        "status": {
            "projects": ["shop", "docs"], "hires": ["nora", "dev", "designer"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
                {"id": "designer", "name": "Designer", "role": "designer", "model": "sonnet"},
                {"id": "writer", "name": "Writer", "role": "writer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.59, "max_runs": 3,
        },
        "tickets": tickets(),
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
        "spend": {}, "merged": [], "today": {}, "judges": {},
        "sizes": {"SHOP-5": [12, 3]},
        "projects": [
            {"name": "shop", "lead": "nora", "path": "/work/my shop",
             "release": {"project": "shop", "version": "0.3", "state": "waiting",
                         "steps": [], "started_at": NOW - 20 * MINUTE}},
            {"name": "docs"},
        ],
        "ship_checks": {"SHOP-2": check},
        "results": {"SHOP-2": "Carts are saved on every change and come back when a shopper signs in."},
        "standups": {"shop": {"at": 1, "text": "The logo's done. Two things wait for you."}},
        "now": NOW,
    })
}

const CART_DIFF: &str = "diff --git a/src/carts.rs b/src/carts.rs\nindex 1..2 100644\n--- a/src/carts.rs\n+++ b/src/carts.rs\n@@ -1,2 +1,3 @@\n-pub struct Cart {}\n+pub struct Cart {\n+    pub lines: Vec<Line>,\n+}\n";
const SESSION_DIFF: &str = "diff --git a/src/session.rs b/src/session.rs\nindex 1..2 100644\n--- a/src/session.rs\n+++ b/src/session.rs\n@@ -0,0 +1 @@\n+pub fn restore() {}\n";

/// What the stand-in says of the conversation: two messages, and the lead
/// thinking when `thinking` is set.
fn conversation(thinking: bool) -> Value {
    json!({
        "messages": [
            {"id": 1, "at": NOW - 90 * MINUTE, "from": {"who": "owner"},
             "text": "Test first. Then make checkout the priority."},
            {"id": 2, "at": NOW - 80 * MINUTE, "from": {"who": "lead", "hire": "nora"},
             "text": "Test sent. Checkout's next:",
             "card": {"type": "plan", "goal": "SHOP-9", "title": "Checkout flow", "version": "0.5",
                      "tasks": [{"title": "Add a cart drawer", "hire": "dev"},
                                {"title": "Design the order confirmation", "hire": "designer"}]}},
            {"id": 3, "at": NOW - 70 * MINUTE, "from": {"who": "studio"}, "text": "",
             "card": {"type": "acted", "task": "SHOP-6", "title": "Product cards",
                      "action": "sent back", "note": "Too many colours."}},
        ],
        "thinking": thinking,
    })
}

fn answer(request: &Request, flags: &Flags) -> Response {
    let thinking = flags.thinking.load(Ordering::Relaxed);
    match request {
        Request::Office => Response::ok(office(flags.failing.load(Ordering::Relaxed))),
        Request::Conversation { project, .. } if project == "shop" => {
            Response::ok(conversation(thinking))
        }
        Request::Conversation { .. } => Response::ok(json!({"messages": [], "thinking": false})),
        // What git says of each: the goal's own (its branch against the base
        // it ships to, which is not the sum of what its tasks changed, as the
        // second task rewrote some of the first one's lines), its tasks', the
        // ready task's and the working task's, which changed nothing.
        Request::Diffstats { tickets } => Response::ok(
            tickets
                .iter()
                .map(|id| {
                    let stat = match id.as_str() {
                        "SHOP-2" => "2 files changed, 75 insertions(+), 6 deletions(-)",
                        "SHOP-3" => "1 file changed, 30 insertions(+), 2 deletions(-)",
                        "SHOP-4" => "1 file changed, 49 insertions(+), 6 deletions(-)",
                        "SHOP-1" => "",
                        _ => "1 file changed, 3 insertions(+), 1 deletion(-)",
                    };
                    (id.clone(), stat)
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
        Request::Diff { ticket } => Response::ok(match ticket.as_str() {
            // The goal's change holds the files of both its tasks, whose
            // worktrees are gone.
            "SHOP-2" => json!(format!("{CART_DIFF}{SESSION_DIFF}")),
            "SHOP-3" | "SHOP-5" => json!(CART_DIFF),
            "SHOP-4" => json!(SESSION_DIFF),
            // A task that changed nothing: its worktree is there, its diff empty.
            "SHOP-1" => json!(""),
            _ => Value::Null,
        }),
        Request::Events { ticket, .. } => Response::ok(match ticket.as_deref() {
            Some("SHOP-3" | "SHOP-4" | "SHOP-5") => json!([
                {"seq": 1, "ticket_id": "x", "at_ms": NOW - 50 * MINUTE, "event": {"type": "review_passed"}},
            ]),
            _ => json!([]),
        }),
        Request::Budget => Response::ok(json!({
            "limits": {"projects": {"shop": 4.0}},
            "spent_24h": {"by_ticket": {"SHOP-1": 1.5, "shop/talk": 0.25}},
        })),
        Request::Talk { project, .. } if project == "docs" => Response::err(
            "docs has no lead yet. Give it one in its conversation in the web UI, or with \
                 `studio lead docs <hire>`.",
        ),
        Request::Talk { text, .. } => Response::ok(json!({
            "id": 9, "at": 5, "from": {"who": "owner"}, "text": text,
        })),
        Request::Tell { task, .. } => Response::ok(json!({"task": task, "how": "live"})),
        _ => Response::ok(Value::Null),
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply. It
/// keeps every request it was sent.
/// What the stand-in is told to say differently.
#[derive(Default)]
struct Flags {
    /// The lead is working on a reply.
    thinking: AtomicBool,
    /// The checks of the goal ready to ship failed.
    failing: AtomicBool,
}

fn fake_studiod(socket: &Path, seen: Arc<Mutex<Vec<Request>>>, flags: Arc<Flags>) {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (seen, flags) = (Arc::clone(&seen), Arc::clone(&flags));
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: Request = serde_json::from_str(&line).unwrap();
                let mut out = serde_json::to_string(&answer(&request, &flags)).unwrap();
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
    flags: Arc<Flags>,
    dir: tempfile::TempDir,
}

impl Web {
    async fn start() -> Web {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let flags = Arc::new(Flags::default());
        fake_studiod(&socket, Arc::clone(&seen), Arc::clone(&flags));
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
            flags,
            dir,
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

    /// What studiod was sent besides a look at the office, since the last call.
    fn sent(&self) -> Vec<Request> {
        self.seen
            .lock()
            .unwrap()
            .drain(..)
            .filter(|request| *request != Request::Office)
            .collect()
    }

    /// Writes a task's run log.
    fn log(&self, task: &str, lines: &[Value]) {
        let runs = self.dir.path().join("runs");
        std::fs::create_dir_all(&runs).unwrap();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(runs.join(format!("{task}.jsonl")), text).unwrap();
    }
}

/// The text of the thread.
fn thread(html: &str) -> String {
    by_id(html, "talk-thread").text()
}

#[tokio::test]
async fn the_thread_holds_what_was_said_and_what_waits_by_time() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let thread = thread(&html);
    // The owner's words, the lead's with their plan, and what the studio did.
    assert!(
        thread.contains("Test first. Then make checkout the priority."),
        "{thread}"
    );
    assert!(thread.contains("You"), "{thread}");
    assert!(thread.contains("Nora"), "{thread}");
    assert!(thread.contains("v0.5 Checkout flow"), "{thread}");
    assert!(thread.contains("2 tasks"), "{thread}");
    assert!(thread.contains("Add a cart drawer"), "{thread}");
    assert!(
        thread.contains("Product cards was sent back: Too many colours."),
        "{thread}"
    );
    // What waits for the owner, with its plain question.
    assert!(thread.contains("Send the launch email?"), "{thread}");
    assert!(thread.contains("Designer · Product cards"), "{thread}");
    assert!(thread.contains("Ready to ship"), "{thread}");
    assert!(thread.contains("Save carts between sessions"), "{thread}");
    // The goal's size is what git says its branch adds, asked for by its plan
    // ticket: not the sum of its tasks (+79 −8), whose worktrees are gone.
    assert!(thread.contains("+75 −6"), "{thread}");
    assert!(!thread.contains("+79"), "{thread}");
    assert!(thread.contains("checks passed"), "{thread}");
    assert!(thread.contains("Publish v0.3?"), "{thread}");
    // The day's numbers. What waits for the owner is the open question, the
    // release, the finished goal and the task that passed review: the four
    // the sidebar counts for this lead.
    let today = find(&html, "aria-label", "Today").text();
    assert_eq!(today, "2 done 1 working 0 sent back 4 for you");
    // The messages come in order of time: what was said before what waits.
    let at = |words: &str| thread.find(words).unwrap_or_else(|| panic!("{words}"));
    assert!(at("Test first") < at("Checkout flow"));
    assert!(at("Checkout flow") < at("Too many colours"));
    assert!(at("Too many colours") < at("Send the launch email?"));
    // Nobody is replying.
    assert!(!thread.contains("is replying"), "{thread}");
}

#[tokio::test]
async fn the_lead_is_replying_while_it_thinks() {
    let web = Web::start().await;
    web.flags.thinking.store(true, Ordering::Relaxed);
    let html = web.page("/leads/shop").await;
    assert_eq!(by_id(&html, "talk-replying").text(), "Nora is replying…");
}

#[tokio::test]
async fn the_header_says_who_leads_where_the_project_is_and_how_far() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let head = by_id(&html, "talk-head");
    let text = head.text();
    assert!(text.contains("Talk to Nora"), "{text}");
    assert!(text.contains("Leads shop"), "{text}");
    assert!(text.contains("v0.3 waits for you"), "{text}");
    assert!(text.contains("v0.4 · 2 of 2"), "{text}");
    // A face for the lead, in its project's colour.
    assert!(head.inner_html().contains(r#"data-face="nora""#));
}

#[tokio::test]
async fn the_composer_has_chips_that_fill_it_and_posts_to_the_lead() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let composer = by_id(&html, "composer");
    assert!(composer.attr_has("data-on:submit", "/leads/shop/talk"));
    let inner = composer.inner_html();
    let chips: Vec<(String, String)> = named(inner, "button")
        .iter()
        .filter(|b| b.attr_is("type", "button"))
        .map(|b| (b.text(), b.attr("data-on:click").unwrap()))
        .collect();
    let words: Vec<&str> = chips.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        words,
        ["How's it going?", "What's next?", "Plan something new"]
    );
    // A chip puts its words in the field named `text` of `#composer`, and
    // moves to it.
    assert!(
        chips[1]
            .1
            .contains("document.querySelector('#composer [name=text]')"),
        "{chips:?}"
    );
    assert!(
        chips[1].1.contains(r#"c.value = "What's next?""#),
        "{chips:?}"
    );
    assert!(chips[1].1.contains("c.focus()"), "{chips:?}");
    let field = find(inner, "name", "text");
    assert_eq!(field.attr("placeholder").as_deref(), Some("Message Nora"));

    let said = web
        .post("/leads/shop/talk", &[("text", "  What's next?  ")])
        .await;
    // The message turns up in the thread, so no toast sits over the composer.
    assert!(!said.contains("Sent to"), "{said}");
    assert!(said.contains("#notice"), "{said}");
    assert_eq!(
        web.sent()
            .into_iter()
            .filter(|r| matches!(r, Request::Talk { .. }))
            .collect::<Vec<_>>(),
        [Request::Talk {
            project: "shop".into(),
            text: "What's next?".into()
        }]
    );
    // Nothing is sent for an empty message.
    let nothing = web.post("/leads/shop/talk", &[("text", "  ")]).await;
    assert!(nothing.contains("Write something"), "{nothing}");
    assert!(
        web.sent()
            .iter()
            .all(|r| !matches!(r, Request::Talk { .. }))
    );
    // A note sent back through the lead names the work.
    web.post(
        "/leads/shop/talk",
        &[("text", "Fewer colours."), ("about", "Save carts")],
    )
    .await;
    assert!(web.sent().contains(&Request::Talk {
        project: "shop".into(),
        text: "Send back \"Save carts\": Fewer colours.".into()
    }));
}

#[tokio::test]
async fn a_project_without_a_lead_has_nobody_to_talk_to() {
    let web = Web::start().await;
    let html = web.page("/leads/docs").await;
    assert!(!has_id(&html, "composer"));
    assert!(
        by_id(&html, "talk-head")
            .text()
            .contains("Nobody leads docs yet")
    );
    assert!(html.contains("has no lead yet"));
}

#[tokio::test]
async fn the_cards_that_wait_for_the_owner_answer_through_the_existing_routes() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    // The ask: Allow and Deny post the decision.
    let ask = by_id(&html, "ask-a1");
    assert!(ask.inner_html().contains("/asks/a1"));
    let buttons: Vec<(String, String)> = named(ask.inner_html(), "button")
        .iter()
        .map(|b| (b.text(), b.attr("value").unwrap()))
        .collect();
    assert_eq!(
        buttons,
        [
            ("Allow".into(), "approve".into()),
            ("Deny".into(), "deny".into())
        ]
    );
    // Ready to ship: a goal ships through its tasks' merge; a task is added.
    let goal = by_id(&html, "ship-SHOP-2");
    let ship = named(goal.inner_html(), "button");
    assert!(ship[0].attr_has("data-on:click", "/tickets/SHOP-3/merge"));
    let look = named(goal.inner_html(), "a");
    assert!(look[0].attr_is("href", "/leads/shop?look=SHOP-2"));
    assert_eq!(look[0].text(), "Look closer");
    let task = by_id(&html, "ship-SHOP-5");
    assert!(
        named(task.inner_html(), "button")[0].attr_has("data-on:click", "/tickets/SHOP-5/land")
    );
    assert!(task.text().contains("+12 −3"), "{}", task.text());
    // The release: Publish and Not yet.
    let release = by_id(&html, "release-shop");
    let forms = named(release.inner_html(), "form");
    assert!(forms[0].attr_has("data-on:submit", "/office/releases/approve"));
    assert!(forms[1].attr_has("data-on:submit", "/office/releases/cancel"));
    assert_eq!(forms[0].text(), "Publish");
    assert_eq!(forms[1].text(), "Not yet");
    // The plan: its tasks, and Looks good and Change it.
    let plan = find(&html, "aria-label", "A plan");
    let text = plan.text();
    assert!(
        text.contains("Add a cart drawer")
            && text.contains("Looks good")
            && text.contains("Change it")
    );
    let change = named(plan.inner_html(), "button")
        .into_iter()
        .find(|b| b.text() == "Change it")
        .unwrap();
    assert!(change.attr_has("data-on:click", "Change the plan: "));
    let approve = named(plan.inner_html(), "form")[0].clone();
    assert!(approve.attr_has("data-on:submit", "/leads/shop/talk"));
}

#[tokio::test]
async fn the_rail_says_who_is_at_work_the_milestone_and_the_spend() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let rail = by_id(&html, "talk-rail");
    let text = rail.text();
    assert!(text.contains("In shop now"), "{text}");
    // Dev has worked on the cart drawer for 25 minutes; the designer waits
    // for the owner.
    assert!(text.contains("Dev"), "{text}");
    assert!(text.contains("Cart drawer working, 25m"), "{text}");
    assert!(text.contains("Product cards waiting for you"), "{text}");
    // Each person once, and the one with nothing to do last, as free.
    assert_eq!(text.matches("Dev").count(), 1, "{text}");
    assert!(text.trim_end().contains("Writer Free"), "{text}");
    assert!(
        text.find("Writer Free") > text.find("Product cards"),
        "{text}"
    );
    // The milestone and its tasks.
    assert!(text.contains("v0.4 Save carts between sessions"), "{text}");
    assert!(text.contains("Store carts in the database"), "{text}");
    // $1.75 of the shop's own $4 limit.
    assert!(text.contains("$1.75 of $4"), "{text}");
    // A running task has a button that opens a field to tell its agent.
    let tell = find(
        rail.inner_html(),
        "data-on:submit",
        "@post('/tasks/SHOP-1/tell', {contentType: 'form'}); el.reset()",
    );
    assert_eq!(tell.name, "form");
    // The task that waits has none.
    assert!(!rail.inner_html().contains("/tasks/SHOP-6/tell"));
    // Faces, drawn for everyone.
    for hire in ["dev", "designer"] {
        assert!(!with(&html, "data-face", hire).is_empty(), "{hire}");
    }
}

#[tokio::test]
async fn telling_an_agent_reaches_studiod_as_a_tell() {
    let web = Web::start().await;
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
async fn look_closer_opens_a_goal_with_its_proof_tasks_and_files() {
    let web = Web::start().await;
    web.log(
        "SHOP-3",
        &[
            json!({"at": NOW, "event": {"type": "permission", "tool": "Edit",
                   "input": {"file_path": "/work/my shop/.studio/worktrees/shop-3/migrations/0007_carts.sql"}}}),
            json!({"at": NOW + 1, "event": {"type": "text", "text": "Going on."}}),
        ],
    );
    let html = web.page("/leads/shop?look=SHOP-2").await;
    let sheet = by_id(&html, "sheet");
    assert_eq!(sheet.attr("data-work").as_deref(), Some("SHOP-2"));
    let text = sheet.text();
    // The header: the work, its project, its tasks and lines.
    assert!(text.contains("Save carts between sessions"), "{text}");
    assert!(text.contains("shop · 2 tasks · +75 −6"), "{text}");
    // The lead's sentence.
    assert!(text.contains("Carts are saved on every change"), "{text}");
    // The proof: checks, review, and the protected path the owner allowed.
    assert!(text.contains("Checks 47 tests pass"), "{text}");
    assert!(text.contains("Review Reviewer agreed"), "{text}");
    assert!(
        text.contains("Protected paths migrations/0007_carts.sql, you allowed it"),
        "{text}"
    );
    // The tasks, all by Dev, and the files with the first one open.
    assert!(text.contains("2 tasks, all by Dev"), "{text}");
    assert!(text.contains("Store carts in the database"), "{text}");
    assert!(text.contains("2 files"), "{text}");
    let files = named(sheet.inner_html(), "details");
    let open: Vec<bool> = files
        .iter()
        .filter(|d| d.inner_html().contains("src/"))
        .map(|d| d.has("open"))
        .collect();
    assert_eq!(open, [true, false]);
    assert!(text.contains("pub fn restore() {}"), "{text}");
    // Ship, send back, try it first and the editor.
    let ship = named(sheet.inner_html(), "button")
        .into_iter()
        .find(|b| b.text() == "Ship to main")
        .expect("a ship button");
    assert!(ship.attr_has("data-on:click", "/tickets/SHOP-3/merge"));
    assert!(text.contains("Send back with a note"), "{text}");
    assert!(
        text.contains("git switch studio/goal/save-carts-between-sessions"),
        "{text}"
    );
    let editor = named(sheet.inner_html(), "a")
        .into_iter()
        .find(|a| a.text() == "Open in editor")
        .expect("an editor link");
    assert_eq!(
        editor.attr("href").as_deref(),
        Some("vscode://file/work/my%20shop")
    );
    // A way back to the conversation, two of them.
    let closing: Vec<_> = named(sheet.inner_html(), "a")
        .into_iter()
        .filter(|a| a.attr_is("aria-label", "Close"))
        .collect();
    assert_eq!(closing.len(), 2);
    assert!(closing.iter().all(|a| a.attr_is("href", "/leads/shop")));
    // The conversation is still under it.
    assert!(has_id(&html, "talk-thread"));
}

/// The tickets studiod was asked the size of, in the requests `sent`.
fn sizes_asked(sent: &[Request]) -> Vec<String> {
    sent.iter()
        .filter_map(|request| match request {
            Request::Diffstats { tickets } => Some(tickets.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[tokio::test]
async fn a_goal_whose_tasks_landed_has_its_size_and_files_from_its_own_change() {
    let web = Web::start().await;

    // HQ: the card tells what the goal's branch adds, and asks for it by the
    // goal's plan ticket, since its tasks' worktrees are gone.
    let hq = web.page("/").await;
    let cards = with(&hq, "data-card", "ship");
    let goal = cards
        .iter()
        .find(|card| {
            card.text()
                .contains("Save carts between sessions is ready to ship")
        })
        .expect("the goal's card");
    assert!(goal.text().contains("+75 −6"), "{}", goal.text());
    let asked = sizes_asked(&web.sent());
    assert!(asked.contains(&"SHOP-2".to_string()), "{asked:?}");
    assert!(!asked.contains(&"SHOP-3".to_string()), "{asked:?}");

    // Look closer: the goal's files and their lines, each file once, and the
    // size of each task.
    let html = web.page("/leads/shop?look=SHOP-2").await;
    let text = by_id(&html, "sheet").text();
    assert!(text.contains("shop · 2 tasks · +75 −6"), "{text}");
    assert!(text.contains("2 files"), "{text}");
    assert!(text.contains("src/carts.rs") && text.contains("src/session.rs"));
    assert!(text.contains("+3 −1") && text.contains("+1 −0"), "{text}");
    assert!(text.contains("+30 −2") && text.contains("+49 −6"), "{text}");
    // The change is the goal's own, not each task's put together.
    let sent = web.sent();
    let asked_diff = |ticket: &str| {
        sent.contains(&Request::Diff {
            ticket: ticket.into(),
        })
    };
    assert!(asked_diff("SHOP-2"));
    assert!(!asked_diff("SHOP-3") && !asked_diff("SHOP-4"));
}

#[tokio::test]
async fn a_goal_is_sent_back_through_its_lead_and_a_task_through_the_changes_route() {
    let web = Web::start().await;
    let goal = web.page("/leads/shop?look=SHOP-2").await;
    let form = named(by_id(&goal, "sheet").inner_html(), "form")
        .into_iter()
        .find(|f| f.attr_has("data-on:submit", "/talk"))
        .expect("a form to the lead");
    assert!(form.attr_has("data-on:submit", "/leads/shop/talk"));
    assert!(
        form.inner_html()
            .contains(r#"value="Save carts between sessions""#)
    );

    let task = web.page("/leads/shop?look=SHOP-5").await;
    let sheet = by_id(&task, "sheet");
    let form = named(sheet.inner_html(), "form")
        .into_iter()
        .find(|f| f.attr_has("data-on:submit", "/tickets/SHOP-5/changes"))
        .expect("a form to the changes route");
    assert!(form.inner_html().contains(r#"name="feedback""#));
    // A task is only added to its goal, so there is no branch to name.
    let ship = named(sheet.inner_html(), "button")
        .into_iter()
        .find(|b| b.attr_has("data-on:click", "/tickets/SHOP-5/land"))
        .expect("a land button");
    assert_eq!(ship.text(), "Ship");
    assert!(
        sheet.text().contains("git switch studio/shop-5"),
        "{}",
        sheet.text()
    );
    assert!(
        sheet.text().contains("shop · 1 task · +3 −1"),
        "{}",
        sheet.text()
    );
    let editor = named(sheet.inner_html(), "a")
        .into_iter()
        .find(|a| a.text() == "Open in editor")
        .unwrap();
    assert_eq!(
        editor.attr("href").as_deref(),
        Some("vscode://file/work/my%20shop/.studio/worktrees/shop-5")
    );
}

#[tokio::test]
async fn look_closer_opens_nothing_for_work_that_is_not_there() {
    let web = Web::start().await;
    assert!(!has_id(&web.page("/leads/shop").await, "sheet"));
    assert!(!has_id(&web.page("/leads/shop?look=").await, "sheet"));
    assert!(!has_id(&web.page("/leads/shop?look=NOPE-1").await, "sheet"));
    // Only what is safe in an address is looked for.
    assert!(!has_id(
        &web.page("/leads/shop?look=a%27b%3Cc").await,
        "sheet"
    ));
    // Another project's work is not this one's.
    assert!(!has_id(&web.page("/leads/docs?look=SHOP-5").await, "sheet"));
}

#[tokio::test]
async fn a_goal_whose_checks_failed_says_so_before_it_is_shipped() {
    let web = Web::start().await;
    web.flags.failing.store(true, Ordering::Relaxed);
    // The card says it, and Ship is not as sure of itself.
    let html = web.page("/leads/shop").await;
    let card = by_id(&html, "ship-SHOP-2");
    assert!(
        card.text().contains("checks failed: cargo test: 2 failed"),
        "{}",
        card.text()
    );
    let ship = named(card.inner_html(), "button");
    assert_eq!(ship[0].text(), "Ship anyway");
    // The sheet says it above the button, which still ships.
    let sheet = web.page("/leads/shop?look=SHOP-2").await;
    let sheet = by_id(&sheet, "sheet");
    let text = sheet.text();
    assert!(
        text.contains("Checks failed cargo test: 2 failed"),
        "{text}"
    );
    assert!(
        text.contains("The checks failed: cargo test: 2 failed. Shipping it anyway still works."),
        "{text}"
    );
    let ship = named(sheet.inner_html(), "button")
        .into_iter()
        .find(|b| b.text() == "Ship anyway")
        .expect("a ship button");
    assert!(ship.attr_has("data-on:click", "/tickets/SHOP-3/merge"));
}

#[tokio::test]
async fn the_office_has_the_floor_a_hint_and_no_old_panel() {
    let web = Web::start().await;
    let html = web.page("/office").await;
    assert!(has_id(&html, "hq-scene"));
    assert!(has_id(&html, "team-sidebar"));
    assert!(!has_id(&html, "hq-panel"));
    assert!(text_of(&html).contains("Click anyone to talk to them"));
    assert_eq!(
        named(&html, "h1")
            .iter()
            .map(|h| h.text())
            .collect::<Vec<_>>(),
        ["The office"]
    );
    // The controls: turn, zoom and What's what.
    assert!(text_of(&html).contains("What's what"));
    // Nothing is picked, so no lead's card is up.
    assert!(by_id(&html, "office-peek").inner_html().trim().is_empty());
    // A turn in the address is the corner the floor is seen from.
    let turned = web.page("/office?turn=2").await;
    assert!(
        find_containing(&turned, "div", "data-signals__ifmissing", "turn: 2")
            .has("data-signals__ifmissing")
    );
}

#[tokio::test]
async fn picking_a_sign_or_a_person_shows_their_leads_card() {
    let web = Web::start().await;
    let peek = |sel: &str| {
        let datastar = json!({"sel": sel, "turn": 0}).to_string();
        let web = &web;
        async move {
            let reply = web
                .http
                .get(format!("{}/team/office/scene", web.base))
                .query(&[("datastar", datastar)])
                .header("cookie", format!("studio_token={TOKEN}"))
                .header("datastar-request", "true")
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), 200);
            reply.text().await.unwrap()
        }
    };
    // What a pick draws at once is the floor and the card, as one patch.
    let card_of = |patch: &str| by_id(patch, "office-peek").inner_html().to_string();
    for sel in ["projects/shop", "hires/nora", "hires/dev", "tickets/SHOP-1"] {
        let patch = peek(sel).await;
        assert!(has_id(&patch, "hq-scene"), "{sel}");
        assert!(
            sel != "projects/shop" || patch.contains("data-picked"),
            "{sel}: the pick is marked"
        );
        let card = card_of(&patch);
        let text = text_of(&card);
        assert!(text.contains("Nora"), "{sel}: {text}");
        assert!(
            text.contains("Leads shop · Building")
                || text.contains("Leads shop · v0.3 waits for you"),
            "{sel}: {text}"
        );
        // Its standup, a count of what waits, and the two ways on.
        assert!(
            text.contains("The logo's done. Two things wait for you."),
            "{sel}: {text}"
        );
        let links: Vec<(String, String)> = named(&card, "a")
            .iter()
            .map(|a| (a.text(), a.attr("href").unwrap()))
            .collect();
        assert_eq!(
            links,
            [
                ("Talk to Nora".into(), "/leads/shop".into()),
                ("What's ready".into(), "/leads/shop?look=SHOP-2".into()),
            ],
            "{sel}"
        );
    }
    // Picking something no lead owns, or nothing, clears the card.
    for sel in ["", "elements/lounge", "projects/garden"] {
        let patch = peek(sel).await;
        assert!(has_id(&patch, "office-peek"), "{sel}");
        assert!(card_of(&patch).trim().is_empty(), "{sel}");
    }
    // A project nobody leads has a card called by its name.
    let docs = card_of(&peek("projects/docs").await);
    assert!(text_of(&docs).contains("Open docs"), "{}", text_of(&docs));
}

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

#[tokio::test]
async fn the_conversation_keeps_up_through_the_one_stream_of_the_team_pages() {
    let web = Web::start().await;
    // The conversation has no stream of its own.
    assert_eq!(web.get("/leads/shop/stream").await.status(), 404);
    let html = web.page("/leads/shop").await;
    assert!(!html.contains("/leads/shop/stream"), "{html}");

    let mut reply = web.get("/team/stream?at=%2Fleads%2Fshop").await;
    assert_eq!(reply.status(), 200);
    let text = patch_with(&mut reply, r#"id="talk-rail""#).await;
    // The whole page is re-sent: the frame, the header, the thread and the rail.
    for id in [
        "team",
        "team-sidebar",
        "talk-head",
        "talk-thread",
        "talk-rail",
    ] {
        assert!(
            text.contains(&format!("id=\"{id}\"")) || text.contains(&format!("id={id}")),
            "{id}: {text}"
        );
    }
    // What the owner may be typing in is the owner's: the patch has no value
    // of its own for the field, which Datastar then leaves as it was typed.
    let composer = by_id(&text, "composer");
    let field = find(composer.inner_html(), "name", "text");
    assert!(!field.has("value"), "{text}");
    assert!(field.has("data-bind:_draft"), "{text}");
    // Nothing is sent while nothing changes; the lead starting on a reply, which
    // the office doesn't show, is sent as it comes.
    web.flags.thinking.store(true, Ordering::Relaxed);
    let next = patch_with(&mut reply, "Nora is replying").await;
    assert!(has_id(&next, "talk-thread"), "{next}");
}

#[tokio::test]
async fn the_stream_draws_the_sheet_of_the_work_the_page_looks_closer_at() {
    let web = Web::start().await;
    let mut reply = web
        .get("/team/stream?at=%2Fleads%2Fshop%3Flook%3DSHOP-2")
        .await;
    let text = patch_with(&mut reply, r#"id="sheet""#).await;
    assert_eq!(
        by_id(&text, "sheet").attr("data-work").as_deref(),
        Some("SHOP-2")
    );
    assert!(
        text_of(&text).contains("Save carts between sessions"),
        "{text}"
    );
}

#[tokio::test]
async fn what_waits_together_stands_under_one_line_and_what_was_done_is_told_once() {
    let web = Web::start().await;
    let html = web.page("/leads/shop").await;
    let thread = thread(&html);
    // The question, both pieces of work and the release come one after the
    // other: one line for all four, not a line each.
    assert!(thread.contains("Four things wait for you:"), "{thread}");
    assert!(!thread.contains("is waiting for you."), "{thread}");
    assert!(!thread.contains("is done and checked."), "{thread}");
    assert_eq!(thread.matches("Ready to ship").count(), 2, "{thread}");
    // Sent back is said once, by the card: no line above it says it too.
    assert_eq!(thread.matches("was sent back").count(), 1, "{thread}");
    assert!(!thread.contains("Sent \"Product cards\" back"), "{thread}");
}

#[tokio::test]
async fn a_task_that_changed_nothing_says_so_instead_of_plus_nothing_minus_nothing() {
    let web = Web::start().await;
    let html = web.page("/leads/shop?look=SHOP-1").await;
    let text = by_id(&html, "sheet").text();
    assert!(text.contains("No changes to show"), "{text}");
    assert!(!text.contains("+0"), "{text}");
    assert!(!text.contains("−0"), "{text}");
    assert!(!text.contains("1 file"), "{text}");
}
