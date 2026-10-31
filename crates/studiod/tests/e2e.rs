//! A whole ticket through the daemon with a scripted runtime: claim, run,
//! ask, checks, review, land.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use studio_config::StudioPaths;
use studio_runtime::{Cost, FailureKind, FakeRuntime, Outcome, RunEvent, Runtime};
use studiod::api::{FileKind, Request, Response};
use studiod::{CiWatcher, Studio};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Setup {
    _dirs: (TempDir, TempDir),
    repo: std::path::PathBuf,
    paths: StudioPaths,
}

/// A game repo and a studio home that knows it, with one hire on the fake runtime.
fn setup(verify: &str) -> Setup {
    setup_with(verify, "daily_usd = 5.0", "")
}

/// Same, with custom `[budget]` lines and any extra studio.toml text.
fn setup_with(verify: &str, budget: &str, extra: &str) -> Setup {
    let repo_dir = tempfile::tempdir().unwrap();
    let repo = repo_dir.path().to_path_buf();
    git(&repo, &["init", "-q", "-b", "master"]);
    for (k, v) in [
        ("user.name", "Owner"),
        ("user.email", "owner@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&repo, &["config", k, v]);
    }
    fs::write(repo.join("README.md"), "game\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "Initial"]);
    fs::write(repo.join(".studio.toml"), format!("verify = [{verify}]\n")).unwrap();

    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join("studio.toml"),
        format!(
            "[budget]\n{budget}\n\n[concurrency]\nmax_runs = 2\n\n\
             [[projects]]\nname = \"game\"\npath = \"{}\"\nintegration_branch = \"master\"\n\n{extra}\n",
            repo.display()
        ),
    )
    .unwrap();
    fs::create_dir_all(home.path().join("hires")).unwrap();
    fs::write(
        home.path().join("hires/dev.md"),
        "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nYou write code.\n",
    )
    .unwrap();
    let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
    Setup {
        repo,
        paths,
        _dirs: (repo_dir, home),
    }
}

fn script() -> Vec<RunEvent> {
    vec![
        RunEvent::Started {
            session: "s1".into(),
            model: Some("fake-1".into()),
        },
        RunEvent::Permission {
            request_id: "p1".into(),
            tool: "Bash".into(),
            input: json!({ "command": "touch level.txt" }),
        },
        RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost::default(),
            summary: Some("Added the level".into()),
        },
    ]
}

fn studio(setup: &Setup, runtime: FakeRuntime) -> Arc<Studio> {
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(runtime));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

async fn ok(studio: &Arc<Studio>, request: Request) -> Value {
    let Response { ok, data, error } = studio.handle(request).await;
    assert!(ok, "request failed: {error:?}");
    data
}

/// Polls until `check` passes. The limit only catches a hang: a step that takes
/// a second on an idle machine can take far longer on a busy one.
async fn until(mut check: impl AsyncFnMut() -> bool) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never became true");
}

async fn state_of(studio: &Arc<Studio>, id: &str) -> Value {
    let tickets = ok(studio, Request::Tickets).await;
    tickets
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()["state"]
        .clone()
}

fn create(title: &str) -> Request {
    create_in(title, "First level")
}

fn create_in(title: &str, goal: &str) -> Request {
    Request::CreateTicket {
        project: "game".into(),
        title: title.into(),
        goal: goal.into(),
        hire: "dev".into(),
        base: None,
        detail: String::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_goes_from_goal_to_landed() {
    let setup = setup(r#"["sh", "-c", "test -f README.md"]"#);
    let runtime = FakeRuntime::new(script());
    let studio = studio(&setup, runtime.clone());
    let ticket = ok(&studio, create("Add a level")).await;
    assert_eq!(ticket["id"], "GAME-1");
    studio.spawn_scheduler();

    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    assert_eq!(state_of(&studio, "GAME-1").await["asking"]["ask"], "a1");
    let asks = ok(&studio, Request::Asks).await;
    assert_eq!(asks[0]["tool"], "Bash");
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: true,
            message: None,
            answers: None,
        },
    )
    .await;

    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let spec = &runtime.specs()[0];
    assert!(spec.prompt.contains("Add a level"));
    assert_eq!(
        spec.append_system_prompt.as_deref(),
        Some("You write code.")
    );
    assert!(spec.cwd.ends_with(".studio/worktrees/game-1"));
    let diff = ok(
        &studio,
        Request::Diff {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!(diff.is_string());

    let landed = ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert_eq!(landed["branch"], "studio/goal/first-level");
    assert_eq!(state_of(&studio, "GAME-1").await, "landed");
    assert_eq!(git(&setup.repo, &["branch", "--show-current"]), "master");

    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap();
    assert!(log.contains("checks passed"));

    // The landed branch is merged into its goal branch, so gc deletes it.
    assert!(git(&setup.repo, &["branch", "--list", "studio/game-1"]).contains("game-1"));
    let removed = ok(&studio, Request::Gc).await;
    assert_eq!(removed.as_array().unwrap().len(), 1);
    assert_eq!(git(&setup.repo, &["branch", "--list", "studio/game-1"]), "");

    // The fake runtime commits nothing, so give the goal some work of its own:
    // landed, but not in the owner's branch yet.
    let goal = "studio/goal/first-level";
    let tree = git(&setup.repo, &["rev-parse", &format!("{goal}^{{tree}}")]);
    let work = git(
        &setup.repo,
        &["commit-tree", &tree, "-p", goal, "-m", "work"],
    );
    git(
        &setup.repo,
        &["update-ref", &format!("refs/heads/{goal}"), &work],
    );
    let before = ok(&studio, Request::MergedTickets).await;
    assert_eq!(before, serde_json::json!([]));

    // Merging the goal brings the work into the owner's own branch.
    let merged = ok(
        &studio,
        Request::Merge {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert_eq!(merged["into"], "master");
    assert_eq!(git(&setup.repo, &["rev-parse", "HEAD"]), merged["sha"]);
    let after = ok(&studio, Request::MergedTickets).await;
    assert_eq!(after, serde_json::json!(["GAME-1"]));
    git(
        &setup.repo,
        &[
            "merge-base",
            "--is-ancestor",
            "studio/goal/first-level",
            "master",
        ],
    );

    // More work on the goal takes it back out of the owner's branch.
    let more = git(
        &setup.repo,
        &["commit-tree", &tree, "-p", goal, "-m", "more"],
    );
    git(
        &setup.repo,
        &["update-ref", &format!("refs/heads/{goal}"), &more],
    );
    let moved = ok(&studio, Request::MergedTickets).await;
    assert_eq!(moved, serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_ask_stops_the_ticket() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: false,
            message: None,
            answers: None,
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "denied").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_with_a_note_refuses_the_call_and_the_run_carries_on() {
    let setup = setup("");
    let runtime = FakeRuntime::new(script());
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: false,
            message: Some("use the Edit tool".into()),
            answers: None,
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(matches!(
        &runtime.received()[0],
        studio_runtime::Control::Deny { message, .. }
            if message == "The owner declined this: use the Edit tool"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blank_note_still_stops_the_ticket() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: false,
            message: Some("  ".into()),
            answers: None,
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "denied").await;
}

/// Approves every open ask.
async fn approve_all(studio: &Arc<Studio>) {
    let asks = ok(studio, Request::Asks).await;
    for ask in asks.as_array().unwrap() {
        ok(studio, decide(ask, true)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_goals_next_ticket_starts_from_the_goal_branch() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("First part")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    // The fake runtime can't edit files, so plant the first ticket's work.
    let first = setup.repo.join(".studio/worktrees/game-1");
    fs::write(first.join("first.txt"), "one\n").unwrap();
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;

    ok(&studio, create("Second part")).await;
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    let second = setup.repo.join(".studio/worktrees/game-2");
    assert!(second.join("first.txt").exists());
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-2.jsonl")).unwrap();
    assert!(log.contains("started from studio/goal/first-level"));
    // The first ticket's own run did not start from a goal branch.
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap();
    assert!(!log.contains("started from"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_goal_runs_one_ticket_at_a_time() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create_in("One", "Alpha")).await;
    ok(&studio, create_in("Two", "Alpha")).await;
    ok(&studio, create_in("Other", "Beta")).await;
    studio.spawn_scheduler();
    // max_runs is 2: Alpha's first ticket and Beta's run side by side.
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 2).await;
    assert!(state_of(&studio, "GAME-3").await["asking"].is_object());
    assert_eq!(state_of(&studio, "GAME-2").await, "queued");

    // Beta finishes and frees a slot, but Alpha's second ticket still waits:
    // the first is only ready, not landed.
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    until(async || state_of(&studio, "GAME-3").await == "ready").await;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(state_of(&studio, "GAME-2").await, "queued");

    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-2").await["asking"].is_object()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_ticket_frees_its_goal() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create_in("One", "Alpha")).await;
    ok(&studio, create_in("Two", "Alpha")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    assert_eq!(state_of(&studio, "GAME-2").await, "queued");
    ok(
        &studio,
        Request::Cancel {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-2").await["asking"].is_object()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tickets_without_a_goal_never_block_each_other() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create_in("One", "")).await;
    ok(&studio, create_in("Two", "")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_land_leaves_unreviewed_work_for_the_owner() {
    let setup = setup("");
    write_project(&setup, "auto_land = true\n");
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.01, true, false)]));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(!run_log(&setup).contains("auto-landed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_land_lands_a_ready_ticket_on_its_goal_branch() {
    let setup = setup("");
    write_project(&setup, "auto_land = true\n");
    add_reviewer(&setup, "fake-2");
    let reviewer = FakeRuntime::new(vec![reply("VERDICT: PASS")]);
    let studio = studio_with(&setup, FakeRuntime::new(script()), reviewer);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "landed").await;
    until(async || run_log(&setup).contains("auto-landed on studio/goal/first-level")).await;
    // Nothing reached the owner's branch.
    git(
        &setup.repo,
        &["rev-parse", "--verify", "studio/goal/first-level"],
    );
    assert_eq!(
        git(&setup.repo, &["log", "--oneline", "master"])
            .lines()
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn without_auto_land_a_ticket_waits_as_ready() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert_eq!(state_of(&studio, "GAME-1").await, "ready");
}

#[tokio::test(flavor = "multi_thread")]
async fn held_for_budget_tickets_resume_when_a_run_is_funded_again() {
    // Checks always fail, so a ticket keeps wanting runs of $0.40.
    let setup = setup_with(
        r#"["sh", "-c", "exit 1"]"#,
        "daily_usd = 1.0\nticket_usd = 0.5",
        "",
    );
    let runtime = FakeRuntime::new(vec![finished(0.4, true, false)]);
    let studio = studio(&setup, runtime);
    // Held by its own cap: two runs spent $0.80 of the ticket's $0.50.
    ok(&studio, create_in("Capped", "Alpha")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "budget").await;
    // Held by the studio's daily limit: the day's $1.00 is gone.
    ok(&studio, create_in("Broke", "Beta")).await;
    until(async || state_of(&studio, "GAME-2").await["held"]["reason"] == "budget").await;
    assert_eq!(studio.resume_budget_held().unwrap(), 0);

    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    fs::write(&config, text.replace("daily_usd = 1.0", "daily_usd = 50.0")).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    // The scheduler may get there first; either way only GAME-2 resumes.
    let _ = studio.resume_budget_held().unwrap();
    let resumed = |events: Vec<Value>| events.iter().any(|e| e["type"] == "resumed");
    assert!(resumed(events_of(&studio, "GAME-2").await));
    assert!(!resumed(events_of(&studio, "GAME-1").await));
    assert_eq!(
        state_of(&studio, "GAME-1").await["held"]["reason"],
        "budget"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_ids_continue_after_a_restart() {
    let setup = setup("");
    {
        use studio_core::event::Event;
        use studio_core::ids::{AskId, HireId, ProjectId, RunId, TicketId};
        let mut store = studio_store::Store::open(setup.paths.data_dir.join("studio.db")).unwrap();
        let id = TicketId::new("OLD-1");
        let events = [
            Event::Created {
                id: id.clone(),
                project: ProjectId::new("game"),
                title: "Old".into(),
                goal: "Old".into(),
                hire: HireId::new("dev"),
                base_branch: "master".into(),
                created_at: 1,
                kind: Default::default(),
                parent: None,
                detail: String::new(),
                source: None,
            },
            Event::Claimed,
            Event::RunStarted {
                run: RunId::new("r"),
            },
            Event::AskOpened {
                ask: AskId::new("a7"),
            },
            Event::AskResolved {
                ask: AskId::new("a7"),
                approved: true,
            },
            Event::Cancelled,
        ];
        for event in &events {
            store.append(&id, 2, event).unwrap();
        }
    }
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("New")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    assert_eq!(ok(&studio, Request::Asks).await[0]["id"], "a8");
}

#[tokio::test(flavor = "multi_thread")]
async fn failing_checks_send_the_work_back_then_fail() {
    let setup = setup(r#"["sh", "-c", "echo broken; exit 1"]"#);
    let runtime = FakeRuntime::new(vec![RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("s1".into()),
        cost: Cost::default(),
        summary: None,
    }]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Break things")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "checks").await;
    // Three attempts in total; the retries carry the check output and resume the session.
    let specs = runtime.specs();
    assert_eq!(specs.len(), 3);
    assert!(specs[1].prompt.contains("broken"));
    assert_eq!(specs[1].resume.as_deref(), Some("s1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_holds_work_that_was_waiting() {
    let setup = setup("");
    let first = studio(&setup, FakeRuntime::new(script()));
    ok(&first, create("Add a level")).await;
    first.spawn_scheduler();
    until(async || {
        !ok(&first, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;

    let second = studio(&setup, FakeRuntime::new(script()));
    assert_eq!(second.recover().unwrap(), 1);
    assert_eq!(
        state_of(&second, "GAME-1").await["held"]["reason"],
        "paused"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn asks_say_when_they_were_asked() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    let asks = ok(&studio, Request::Asks).await;
    assert!(asks[0]["at_ms"].as_u64().unwrap() >= before, "{asks}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_ticket_is_held_until_it_resumes() {
    let setup = setup("");
    let (runtime, _finish) = FakeRuntime::new(vec![
        RunEvent::Started {
            session: "s1".into(),
            model: None,
        },
        finished(0.0, true, false),
    ])
    .held();
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Add a level")).await;
    let pause = || Request::Pause {
        ticket: "GAME-1".into(),
    };
    // Nothing runs yet, so there is nothing to pause.
    assert!(!studio.handle(pause()).await.ok);
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "working").await;
    ok(&studio, pause()).await;
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "paused").await;
    assert!(run_log(&setup).contains("paused by the owner"));
    ok(
        &studio,
        Request::Resume {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    // The scheduler gives the resumed ticket a new run.
    until(async || runtime.specs().len() == 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_answers_requests() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let socket = studio.socket_path();
    tokio::spawn(studiod::serve(Arc::clone(&studio)));
    until(async || socket.exists()).await;
    let response =
        tokio::task::spawn_blocking(move || studiod::api::call(&socket, &Request::Status).unwrap())
            .await
            .unwrap();
    assert!(response.ok);
    assert_eq!(response.data["projects"][0], "game");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_projects_and_hires_are_rejected() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let bad = studio
        .handle(Request::CreateTicket {
            project: "nope".into(),
            title: "x".into(),
            goal: String::new(),
            hire: "dev".into(),
            base: None,
            detail: String::new(),
        })
        .await;
    assert!(!bad.ok);
    let bad = studio
        .handle(Request::CreateTicket {
            project: "game".into(),
            title: "x".into(),
            goal: String::new(),
            hire: "ghost".into(),
            base: None,
            detail: String::new(),
        })
        .await;
    assert!(!bad.ok);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_keeps_its_detail() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let made = studio
        .handle(Request::CreateTicket {
            project: "game".into(),
            title: "Add a pause menu".into(),
            goal: String::new(),
            hire: "dev".into(),
            base: None,
            detail: "Escape opens it.".into(),
        })
        .await;
    assert!(made.ok, "{:?}", made.error);
    assert_eq!(made.data["detail"], "Escape opens it.");
}

fn finished(usd: f64, reported: bool, cumulative: bool) -> RunEvent {
    RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("s1".into()),
        cost: Cost {
            usd,
            reported,
            cumulative,
            ..Cost::default()
        },
        summary: None,
    }
}

fn spent(studio_budget: &Value) -> f64 {
    studio_budget["spent_24h"]["total"].as_f64().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_budget_never_claims_work() {
    let setup = setup_with("", "daily_usd = 0.02", "");
    let runtime = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Anything")).await;
    studio.schedule().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state_of(&studio, "GAME-1").await, "queued");
    assert!(runtime.specs().is_empty());
    assert_eq!(ok(&studio, Request::Status).await["out_of_budget"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn budget_lists_each_hires_own_ticket_cap() {
    let setup = setup("");
    fs::write(
        setup.paths.config_dir.join("hires/dev.md"),
        "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\nbudget_per_ticket_usd: 0.5\n---\nYou write code.\n",
    )
    .unwrap();
    let studio = studio(&setup, FakeRuntime::new(Vec::new()));
    let budget = ok(&studio, Request::Budget).await;
    assert_eq!(budget["limits"]["hires_ticket"]["dev"], 0.5);
    assert_eq!(ok(&studio, Request::Status).await["out_of_budget"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_max_runs_pauses_the_studio() {
    let setup = setup("");
    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("max_runs = 2", "max_runs = 0")).unwrap();
    let runtime = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Anything")).await;
    studio.schedule().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state_of(&studio, "GAME-1").await, "queued");
    assert!(runtime.specs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_studio_toml_applies_without_a_restart() {
    let setup = setup("");
    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("max_runs = 2", "max_runs = 0")).unwrap();
    let runtime = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Anything")).await;
    studio.schedule().unwrap();
    assert!(runtime.specs().is_empty());

    // Past the check interval, and a time stamp the file system can tell apart.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    fs::write(&config, &text).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    studio.schedule().unwrap();
    until(async || !runtime.specs().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hire_file_edited_on_disk_applies_without_a_restart() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.01, true, false)]));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    fs::write(
        setup.paths.config_dir.join("hires/newbie.md"),
        "---\nname: Newbie\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nHi.\n",
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    studio.schedule().unwrap();
    let made = studio
        .handle(Request::CreateTicket {
            project: "game".into(),
            title: "x".into(),
            goal: String::new(),
            hire: "newbie".into(),
            base: None,
            detail: String::new(),
        })
        .await;
    assert!(made.ok, "{:?}", made.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_is_held_once_its_cap_is_spent() {
    // Checks always fail, so the ticket keeps asking for more runs.
    let setup = setup_with(
        r#"["sh", "-c", "exit 1"]"#,
        "daily_usd = 10.0\nticket_usd = 0.5",
        "",
    );
    let runtime = FakeRuntime::new(vec![finished(0.4, true, false)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Expensive")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "budget").await;

    // The second run only got what was left of the ticket's cap.
    let caps: Vec<f64> = runtime
        .specs()
        .iter()
        .map(|s| s.max_budget_usd.unwrap())
        .collect();
    assert_eq!(caps.len(), 2);
    assert!((caps[0] - 0.5).abs() < 1e-9);
    assert!((caps[1] - 0.1).abs() < 1e-9);
    let budget = ok(&studio, Request::Budget).await;
    assert!((spent(&budget) - 0.8).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_live_meter_stops_a_run_that_overspends() {
    let setup = setup_with(
        "",
        "daily_usd = 10.0\nticket_usd = 0.5",
        "[prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n",
    );
    let runtime = FakeRuntime::new(vec![
        RunEvent::Started {
            session: "s1".into(),
            model: None,
        },
        RunEvent::Usage {
            message_id: "m1".into(),
            model: None,
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
        finished(0.0, false, false),
    ]);
    let studio = studio(&setup, runtime);
    ok(&studio, create("Burns money")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "budget").await;
    // The hold lands just before the run's spend is written.
    // Unreported cost falls back to the meter's estimate.
    until(async || (spent(&ok(&studio, Request::Budget).await) - 1.0).abs() < 1e-9).await;
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap();
    assert!(log.contains("reached this run"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ticket_spend_counts_the_live_run_so_far() {
    let setup = setup_with(
        "",
        "daily_usd = 10.0",
        "[prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n",
    );
    let mut events = script();
    events.insert(
        1,
        RunEvent::Usage {
            message_id: "m1".into(),
            model: None,
            input_tokens: 100_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    );
    let studio = studio(&setup, FakeRuntime::new(events));
    ok(&studio, create("Still running")).await;
    studio.spawn_scheduler();
    // The run waits on its ask, with nothing in the ledger yet.
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    assert!((spent(&ok(&studio, Request::Budget).await) - 0.0).abs() < 1e-9);
    let one = ok(
        &studio,
        Request::TicketSpend {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!((one.as_f64().unwrap() - 0.1).abs() < 1e-9, "{one}");
    let all = ok(&studio, Request::TicketSpends).await;
    assert!(
        (all["GAME-1"].as_f64().unwrap() - 0.1).abs() < 1e-9,
        "{all}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cumulative_session_costs_are_not_counted_twice() {
    // Three runs of one session, each reporting the same running total.
    let setup = setup_with(r#"["sh", "-c", "exit 1"]"#, "daily_usd = 10.0", "");
    let runtime = FakeRuntime::new(vec![finished(0.3, true, true)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Resumes")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "checks").await;
    assert_eq!(runtime.specs().len(), 3);
    let budget = ok(&studio, Request::Budget).await;
    assert!((spent(&budget) - 0.3).abs() < 1e-9);
    assert_eq!(budget["runs_24h"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn spend_is_reported_by_day_model_and_ticket() {
    let setup = setup("");
    let runtime = FakeRuntime::new(vec![finished(0.25, true, false)]);
    let studio = studio(&setup, runtime);
    ok(&studio, create("Costs a quarter")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let days = ok(&studio, Request::SpendByDay { days: 3 }).await;
    let days = days.as_array().unwrap();
    assert_eq!(days.len(), 3);
    assert_eq!(days[0]["usd"], 0.0);
    // Today's, unless the run crossed midnight.
    let late = days[1]["usd"].as_f64().unwrap() + days[2]["usd"].as_f64().unwrap();
    assert!((late - 0.25).abs() < 1e-9);
    assert!(days[0]["start_ms"].as_u64() < days[2]["start_ms"].as_u64());
    assert!(days[0]["date"].as_str() < days[2]["date"].as_str());

    let budget = ok(&studio, Request::Budget).await;
    assert!((budget["spent_24h"]["by_ticket"]["GAME-1"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(
        budget["spent_24h"]["by_model"].as_object().unwrap().len(),
        1
    );
    let runs = budget["spent_24h"]["runs_by_hire"].as_object().unwrap();
    let total: u64 = runs.values().filter_map(|n| n.as_u64()).sum();
    assert_eq!(total, budget["runs_24h"].as_u64().unwrap());

    // The ticket last changed when it became ready, no earlier than its creation.
    let updated = ok(&studio, Request::UpdatedAt).await;
    let tickets = ok(&studio, Request::Tickets).await;
    let created = tickets[0]["created_at"].as_u64().unwrap();
    assert!(updated["GAME-1"].as_u64().unwrap() >= created, "{updated}");
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_events_cover_every_ticket_from_a_time() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(Vec::new()));
    let before = now_ms();
    ok(&studio, create("One")).await;
    ok(&studio, create_in("Two", "Other goal")).await;

    let all = ok(
        &studio,
        Request::RecentEvents {
            since_ms: before,
            limit: Some(10),
            kinds: Vec::new(),
        },
    )
    .await;
    let ids: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["ticket_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["GAME-1", "GAME-2"]);
    let newest = ok(
        &studio,
        Request::RecentEvents {
            since_ms: before,
            limit: Some(1),
            kinds: Vec::new(),
        },
    )
    .await;
    assert_eq!(newest[0]["ticket_id"], "GAME-2");
    let none = ok(
        &studio,
        Request::RecentEvents {
            since_ms: before + 3_600_000,
            limit: Some(10),
            kinds: Vec::new(),
        },
    )
    .await;
    assert_eq!(none, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn activity_holds_the_histories_of_busy_tickets() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.25, true, false)]));
    ok(&studio, create("Costs a quarter")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let all = ok(&studio, Request::Activity { since_ms: 0 }).await;
    let all = all.as_array().unwrap();
    assert_eq!(all[0]["event"]["type"], "created");
    assert!(all.iter().all(|e| e["ticket_id"] == "GAME-1"));
    assert!(all.iter().any(|e| e["event"]["type"] == "checks_passed"));
    // A ready ticket that has done nothing since is left out.
    let later = ok(
        &studio,
        Request::Activity {
            since_ms: u64::MAX / 2,
        },
    )
    .await;
    assert_eq!(later, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn status_counts_goals_and_rule_files() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(Vec::new()));
    let status = ok(&studio, Request::Status).await;
    assert_eq!(
        (status["goals"].as_u64(), status["rules"].as_u64()),
        (Some(0), Some(0))
    );

    let rules = setup.paths.config_dir.join("rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(rules.join("tests.md"), "---\nkind: guide\n---\nTest it.\n").unwrap();
    fs::write(rules.join("notes.txt"), "not a rule").unwrap();
    ok(
        &studio,
        Request::CreateGoal {
            project: "game".into(),
            text: "Add a level".into(),
            hire: Some("dev".into()),
            detail: String::new(),
        },
    )
    .await;
    let status = ok(&studio, Request::Status).await;
    assert_eq!(
        (status["goals"].as_u64(), status["rules"].as_u64()),
        (Some(1), Some(1))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_price_and_no_reported_cost_refuses_to_run() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::without_cost_reports(script()));
    ok(&studio, create("Unpriced")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "budget").await;
    // The hold's time is the one the board shows.
    let since = ok(&studio, Request::StateSince).await;
    let events = ok(
        &studio,
        Request::Events {
            since: 0,
            limit: 100,
            ticket: Some("GAME-1".into()),
        },
    )
    .await;
    let held = events
        .as_array()
        .unwrap()
        .iter()
        .rfind(|e| e["event"]["type"] == "held")
        .unwrap();
    assert_eq!(since["GAME-1"], held["at_ms"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_runs_never_reserve_more_than_the_day_allows() {
    let setup = setup_with("", "daily_usd = 1.0\nticket_usd = 0.8", "");
    let runtime = FakeRuntime::new(script());
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create_in("First", "One")).await;
    ok(&studio, create_in("Second", "Two")).await;
    studio.spawn_scheduler();
    // Both runs stop at their ask, holding their reservations.
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 2).await;
    let mut caps: Vec<f64> = runtime
        .specs()
        .iter()
        .map(|s| s.max_budget_usd.unwrap())
        .collect();
    caps.sort_by(f64::total_cmp);
    assert!((caps[0] - 0.2).abs() < 1e-9 && (caps[1] - 0.8).abs() < 1e-9);
    let reserved = ok(&studio, Request::Budget).await["reserved"].clone();
    let total: f64 = reserved
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_f64().unwrap())
        .sum();
    assert!((total - 1.0).abs() < 1e-9);
}

fn write_rule(setup: &Setup, name: &str, text: &str) {
    let dir = setup.paths.config_dir.join("rules");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}.md")), text).unwrap();
}

fn permission(id: &str, tool: &str, input: Value) -> RunEvent {
    RunEvent::Permission {
        request_id: id.into(),
        tool: tool.into(),
        input,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_decide_permissions_without_bothering_the_owner() {
    let setup = setup("");
    write_rule(
        &setup,
        "cargo-ok",
        "---\nkind: gate\nlevel: allow\naction: \"bash:cargo \"\n---\nBuilding and testing is fine.\n",
    );
    write_rule(
        &setup,
        "no-push",
        "---\nkind: gate\nlevel: block\naction: \"bash:git push\"\n---\nThe studio pushes, not agents.\n",
    );
    let runtime = FakeRuntime::new(vec![
        permission("p1", "Bash", json!({ "command": "cargo test" })),
        permission("p2", "Bash", json!({ "command": "git push origin master" })),
        permission("p3", "Write", json!({ "file_path": "src/level.rs" })),
        finished(0.01, true, false),
    ]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Rules")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let received = runtime.received();
    assert!(
        matches!(&received[0], studio_runtime::Control::Allow { request_id } if request_id == "p1")
    );
    assert!(
        matches!(&received[1], studio_runtime::Control::Deny { message, .. } if message.contains("The studio pushes"))
    );
    assert!(matches!(
        &received[2],
        studio_runtime::Control::Allow { .. }
    ));
    assert!(
        ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protected_path_asks_with_the_reason() {
    let setup = setup("");
    write_rule(
        &setup,
        "tuning",
        "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"crates/engine/src/rules.rs\"]\n---\nTuning is the owner's call.\n",
    );
    let runtime = FakeRuntime::new(vec![
        permission(
            "p1",
            "Edit",
            json!({ "file_path": "crates/engine/src/rules.rs" }),
        ),
        finished(0.01, true, false),
    ]);
    let studio = studio(&setup, runtime);
    ok(&studio, create("Tune")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    let ask = ok(&studio, Request::Asks).await[0].clone();
    assert!(
        ask["reasons"][0]
            .as_str()
            .unwrap()
            .contains("Tuning is the owner's call")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_outside_the_worktree_ask() {
    let setup = setup("");
    let runtime = FakeRuntime::new(vec![
        permission("p1", "Write", json!({ "file_path": "../../escape.txt" })),
        finished(0.01, true, false),
    ]);
    let studio = studio(&setup, runtime);
    ok(&studio, create("Escape")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lint_rule_sends_a_change_back_before_it_is_committed() {
    // The fake runtime can't edit files, so the test plants a live ID in the
    // worktree while the run waits at its ask.
    let setup = setup("");
    write_rule(
        &setup,
        "no-live-ids",
        "---\nkind: lint\nlevel: block\npattern: \"ca-app-pub-[0-9]+\"\n---\nNever commit live ad unit IDs.\n",
    );
    let runtime = FakeRuntime::new(vec![
        permission("p1", "Bash", json!({ "command": "echo" })),
        finished(0.01, true, false),
    ]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Ads")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    let worktree = setup.repo.join(".studio/worktrees/game-1");
    fs::write(
        worktree.join("ads.rs"),
        "const UNIT: &str = \"ca-app-pub-1234\";\n",
    )
    .unwrap();
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: true,
            message: None,
            answers: None,
        },
    )
    .await;
    // Every attempt finds the same planted ID, so the ticket fails on checks
    // without ever committing it.
    until(async || {
        let asks = ok(&studio, Request::Asks).await;
        for ask in asks.as_array().unwrap() {
            let id = ask["id"].as_str().unwrap().to_string();
            ok(
                &studio,
                Request::Decide {
                    ask: id,
                    allow: true,
                    message: None,
                    answers: None,
                },
            )
            .await;
        }
        state_of(&studio, "GAME-1").await["failed"]["class"] == "checks"
    })
    .await;
    assert_eq!(git(&worktree, &["log", "--oneline"]).lines().count(), 1);
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap();
    assert!(log.contains("Never commit live ad unit IDs"));
}

fn add_reviewer(setup: &Setup, model: &str) {
    fs::write(
        setup.paths.config_dir.join("hires/critic.md"),
        format!("---\nname: Critic\nrole: reviewer\nruntime: fake-review\nmodel: {model}\n---\nYou review.\n"),
    )
    .unwrap();
}

fn studio_with(setup: &Setup, author: FakeRuntime, reviewer: FakeRuntime) -> Arc<Studio> {
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(author));
    runtimes.insert("fake-review".into(), Arc::new(reviewer));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

fn reply(text: &str) -> RunEvent {
    RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("r1".into()),
        cost: Cost {
            usd: 0.01,
            reported: true,
            ..Cost::default()
        },
        summary: Some(text.into()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reviewer_on_another_model_passes_the_work() {
    let setup = setup("");
    add_reviewer(&setup, "fake-2");
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![
        permission("w1", "Write", json!({ "file_path": "src/fix.rs" })),
        reply("Does what the ticket asks.\nVERDICT: PASS"),
    ]);
    let studio = studio_with(&setup, author, reviewer.clone());
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let spec = &reviewer.specs()[0];
    assert!(spec.prompt.contains("VERDICT: PASS"));
    assert!(spec.resume.is_none());
    // Reviewers can't write, even inside the worktree.
    assert!(matches!(
        &reviewer.received()[0],
        studio_runtime::Control::Deny { .. }
    ));
    let budget = ok(&studio, Request::Budget).await;
    assert!(budget["spent_24h"]["by_hire"]["critic"].as_f64().unwrap() > 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_given_before_the_budget_ran_out_still_counts() {
    let setup = setup("");
    add_reviewer(&setup, "fake-2");
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![RunEvent::Finished {
        outcome: Outcome::Budget,
        session: Some("r1".into()),
        cost: Cost {
            usd: 0.6,
            reported: true,
            ..Cost::default()
        },
        summary: Some("Does what the ticket asks.\nVERDICT: PASS".into()),
    }]);
    let studio = studio_with(&setup, author, reviewer);
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_said_before_a_cut_short_run_still_counts() {
    let setup = setup("");
    add_reviewer(&setup, "fake-2");
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![
        RunEvent::Text {
            text: "Does what the ticket asks.\nVERDICT: PASS".into(),
        },
        RunEvent::Finished {
            outcome: Outcome::Budget,
            session: Some("r1".into()),
            cost: Cost::default(),
            summary: None,
        },
    ]);
    let studio = studio_with(&setup, author, reviewer);
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reviewer_on_the_authors_model_falls_back_to_its_escalate_model() {
    let setup = setup("");
    fs::write(
        setup.paths.config_dir.join("hires/critic.md"),
        "---\nname: Critic\nrole: reviewer\nruntime: fake-review\nmodel: fake-1\nescalate: [fake-2]\n---\nYou review.\n",
    )
    .unwrap();
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![reply("VERDICT: PASS")]);
    let studio = studio_with(&setup, author, reviewer.clone());
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let specs = reviewer.specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].model.as_deref(), Some("fake-2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_review_sends_the_notes_back_to_the_author() {
    let setup = setup("");
    add_reviewer(&setup, "fake-2");
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![reply("The level never loads.\nVERDICT: FAIL")]);
    let studio = studio_with(&setup, author.clone(), reviewer);
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "review").await;
    let specs = author.specs();
    assert_eq!(specs.len(), 3);
    assert!(specs[1].prompt.contains("The level never loads."));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_verdict_is_a_fail_and_same_model_reviewers_are_skipped() {
    let first = setup("");
    add_reviewer(&first, "fake-1");
    let reviewer = FakeRuntime::new(vec![reply("VERDICT: PASS")]);
    let studio = studio_with(
        &first,
        FakeRuntime::new(vec![finished(0.01, true, false)]),
        reviewer.clone(),
    );
    ok(&studio, create("Self review")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(reviewer.specs().is_empty());

    let second = setup("");
    add_reviewer(&second, "fake-2");
    let reviewer = FakeRuntime::new(vec![reply("Looks fine to me.")]);
    let studio = studio_with(
        &second,
        FakeRuntime::new(vec![finished(0.01, true, false)]),
        reviewer,
    );
    ok(&studio, create("Vague")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "review").await;
}

fn save(kind: FileKind, name: &str, text: &str) -> Request {
    Request::SaveFile {
        kind,
        name: name.into(),
        text: text.into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_rule_is_listed() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let text = "---\nkind: lint\nlevel: warn\npattern: todo\n---\nNo TODOs.\n";
    ok(&studio, save(FileKind::Rules, "no-todo", text)).await;
    let files = ok(
        &studio,
        Request::Files {
            kind: FileKind::Rules,
        },
    )
    .await;
    assert_eq!(files[0]["name"], "no-todo");
    assert_eq!(files[0]["text"], text);
    assert!(files[0]["problem"].is_null());
    assert!(setup.paths.config_dir.join("rules/no-todo.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_rule_is_refused_and_not_written() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let text = "---\nkind: lint\nlevel: block\npattern: \"(\"\n---\nBad.\n";
    let bad = studio.handle(save(FileKind::Rules, "bad", text)).await;
    assert!(!bad.ok);
    assert!(bad.error.unwrap().contains("not a valid regex"));
    assert!(!setup.paths.config_dir.join("rules/bad.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_file_names_are_refused() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let text = "---\nkind: lint\nlevel: warn\npattern: x\n---\nok\n";
    for name in ["../x", "", "A", "-a", "a.b", "a/b", &"a".repeat(65)] {
        assert!(
            !studio.handle(save(FileKind::Rules, name, text)).await.ok,
            "{name}"
        );
    }
    let del = Request::DeleteFile {
        kind: FileKind::Hires,
        name: "../dev".into(),
    };
    assert!(!studio.handle(del).await.ok);
    let big = format!("{text}{}", "x".repeat(65 * 1024));
    assert!(!studio.handle(save(FileKind::Rules, "big", &big)).await.ok);
    assert!(setup.paths.config_dir.join("hires/dev.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_saved_hire_is_usable_without_a_restart() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let ghost = Request::CreateTicket {
        project: "game".into(),
        title: "x".into(),
        goal: String::new(),
        hire: "newbie".into(),
        base: None,
        detail: String::new(),
    };
    assert!(!studio.handle(ghost.clone()).await.ok);
    let text = "---\nname: Newbie\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nHi.\n";
    ok(&studio, save(FileKind::Hires, "newbie", text)).await;
    ok(&studio, ghost).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_file_removes_it() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    let del = Request::DeleteFile {
        kind: FileKind::Hires,
        name: "dev".into(),
    };
    ok(&studio, del.clone()).await;
    assert!(!setup.paths.config_dir.join("hires/dev.md").exists());
    let files = ok(
        &studio,
        Request::Files {
            kind: FileKind::Hires,
        },
    )
    .await;
    assert!(files.as_array().unwrap().is_empty());
    assert!(!studio.handle(create("a")).await.ok);
    assert!(!studio.handle(del).await.ok);
}

fn plan_script() -> Vec<RunEvent> {
    vec![
        RunEvent::Started {
            session: "plan".into(),
            model: Some("fake-1".into()),
        },
        RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("plan".into()),
            cost: Cost::default(),
            summary: Some("Two tickets".into()),
        },
    ]
}

fn file(ticket: &str, title: &str, detail: &str) -> Request {
    Request::FileTicket {
        ticket: ticket.into(),
        title: title.into(),
        detail: detail.into(),
        hire: None,
    }
}

#[tokio::test]
async fn a_producer_files_tickets_on_its_goal_and_the_plan_lands() {
    let setup = setup_with("", "daily_usd = 5.0", "[goals]\nmax_filed = 2");
    fs::write(
        setup.paths.config_dir.join("hires/producer.md"),
        "---\nname: Producer\nrole: producer\nruntime: fake\nmodel: fake-1\ntools: [file_ticket]\n---\nYou plan.\n",
    )
    .unwrap();
    let (runtime, finish) = FakeRuntime::new(plan_script()).with_mcp().held();
    let studio = studio(&setup, runtime.clone());
    studio.spawn_scheduler();

    let plan = ok(
        &studio,
        Request::CreateGoal {
            project: "game".into(),
            text: "Add a boss fight".into(),
            hire: None,
            detail: String::new(),
        },
    )
    .await;
    assert_eq!(plan["title"], "Plan: Add a boss fight");
    assert_eq!(plan["kind"], "plan");
    assert_eq!(plan["hire"], "producer");
    let plan_id = plan["id"].as_str().unwrap().to_string();
    let early = studio.handle(file(&plan_id, "Too soon", "x")).await;
    assert!(early.error.unwrap().contains("only a working ticket"));
    until(async || state_of(&studio, &plan_id).await == "working").await;

    // The producer's run gets the studio tools, pointed at this ticket.
    let spec = runtime.specs().remove(0);
    let config: Value =
        serde_json::from_str(&fs::read_to_string(spec.mcp_config.unwrap()).unwrap()).unwrap();
    let args = &config["mcpServers"]["studio"]["args"];
    assert_eq!(args[0], "mcp");
    assert_eq!(args[2], plan_id.as_str());

    // What the MCP server sends when the producer calls file_ticket.
    let child = ok(&studio, file(&plan_id, "Boss sprite", "Acceptance: 32x32")).await;
    assert_eq!(child["parent"], plan_id.as_str());
    assert_eq!(child["goal"], "Add a boss fight");
    assert_eq!(child["hire"], "dev");
    assert_eq!(child["kind"], "work");
    ok(&studio, file(&plan_id, "Boss AI", "Acceptance: chases")).await;
    let capped = studio.handle(file(&plan_id, "One more", "x")).await;
    assert!(!capped.ok);
    assert!(capped.error.unwrap().contains("most it may file (2)"));

    // The engineer's hire doesn't list file_ticket.
    let child_id = child["id"].as_str().unwrap();
    let refused = studio.handle(file(child_id, "Nope", "x")).await;
    assert!(!refused.ok);
    assert!(refused.error.unwrap().contains("file_ticket"));

    // Children run on the same held fake, so release every waiting run.
    until(async || {
        finish.notify_waiters();
        state_of(&studio, &plan_id).await == "landed"
    })
    .await;
    // The plan's worktree is gone and nothing was merged.
    assert!(
        git(&setup.repo, &["worktree", "list"])
            .lines()
            .all(|l| !l.contains("studio/") || !l.contains(&plan_id.to_lowercase()))
    );
    let specs = runtime.specs();
    assert!(specs.iter().any(|s| s.prompt.contains("planning the goal")));
    until(async || {
        runtime
            .specs()
            .iter()
            .any(|s| s.prompt.contains("Acceptance: 32x32"))
    })
    .await;
}

#[tokio::test]
async fn a_goal_needs_a_producer() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(plan_script()));
    let reply = studio
        .handle(Request::CreateGoal {
            project: "game".into(),
            text: "Anything".into(),
            hire: None,
            detail: String::new(),
        })
        .await;
    assert!(!reply.ok);
    assert!(reply.error.unwrap().contains("producer"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_attempt_moves_the_hire_to_its_next_model() {
    let setup = setup(r#"["sh", "-c", "echo broken; exit 1"]"#);
    fs::write(
        setup.paths.config_dir.join("hires/dev.md"),
        "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\nescalate: [fake-2]\n---\nYou write code.\n",
    )
    .unwrap();
    let runtime = FakeRuntime::new(vec![RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("s1".into()),
        cost: Cost::default(),
        summary: None,
    }]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Break things")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "checks").await;

    let specs = runtime.specs();
    assert_eq!(specs.len(), 3);
    let models: Vec<_> = specs.iter().map(|s| s.model.as_deref()).collect();
    assert_eq!(models, [Some("fake-1"), Some("fake-2"), Some("fake-2")]);
    // The first escalation starts a fresh session; the next keeps resuming it.
    assert!(specs[1].resume.is_none());
    assert!(specs[1].prompt.contains("broken"));
    assert_eq!(specs[2].resume.as_deref(), Some("s1"));
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap();
    assert!(log.contains("escalated to fake-2 after 1 failed attempts"));
}

#[tokio::test(flavor = "multi_thread")]
async fn landed_tickets_give_quotes_and_etas() {
    let setup = setup("");
    let runtime = FakeRuntime::new(script());
    let studio = studio(&setup, runtime);
    ok(&studio, create("First")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    ok(
        &studio,
        Request::Decide {
            ask: "a1".into(),
            allow: true,
            message: None,
            answers: None,
        },
    )
    .await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    // A ticket waiting for the owner has no finish time to give.
    assert!(
        ok(&studio, Request::Estimates)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;

    ok(&studio, create("Second")).await;
    let estimates = ok(&studio, Request::Estimates).await;
    let estimates = estimates.as_array().unwrap();
    assert_eq!(estimates.len(), 1);
    assert_eq!(estimates[0]["ticket"], "GAME-2");
    assert!(estimates[0]["quote_usd"].is_number());
    assert!(estimates[0]["eta_ms"].as_u64().unwrap() > 0);
}

fn fake_gh(dir: &Path, runs: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    let script = format!(
        "#!/bin/sh\ncase \"$2\" in\n list) cat <<'EOF'\n{runs}\nEOF\n;;\n view) echo 'the failing log';;\nesac\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn run_json(id: u64, workflow: &str, conclusion: &str) -> String {
    format!(
        r#"{{"databaseId":{id},"conclusion":"{conclusion}","status":"completed","headSha":"abcdef1234567","workflowName":"{workflow}","url":"https://example.com/runs/{id}"}}"#
    )
}

async fn tickets(studio: &Arc<Studio>) -> Vec<Value> {
    ok(studio, Request::Tickets)
        .await
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_ci_run_files_one_ticket_once() {
    let setup = setup("");
    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        text.replace(
            "integration_branch = \"master\"",
            "integration_branch = \"master\"\nwatch_ci = true",
        ),
    )
    .unwrap();
    let studio = studio(&setup, FakeRuntime::new(vec![]));
    let bin = tempfile::tempdir().unwrap();

    // The newest run of "build" failed; an older failure and a passing "lint" are ignored.
    let runs = format!(
        "[{},{},{}]",
        run_json(7, "build", "failure"),
        run_json(5, "build", "failure"),
        run_json(6, "lint", "success")
    );
    let watcher = CiWatcher::new(fake_gh(bin.path(), &runs));
    watcher.tick(&studio).await;
    watcher.tick(&studio).await;
    let all = tickets(&studio).await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["title"], "Fix CI: build failed on abcdef1");
    assert_eq!(all[0]["hire"], "dev");
    assert_eq!(all[0]["source"], "ci:7");
    let detail = all[0]["detail"].as_str().unwrap();
    assert!(detail.contains("https://example.com/runs/7") && detail.contains("the failing log"));

    // Cancelled, the same run is not filed again; a new failure is.
    ok(
        &studio,
        Request::Cancel {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    watcher.tick(&studio).await;
    assert_eq!(tickets(&studio).await.len(), 1);
    let watcher = CiWatcher::new(fake_gh(
        bin.path(),
        &format!("[{}]", run_json(8, "build", "failure")),
    ));
    watcher.tick(&studio).await;
    let all = tickets(&studio).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[1]["source"], "ci:8");

    // An open CI ticket holds back further ones.
    let watcher = CiWatcher::new(fake_gh(
        bin.path(),
        &format!("[{}]", run_json(9, "build", "failure")),
    ));
    watcher.tick(&studio).await;
    assert_eq!(tickets(&studio).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_gh_is_harmless() {
    let setup = setup("");
    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        text.replace(
            "integration_branch = \"master\"",
            "integration_branch = \"master\"\nwatch_ci = true",
        ),
    )
    .unwrap();
    let studio = studio(&setup, FakeRuntime::new(vec![]));
    let watcher = CiWatcher::new("/nonexistent/gh");
    watcher.tick(&studio).await;
    watcher.tick(&studio).await;
    assert!(tickets(&studio).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn gc_removes_only_what_no_open_ticket_needs() {
    use studio_core::ids::TicketId;
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(vec![]));
    ok(&studio, create("Cancelled one")).await;
    ok(&studio, create("Still queued")).await;
    let repo = studio_git::Repo::open(&setup.repo).unwrap();
    for id in ["GAME-1", "GAME-2", "GAME-9"] {
        repo.create_worktree(&TicketId::new(id), "master").unwrap();
    }
    ok(
        &studio,
        Request::Cancel {
            ticket: "GAME-1".into(),
        },
    )
    .await;

    let stat = ok(
        &studio,
        Request::Diffstat {
            ticket: "GAME-2".into(),
        },
    )
    .await;
    assert!(stat.is_string());
    // Several at once: an unknown ticket or one without a worktree is null.
    let stats = ok(
        &studio,
        Request::Diffstats {
            tickets: vec!["GAME-2".into(), "NOPE-1".into()],
        },
    )
    .await;
    assert_eq!(stats["GAME-2"], stat);
    assert!(stats["NOPE-1"].is_null(), "{stats}");
    let events = ok(
        &studio,
        Request::Events {
            since: 0,
            limit: 100,
            ticket: Some("GAME-2".into()),
        },
    )
    .await;
    assert_eq!(events.as_array().unwrap().len(), 1);

    let removed = ok(&studio, Request::Gc).await;
    let lines: Vec<&str> = removed
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("GAME-1: removed worktree"))
    );
    // GAME-1 was cancelled: its branch stays for the owner.
    assert!(repo.branch_exists("studio/game-1").unwrap());
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("no ticket: removed worktree"))
    );

    let worktrees = repo.studio_worktrees().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].branch, "studio/game-2");
    assert!(
        ok(
            &studio,
            Request::Diffstat {
                ticket: "GAME-1".into()
            }
        )
        .await
        .is_null()
    );
}

// Packs, capture evidence and gates.

fn write_project(setup: &Setup, text: &str) {
    fs::write(setup.repo.join(".studio.toml"), text).unwrap();
}

/// A project file whose check leaves an art file behind, so the change
/// matches `assets/**`.
fn project_with_art(extra: &str) -> String {
    format!(
        "verify = [[\"sh\", \"-c\", \"mkdir -p assets && echo x > assets/hero.png\"]]\n{extra}\n"
    )
}

const CAPTURE: &str =
    r#"capture = ["sh", "-c", 'printf x > "$0/title.png"; printf x > "$0/notes.txt"', "{out}"]"#;

fn add_critic(setup: &Setup, id: &str, model: &str) {
    fs::write(
        setup.paths.config_dir.join(format!("hires/{id}.md")),
        format!("---\nname: Critic\nrole: critic\nruntime: fake-review\nmodel: {model}\n---\nYou judge art.\n"),
    )
    .unwrap();
}

async fn events_of(studio: &Arc<Studio>, ticket: &str) -> Vec<Value> {
    let events = ok(
        studio,
        Request::Events {
            since: 0,
            limit: 1000,
            ticket: Some(ticket.into()),
        },
    )
    .await;
    events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"].clone())
        .collect()
}

fn run_log(setup: &Setup) -> String {
    fs::read_to_string(setup.paths.data_dir.join("runs/GAME-1.jsonl")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capture_run_records_its_evidence() {
    let setup = setup("");
    write_project(
        &setup,
        &project_with_art(&format!("{CAPTURE}\ncapture_paths = [\"assets/**\"]")),
    );
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.01, true, false)]));
    ok(&studio, create("Art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let files = ok(
        &studio,
        Request::Evidence {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    let files = files.as_array().unwrap();
    // Only images and clips are listed.
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["attempt"], 0);
    assert_eq!(files[0]["name"], "title.png");
    let path = files[0]["path"].as_str().unwrap();
    assert!(path.ends_with(".studio/worktrees/game-1/.studio-evidence/0/title.png"));
    assert!(Path::new(path).is_file());
    assert!(run_log(&setup).contains("evidence: .studio-evidence/0/title.png"));

    // The folder can't be committed, from this worktree or any other.
    let exclude = fs::read_to_string(setup.repo.join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == ".studio-evidence/"));
    let worktree = setup.repo.join(".studio/worktrees/game-1");
    assert!(!git(&worktree, &["status", "--porcelain"]).contains("studio-evidence"));
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_only_runs_for_matching_changes() {
    let setup = setup("");
    write_project(
        &setup,
        &project_with_art(&format!("{CAPTURE}\ncapture_paths = [\"src/**\"]")),
    );
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.01, true, false)]));
    ok(&studio, create("Art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let files = ok(
        &studio,
        Request::Evidence {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!(files.as_array().unwrap().is_empty());
    assert!(run_log(&setup).contains("capture skipped"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_capture_fails_the_checks() {
    let setup = setup("");
    write_project(
        &setup,
        "capture = [\"sh\", \"-c\", \"echo window went black >&2; exit 3\"]\n",
    );
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, author.clone());
    ok(&studio, create("Crashy")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "checks").await;
    let log = run_log(&setup);
    assert!(log.contains("crashed or hung"));
    assert!(log.contains("window went black"));
    assert!(author.specs()[1].prompt.contains("window went black"));
}

fn gate_project() -> String {
    project_with_art(&format!(
        "{CAPTURE}\n[[gates]]\nname = \"art\"\npaths = [\"assets/**\"]\nhire = \"art-critic\"\nprompt = \"Be strict about palettes.\"\nrounds = 2\n"
    ))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_gate_sends_the_work_back_until_it_passes() {
    let setup = setup("");
    add_critic(&setup, "art-critic", "fake-2");
    write_project(&setup, &gate_project().replace("rounds = 2", "rounds = 3"));
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let gate = FakeRuntime::sequence(vec![
        vec![reply("The sprite is blurry.\nVERDICT: FAIL")],
        vec![reply("Crisp now.\nVERDICT: PASS")],
    ]);
    let studio = studio_with(&setup, author.clone(), gate.clone());
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let specs = gate.specs();
    assert_eq!(specs.len(), 2);
    assert!(specs[0].prompt.contains("Be strict about palettes."));
    assert!(specs[0].prompt.contains("- .studio-evidence/0/title.png"));
    assert!(!specs[0].prompt.contains("notes.txt"));
    // The new hero.png is a new file: only its `after` image is written.
    assert!(specs[0].prompt.contains(
        "- assets/hero.png: before new file, after .studio-evidence/0/after-assets-hero-png.png"
    ));
    // After the send-back the next attempt has its own evidence folder.
    assert!(specs[1].prompt.contains("- .studio-evidence/1/title.png"));
    let author_specs = author.specs();
    assert_eq!(author_specs.len(), 2);
    assert!(
        author_specs[1]
            .prompt
            .contains("art gate: The sprite is blurry.")
    );
    let failed: Vec<_> = events_of(&studio, "GAME-1")
        .await
        .into_iter()
        .filter(|e| e["type"] == "review_failed")
        .collect();
    assert_eq!(failed.len(), 1);
    assert!(
        failed[0]["detail"]
            .as_str()
            .unwrap()
            .starts_with("art gate:")
    );
    // Both attempts' screenshots, and the image the gate wrote for each:
    // the ticket page shows the pairs apart from the screenshots.
    let evidence = ok(
        &studio,
        Request::Evidence {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    let mut names: Vec<String> = evidence
        .as_array()
        .unwrap()
        .iter()
        .map(|f| format!("{}/{}", f["attempt"], f["name"].as_str().unwrap()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "0/after-assets-hero-png.png",
            "0/title.png",
            "1/after-assets-hero-png.png",
            "1/title.png"
        ]
    );
}

/// A gate that always fails, rounds = 2: the second failure asks the owner.
async fn gate_asks(setup: &Setup) -> (Arc<Studio>, Value) {
    add_critic(setup, "art-critic", "fake-2");
    write_project(setup, &gate_project());
    let gate = FakeRuntime::new(vec![reply("Still ugly.\nVERDICT: FAIL")]);
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio_with(setup, author.clone(), gate.clone());
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    // One send-back, then the ask; no third author run.
    assert_eq!(author.specs().len(), 2);
    assert_eq!(gate.specs().len(), 2);
    let ask = ok(&studio, Request::Asks).await[0].clone();
    assert_eq!(ask["kind"], "gate");
    assert_eq!(ask["tool"], "art");
    assert_eq!(ask["hire"], "art-critic");
    assert!(
        ask["reasons"][0]
            .as_str()
            .unwrap()
            .starts_with("art gate: Still ugly.")
    );
    assert!(state_of(&studio, "GAME-1").await["asking"].is_object());
    (studio, ask)
}

fn decide(ask: &Value, allow: bool) -> Request {
    Request::Decide {
        ask: ask["id"].as_str().unwrap().into(),
        allow,
        message: None,
        answers: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_gate_asks_and_the_owner_can_pass_it() {
    let setup = setup("");
    let (studio, ask) = gate_asks(&setup).await;
    ok(&studio, decide(&ask, true)).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_gate_asks_and_the_owner_can_fail_the_ticket() {
    let setup = setup("");
    let (studio, ask) = gate_asks(&setup).await;
    ok(&studio, decide(&ask, false)).await;
    until(async || state_of(&studio, "GAME-1").await["failed"]["class"] == "denied").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gate_never_grades_its_own_authors_model() {
    let setup = setup("");
    // Same model as the author: skipped, not self-graded.
    add_critic(&setup, "art-critic", "fake-1");
    write_project(&setup, &gate_project());
    let gate = FakeRuntime::new(vec![reply("VERDICT: FAIL")]);
    let studio = studio_with(
        &setup,
        FakeRuntime::new(vec![finished(0.01, true, false)]),
        gate.clone(),
    );
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(gate.specs().is_empty());
    assert!(run_log(&setup).contains("gate art skipped"));

    // A gate that names the ticket's own hire is skipped too.
    let setup = setup_with_dev_gate();
    let gate = FakeRuntime::new(vec![reply("VERDICT: FAIL")]);
    let studio = studio_with(
        &setup,
        FakeRuntime::new(vec![finished(0.01, true, false)]),
        gate.clone(),
    );
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(gate.specs().is_empty());
    assert!(run_log(&setup).contains("the ticket's own hire"));
}

fn setup_with_dev_gate() -> Setup {
    let setup = setup("");
    write_project(&setup, &gate_project().replace("art-critic", "dev"));
    setup
}

/// Writes a hire on the given runtime with a model and escalate list.
fn write_hire(setup: &Setup, id: &str, role: &str, runtime: &str, model: &str, escalate: &str) {
    fs::write(
        setup.paths.config_dir.join(format!("hires/{id}.md")),
        format!("---\nname: {id}\nrole: {role}\nruntime: {runtime}\nmodel: {model}\nescalate: [{escalate}]\n---\nYou work.\n"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gate_on_the_authors_model_falls_back_to_an_escalate_model() {
    let setup = setup("");
    // The author moves to fake-2 after a send-back.
    write_hire(&setup, "dev", "engineer", "fake", "fake-1", "fake-2");
    write_hire(
        &setup,
        "art-critic",
        "critic",
        "fake-review",
        "fake-1",
        "fake-2, fake-3",
    );
    write_project(&setup, &gate_project().replace("rounds = 2", "rounds = 3"));
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let gate = FakeRuntime::sequence(vec![
        vec![reply("Blurry.\nVERDICT: FAIL")],
        vec![reply("Crisp.\nVERDICT: PASS")],
    ]);
    let studio = studio_with(&setup, author.clone(), gate.clone());
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let authors: Vec<_> = author.specs().into_iter().map(|s| s.model).collect();
    assert_eq!(authors, [Some("fake-1".into()), Some("fake-2".into())]);
    // The first attempt was fake-1's, so the critic takes its first escalate
    // model; the second was fake-2's, so only fake-2 is avoided.
    let critics: Vec<_> = gate.specs().into_iter().map(|s| s.model).collect();
    assert_eq!(critics, [Some("fake-2".into()), Some("fake-1".into())]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gate_with_only_the_authors_model_is_skipped() {
    let setup = setup("");
    write_hire(
        &setup,
        "art-critic",
        "critic",
        "fake-review",
        "fake-1",
        "fake-1",
    );
    write_project(&setup, &gate_project());
    let gate = FakeRuntime::new(vec![reply("VERDICT: FAIL")]);
    let studio = studio_with(
        &setup,
        FakeRuntime::new(vec![finished(0.01, true, false)]),
        gate.clone(),
    );
    ok(&studio, create("Hero art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(gate.specs().is_empty());
    assert!(run_log(&setup).contains("art-critic runs on fake-1, which the author used"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pack_hires_load_and_studio_hires_override_them() {
    let pack = tempfile::tempdir().unwrap();
    fs::create_dir_all(pack.path().join("hires")).unwrap();
    for (name, model) in [("helper", "pack-1"), ("dev", "pack-2")] {
        fs::write(
            pack.path().join(format!("hires/{name}.md")),
            format!("---\nname: {name}\nrole: engineer\nruntime: fake\nmodel: {model}\n---\nFrom the pack.\n"),
        )
        .unwrap();
    }
    let extra = format!(
        "[[packs]]\nname = \"p\"\npath = \"{}\"\n",
        pack.path().display()
    );
    let setup = setup_with("", "daily_usd = 5.0", &extra);
    let runtime = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, runtime.clone());

    let status = ok(&studio, Request::Status).await;
    let hires: Vec<&str> = status["hires"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    assert!(hires.contains(&"helper"));
    // The studio's own `dev` replaced the pack's.
    ok(&studio, create("Who am I")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let spec = &runtime.specs()[0];
    assert_eq!(spec.model.as_deref(), Some("fake-1"));
    assert_eq!(
        spec.append_system_prompt.as_deref(),
        Some("You write code.")
    );

    // Pack hires are read-only: the editable list holds the studio's files only.
    let files = ok(
        &studio,
        Request::Files {
            kind: FileKind::Hires,
        },
    )
    .await;
    let names: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["dev"]);
}

/// A studio with an approver on its own scripted runtime.
fn with_approver(setup: &Setup, dev: FakeRuntime, approver: FakeRuntime) -> Arc<Studio> {
    fs::write(
        setup.paths.config_dir.join("hires/approver.md"),
        "---\nname: Approver\nrole: approver\nruntime: fake-approver\nmodel: fake-2\n---\nYou decide.\n",
    )
    .unwrap();
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(dev));
    runtimes.insert("fake-approver".into(), Arc::new(approver));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

fn bash_then_finish() -> FakeRuntime {
    FakeRuntime::new(vec![
        permission(
            "p1",
            "Bash",
            json!({ "command": "cargo run --example demo" }),
        ),
        finished(0.01, true, false),
    ])
}

#[tokio::test(flavor = "multi_thread")]
async fn the_approver_allows_a_call_without_bothering_the_owner() {
    let setup = setup("");
    let dev = bash_then_finish();
    let approver = FakeRuntime::new(vec![reply(
        "It runs the project's example.\nDECISION: ALLOW: fine",
    )]);
    let studio = with_approver(&setup, dev.clone(), approver.clone());
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(matches!(
        &dev.received()[0],
        studio_runtime::Control::Allow { .. }
    ));
    assert!(
        ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let spec = &approver.specs()[0];
    assert!(spec.prompt.contains("cargo run --example demo"));
    assert!(spec.prompt.contains("Demo"));
    assert_eq!(spec.max_turns, Some(1));
    assert!(spec.disallowed_tools.contains(&"Bash".to_string()));
    assert!(run_log(&setup).contains("approver allowed Bash: fine"));
    // The approver's cost is charged to the ticket.
    let spend = ok(
        &studio,
        Request::TicketSpend {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!(spend.as_f64().unwrap() > 0.015, "{spend}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approver_denial_does_not_stop_the_ticket() {
    let setup = setup("");
    let dev = bash_then_finish();
    let approver = FakeRuntime::new(vec![reply(
        "Use the Edit tool.\nDECISION: DENY: write files with Edit",
    )]);
    let studio = with_approver(&setup, dev.clone(), approver);
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(matches!(
        &dev.received()[0],
        studio_runtime::Control::Deny { message, .. }
            if message.contains("approver declined this: write files with Edit")
    ));
    assert!(
        ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_approver_can_send_a_call_to_the_owner_with_its_reason() {
    let setup = setup("");
    let approver = FakeRuntime::new(vec![reply(
        "It publishes.\nDECISION: OWNER: it publishes the crate",
    )]);
    let studio = with_approver(&setup, bash_then_finish(), approver);
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    let ask = ok(&studio, Request::Asks).await[0].clone();
    assert!(
        ask["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "Approver: it publishes the crate")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unclear_approver_falls_back_to_the_owner() {
    let setup = setup("");
    let approver = FakeRuntime::new(vec![reply("Sure.")]);
    let studio = with_approver(&setup, bash_then_finish(), approver);
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unclear_approver_is_asked_once_more_for_its_decision() {
    let setup = setup("");
    let dev = bash_then_finish();
    let approver = FakeRuntime::sequence(vec![
        vec![reply("It runs the project's own example, which is fine.")],
        vec![reply("DECISION: ALLOW: runs the example")],
    ]);
    let studio = with_approver(&setup, dev.clone(), approver.clone());
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(matches!(
        &dev.received()[0],
        studio_runtime::Control::Allow { .. }
    ));
    assert!(
        ok(&studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let specs = approver.specs();
    assert_eq!(specs.len(), 2);
    // The second ask carries the first reply.
    assert!(
        specs[1]
            .prompt
            .contains("It runs the project's own example, which is fine.")
    );
    assert!(run_log(&setup).contains("approver allowed Bash: runs the example"));
}

#[tokio::test(flavor = "multi_thread")]
async fn two_unclear_replies_reach_the_owner_with_the_first_quoted() {
    let setup = setup("");
    let approver = FakeRuntime::sequence(vec![
        vec![reply("Looks like a normal build step.")],
        vec![reply("Yes.")],
    ]);
    let studio = with_approver(&setup, bash_then_finish(), approver.clone());
    ok(&studio, create("Demo")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    assert_eq!(approver.specs().len(), 2);
    let ask = ok(&studio, Request::Asks).await[0].clone();
    assert!(
        ask["reasons"].as_array().unwrap().iter().any(|r| r
            == "Approver: the approver gave no clear answer: \"Looks like a normal build step.\""),
        "{ask}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_rules_skip_the_approver() {
    let setup = setup("");
    write_rule(
        &setup,
        "tuning",
        "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"crates/engine/src/rules.rs\"]\n---\nTuning is the owner's call.\n",
    );
    let dev = FakeRuntime::new(vec![
        permission(
            "p1",
            "Edit",
            json!({ "file_path": "crates/engine/src/rules.rs" }),
        ),
        finished(0.01, true, false),
    ]);
    let approver = FakeRuntime::new(vec![reply("DECISION: ALLOW: fine")]);
    let studio = with_approver(&setup, dev, approver.clone());
    ok(&studio, create("Tune")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    assert!(approver.specs().is_empty());
}

fn with_router(setup: &Setup, router: FakeRuntime) -> Arc<Studio> {
    fs::write(
        setup.paths.config_dir.join("hires/alpha.md"),
        "---\nname: Alpha\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nYou also write code.\n",
    )
    .unwrap();
    fs::write(
        setup.paths.config_dir.join("hires/router.md"),
        "---\nname: Router\nrole: router\nruntime: fake-router\nmodel: fake-2\n---\nYou pick.\n",
    )
    .unwrap();
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(FakeRuntime::new(plan_script())));
    runtimes.insert("fake-router".into(), Arc::new(router));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

fn unrouted(title: &str) -> Request {
    Request::CreateTicket {
        project: "game".into(),
        title: title.into(),
        goal: String::new(),
        hire: String::new(),
        base: None,
        detail: String::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_router_picks_the_hire_for_a_ticket() {
    let setup = setup("");
    let router = FakeRuntime::new(vec![reply("Plain code work.\nHIRE: dev")]);
    let studio = with_router(&setup, router.clone());
    let ticket = ok(&studio, unrouted("Add a level")).await;
    assert_eq!(ticket["hire"], "dev");
    let prompt = &router.specs()[0].prompt;
    assert!(prompt.contains("- alpha: Alpha") && prompt.contains("- dev: Dev"));
    assert!(!prompt.contains("- router:"));
    assert!(run_log(&setup).contains("routed to dev: Plain code work."));
    let spend = ok(
        &studio,
        Request::TicketSpend {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!(spend.as_f64().unwrap() > 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unusable_routing_answer_falls_back_to_the_first_engineer() {
    let setup = setup("");
    let studio = with_router(&setup, FakeRuntime::new(vec![reply("HIRE: nobody")]));
    let ticket = ok(&studio, unrouted("Add a level")).await;
    assert_eq!(ticket["hire"], "alpha");
    assert!(run_log(&setup).contains("routed to alpha"));
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_router_the_first_engineer_takes_it_and_without_one_it_errors() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    assert_eq!(ok(&studio, unrouted("x")).await["hire"], "dev");
    fs::remove_file(setup.paths.config_dir.join("hires/dev.md")).unwrap();
    let studio = studio_reopened(&setup);
    let failed = studio.handle(unrouted("y")).await;
    assert!(failed.error.unwrap().contains("no router or engineer"));
}

fn studio_reopened(setup: &Setup) -> Arc<Studio> {
    studio(setup, FakeRuntime::new(script()))
}
#[tokio::test]
async fn testing_a_rule_that_cannot_be_saved_is_an_error() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(vec![]));
    let bad = [
        "---\nkind: lint\nlevel: warn\npattern: \"(\"\n---\nNo.\n",
        "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"[\"]\n---\nNo.\n",
    ];
    for text in bad {
        let reply = studio
            .handle(Request::TestRule {
                text: text.into(),
                since: 0,
            })
            .await;
        assert!(!reply.ok, "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rule_files_come_from_the_studio_its_packs_and_its_projects() {
    let pack = tempfile::tempdir().unwrap();
    let guide = "---\nkind: guide\nlevel: warn\n---\nKeep it small.\n";
    fs::create_dir_all(pack.path().join("rules")).unwrap();
    for name in ["no-push", "old"] {
        fs::write(pack.path().join(format!("rules/{name}.md")), guide).unwrap();
    }
    let extra = format!(
        "[[packs]]\nname = \"p\"\npath = \"{}\"\n\n[rules]\ndisable = [\"pack/p/old\"]\n",
        pack.path().display()
    );
    let setup = setup_with("", "daily_usd = 5.0", &extra);
    write_rule(&setup, "style", guide);
    let project = setup.repo.join(".studio/rules");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("protect.md"),
        "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"src/**\"]\n---\nAsk first.\n",
    )
    .unwrap();
    fs::write(project.join("broken.md"), "oops").unwrap();
    let studio = studio(&setup, FakeRuntime::new(Vec::new()));

    let files = ok(&studio, Request::RuleFiles).await;
    let files = files.as_array().unwrap();
    let found: Vec<(&str, &str, &str)> = files
        .iter()
        .map(|f| {
            (
                f["origin"].as_str().unwrap(),
                f["owner"].as_str().unwrap(),
                f["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("studio", "", "style"),
            ("pack", "p", "no-push"),
            ("pack", "p", "old"),
            ("project", "game", "broken"),
            ("project", "game", "protect"),
        ]
    );
    assert_eq!(files[0]["text"], guide);
    assert!(
        files[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("rules/style.md")
    );
    assert_eq!(
        (
            files[1]["disabled"].as_bool(),
            files[2]["disabled"].as_bool()
        ),
        (Some(false), Some(true))
    );
    assert!(
        files[3]["problem"]
            .as_str()
            .unwrap()
            .contains("frontmatter")
    );
    assert!(files[4]["problem"].is_null());
    assert_eq!(ok(&studio, Request::Status).await["rules"], 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn hires_today_counts_passed_work_reviews_and_the_approvers_calls() {
    let setup = setup("");
    add_reviewer(&setup, "fake-2");
    fs::write(
        setup.paths.config_dir.join("hires/gatekeeper.md"),
        "---\nname: Gatekeeper\nrole: approver\nruntime: fake\nmodel: fake-1\n---\nYou decide.\n",
    )
    .unwrap();
    let author = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let reviewer = FakeRuntime::new(vec![reply("Does what it says.\nVERDICT: PASS")]);
    let studio = studio_with(&setup, author, reviewer);
    ok(&studio, create("Reviewed")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    // The approver's calls are notes in the run logs, which don't name it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let notes = [
        json!({"at": now, "event": {"type": "permission", "request_id": "r", "tool": "Bash",
                                    "input": {"command": "curl example.com"}}}),
        json!({"at": now, "note": "approver denied Bash: not needed"}),
        json!({"at": now - 2 * 86_400_000, "note": "approver allowed Bash: too old"}),
    ]
    .map(|n| n.to_string())
    .join("\n");
    fs::write(setup.paths.data_dir.join("runs/GAME-9.jsonl"), notes).unwrap();

    let today = ok(&studio, Request::HiresToday).await;
    assert_eq!(today["dev"]["done"], 1);
    assert_eq!(today["critic"]["reviews"], 1);
    assert_eq!(
        (
            today["gatekeeper"]["denied"].as_u64(),
            today["gatekeeper"]["allowed"].as_u64()
        ),
        (Some(1), Some(0))
    );

    let calls = ok(&studio, Request::ApproverCalls { limit: 10 }).await;
    assert_eq!(calls.as_array().unwrap().len(), 1, "{calls}");
    assert_eq!(
        (&calls[0]["ticket"], &calls[0]["verdict"], &calls[0]["what"]),
        (
            &json!("GAME-9"),
            &json!("denied"),
            &json!("curl example.com")
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_gives_the_run_slots_and_the_team() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(Vec::new()));
    let status = ok(&studio, Request::Status).await;
    assert_eq!(status["max_runs"], 2);
    assert_eq!(
        status["team"],
        json!([{"id": "dev", "role": "engineer", "model": "fake-1"}])
    );
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_events_and_check_notes_say_what_happened_when() {
    let setup = setup(r#"["sh", "-c", "test -f README.md"]"#);
    let before = now_ms();
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || ok(&studio, Request::Asks).await.as_array().unwrap().len() == 1).await;
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let recent = ok(
        &studio,
        Request::RecentEvents {
            since_ms: before,
            limit: None,
            kinds: Vec::new(),
        },
    )
    .await;
    let kinds: Vec<&str> = recent
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"created"), "{kinds:?}");
    assert!(kinds.contains(&"ask_opened") && kinds.contains(&"checks_started"));
    let later = ok(
        &studio,
        Request::RecentEvents {
            since_ms: now_ms() + 60_000,
            limit: None,
            kinds: Vec::new(),
        },
    )
    .await;
    assert_eq!(later, json!([]));
    // Asked for by type, only those come back.
    let asked = ok(
        &studio,
        Request::RecentEvents {
            since_ms: before,
            limit: None,
            kinds: vec!["ask_opened".into(), "checks_started".into()],
        },
    )
    .await;
    let mut kinds: Vec<&str> = asked
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"]["type"].as_str().unwrap())
        .collect();
    kinds.dedup();
    assert_eq!(kinds, ["ask_opened", "checks_started"]);
    // The run log names each check as it starts.
    assert!(run_log(&setup).contains("check: sh -c test -f README.md"));
}

/// Waits until some ask is open.
async fn an_ask(studio: &Arc<Studio>) {
    until(async || {
        !ok(studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
}

fn commit_on(repo: &Path, file: &str, text: &str, message: &str) {
    fs::write(repo.join(file), text).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-q", "-m", message]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_landing_that_conflicts_goes_back_to_its_agent_to_resolve() {
    let setup = setup("");
    let runtime = FakeRuntime::new(script());
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    an_ask(&studio).await;
    let worktree = setup.repo.join(".studio/worktrees/game-1");
    fs::write(worktree.join("README.md"), "game\nticket\n").unwrap();
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    // Meanwhile the owner changed the same line on master, where the goal
    // branch starts.
    commit_on(&setup.repo, "README.md", "game\nowner\n", "Owner edit");

    let landing = studio
        .handle(Request::Land {
            ticket: "GAME-1".into(),
        })
        .await;
    assert!(!landing.ok);
    let error = landing.error.unwrap();
    assert!(
        error.contains("conflicts with studio/goal/first-level in: README.md"),
        "{error}"
    );
    let events = events_of(&studio, "GAME-1").await;
    assert!(events.iter().any(|e| e["type"] == "conflicted"));

    // It runs again with the goal branch merged in and its agent told why.
    an_ask(&studio).await;
    let text = fs::read_to_string(worktree.join("README.md")).unwrap();
    assert!(text.contains("<<<<<<< ") && text.contains("owner") && text.contains("ticket"));
    let prompt = runtime.specs().last().unwrap().prompt.clone();
    assert!(
        prompt.contains("conflicts with it in: README.md"),
        "{prompt}"
    );
    // Markers left in send it back without a commit.
    approve_all(&studio).await;
    until(async || run_log(&setup).contains("conflict markers are still in: README.md")).await;
    an_ask(&studio).await;
    fs::write(worktree.join("README.md"), "game\nowner\nticket\n").unwrap();
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    let goal = "studio/goal/first-level";
    assert_eq!(
        git(&setup.repo, &["show", &format!("{goal}:README.md")]),
        "game\nowner\nticket"
    );
    let subjects = git(&setup.repo, &["log", "--format=%s", goal]);
    assert!(
        subjects.contains("Merge branch 'studio/goal/first-level' into studio/game-1"),
        "{subjects}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_goal_that_conflicts_with_its_base_gets_a_ticket_to_merge_it() {
    let setup = setup("");
    let studio = studio(&setup, FakeRuntime::new(script()));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    an_ask(&studio).await;
    fs::write(
        setup.repo.join(".studio/worktrees/game-1/README.md"),
        "game\nticket\n",
    )
    .unwrap();
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    commit_on(&setup.repo, "README.md", "game\nowner\n", "Owner edit");

    let merge = || {
        studio.handle(Request::Merge {
            ticket: "GAME-1".into(),
        })
    };
    let first = merge().await;
    assert!(!first.ok);
    let error = first.error.unwrap();
    assert!(
        error.contains("conflicts with master in: README.md; GAME-2 merges master"),
        "{error}"
    );
    // Asking again points at the same ticket instead of filing another.
    assert!(merge().await.error.unwrap().contains("GAME-2"));
    let tickets = ok(&studio, Request::Tickets).await;
    let tickets = tickets.as_array().unwrap();
    assert_eq!(tickets.len(), 2);
    let fix = tickets.iter().find(|t| t["id"] == "GAME-2").unwrap();
    assert_eq!(fix["goal"], "First level");
    assert_eq!(fix["source"], "merge:master");

    an_ask(&studio).await;
    let worktree = setup.repo.join(".studio/worktrees/game-2");
    let text = fs::read_to_string(worktree.join("README.md")).unwrap();
    assert!(text.contains("<<<<<<< "), "{text}");
    let log = fs::read_to_string(setup.paths.data_dir.join("runs/GAME-2.jsonl")).unwrap();
    assert!(log.contains("merged master; conflicts in: README.md"));
    fs::write(worktree.join("README.md"), "game\nowner\nticket\n").unwrap();
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-2").await == "ready").await;
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-2".into(),
        },
    )
    .await;

    ok(
        &studio,
        Request::Merge {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert_eq!(
        git(&setup.repo, &["show", "master:README.md"]),
        "game\nowner\nticket"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_held_for_quota_tries_again_once_the_limit_resets() {
    let setup = setup("");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let resets = now + 3600;
    let refused = vec![
        RunEvent::Started {
            session: "s1".into(),
            model: Some("fake-1".into()),
        },
        RunEvent::RateLimit {
            status: "rejected".into(),
            resets_at: Some(resets),
        },
        RunEvent::Finished {
            outcome: Outcome::Failed {
                kind: FailureKind::Quota,
                message: "Claude usage limit reached".into(),
            },
            session: Some("s1".into()),
            cost: Cost::default(),
            summary: None,
        },
    ];
    let studio = studio(&setup, FakeRuntime::sequence(vec![refused, script()]));
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await["held"]["reason"] == "quota").await;
    let log = run_log(&setup);
    assert!(
        log.contains("Claude usage limit reached; trying again in 1 h"),
        "{log}"
    );

    // Not before the limit resets, but a minute after.
    assert_eq!(studio.resume_quota_held(resets * 1000).unwrap(), 0);
    assert_eq!(studio.resume_quota_held(resets * 1000 + 60_000).unwrap(), 1);
    an_ask(&studio).await;
    approve_all(&studio).await;
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn evidence_and_image_pairs_outlive_the_worktree() {
    let setup = setup("");
    write_project(
        &setup,
        &project_with_art(&format!("{CAPTURE}\ncapture_paths = [\"assets/**\"]")),
    );
    let studio = studio(&setup, FakeRuntime::new(vec![finished(0.01, true, false)]));
    ok(&studio, create("Art")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    // What a gate leaves for its critic: the shipped and the new image.
    let worktree = setup.repo.join(".studio/worktrees/game-1");
    for name in ["before-hero-png.png", "after-hero-png.png"] {
        fs::write(worktree.join(".studio-evidence/0").join(name), name).unwrap();
    }
    let evidence = async || {
        let files = ok(
            &studio,
            Request::Evidence {
                ticket: "GAME-1".into(),
            },
        )
        .await;
        files
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let path = f["path"].as_str().unwrap().to_string();
                (f["name"].as_str().unwrap().to_string(), path)
            })
            .collect::<Vec<_>>()
    };
    let names =
        |files: &[(String, String)]| files.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    let before = evidence().await;
    assert_eq!(
        names(&before),
        ["after-hero-png.png", "before-hero-png.png", "title.png"]
    );

    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    assert!(!worktree.exists(), "{}", run_log(&setup));
    let after = evidence().await;
    assert_eq!(names(&after), names(&before));
    let kept = setup.paths.data_dir.join("evidence/GAME-1/0");
    for (name, path) in &after {
        assert_eq!(Path::new(path), kept.join(name));
        assert!(Path::new(path).is_file());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_projects_env_reaches_its_agents_and_checks() {
    let setup = setup("");
    write_project(
        &setup,
        r#"verify = [["sh", "-c", "printf %s \"$SHARED\" > shared.txt"]]

[env]
SHARED = "{repo}/cache"
"#,
    );
    let runtime = FakeRuntime::new(vec![finished(0.01, true, false)]);
    let studio = studio(&setup, runtime.clone());
    ok(&studio, create("Build")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let want = format!("{}/cache", setup.repo.display());
    let worktree = setup.repo.join(".studio/worktrees/game-1");
    assert_eq!(
        fs::read_to_string(worktree.join("shared.txt")).unwrap(),
        want
    );
    let spec = &runtime.specs()[0];
    assert!(
        spec.env.contains(&("SHARED".to_string(), want)),
        "{:?}",
        spec.env
    );
}
