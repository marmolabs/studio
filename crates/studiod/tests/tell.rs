//! Telling a task's agent something, the one-line results of finished tasks
//! and the plain questions of the asks, through the daemon with a scripted
//! runtime.
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
use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};
use studiod::Studio;
use studiod::api::{LeadAct, Request, Response};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
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
}

struct Setup {
    _dirs: (TempDir, TempDir),
    paths: StudioPaths,
}

/// A repository whose project checks run `verify`, and a studio home that
/// knows it, with the hire `dev` on the fake runtime.
fn setup(verify: &str) -> Setup {
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
            "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
             [[projects]]\nname = \"game\"\npath = \"{}\"\nintegration_branch = \"master\"\n",
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
        paths,
        _dirs: (repo_dir, home),
    }
}

fn summary(text: &str) -> RunEvent {
    RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("s1".into()),
        cost: Cost::default(),
        summary: Some(text.into()),
    }
}

fn started() -> RunEvent {
    RunEvent::Started {
        session: "s1".into(),
        model: Some("fake-1".into()),
    }
}

fn studio(setup: &Setup, runtime: &FakeRuntime) -> Arc<Studio> {
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(runtime.clone()));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

async fn call(studio: &Arc<Studio>, request: Request) -> Response {
    studio.handle(request).await
}

async fn ok(studio: &Arc<Studio>, request: Request) -> Value {
    let Response { ok, data, error } = call(studio, request).await;
    assert!(ok, "request failed: {error:?}");
    data
}

async fn fails(studio: &Arc<Studio>, request: Request) -> String {
    let response = call(studio, request).await;
    assert!(!response.ok, "request worked: {:?}", response.data);
    response.error.unwrap()
}

/// Polls until `check` passes. The limit only catches a hang.
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
    Request::CreateTicket {
        project: "game".into(),
        title: title.into(),
        goal: "First level".into(),
        hire: "dev".into(),
        base: None,
        detail: String::new(),
    }
}

fn tell(task: &str, text: &str) -> Request {
    Request::Tell {
        task: task.into(),
        text: text.into(),
    }
}

fn run_log(setup: &Setup, task: &str) -> String {
    fs::read_to_string(setup.paths.data_dir.join(format!("runs/{task}.jsonl"))).unwrap()
}

/// Nora, on the fake runtime, leads the game.
fn nora_leads_the_game(setup: &Setup) {
    let config = setup.paths.config_dir.join("studio.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        text.replace(
            "integration_branch = \"master\"\n",
            "integration_branch = \"master\"\nlead = \"nora\"\n",
        ),
    )
    .unwrap();
    fs::write(
        setup.paths.config_dir.join("hires/nora.md"),
        "---\nname: Nora\nrole: lead\nruntime: fake\nmodel: fake-1\n---\nYou lead the game.\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leads_message_reaches_the_working_agent_as_from_the_lead() {
    let setup = setup("");
    nora_leads_the_game(&setup);
    let (runtime, release) = FakeRuntime::new(vec![started(), summary("Added the level.")]).held();
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "working").await;

    let acts = |task: &str, act| Request::LeadActs {
        project: "game".into(),
        task: task.into(),
        act,
    };
    let text = "  Use three colours.  ".to_string();
    let told = ok(&studio, acts("GAME-1", LeadAct::Tell { text })).await;
    assert_eq!(told, json!({"task": "GAME-1", "how": "live"}));
    until(async || !runtime.told().is_empty()).await;
    // The agent hears who it is from, in its session and in the run's log.
    assert_eq!(runtime.told(), ["Message from Nora: Use three colours."]);
    assert!(run_log(&setup, "GAME-1").contains("Message from Nora: Use three colours."));
    assert!(!run_log(&setup, "GAME-1").contains("Message from the owner"));

    // A task the game doesn't have is not the game's to tell.
    let text = "Hello".to_string();
    let error = fails(&studio, acts("GAME-9", LeadAct::Tell { text })).await;
    assert_eq!(error, "GAME-9 isn't one of game's tasks.");
    release.notify_one();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_working_agent_hears_the_message_in_its_live_session() {
    let setup = setup("");
    let (runtime, release) = FakeRuntime::new(vec![started(), summary("Added the level.")]).held();
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "working").await;

    let told = ok(&studio, tell("GAME-1", "  Use three colours.  ")).await;
    assert_eq!(told, json!({"task": "GAME-1", "how": "live"}));
    until(async || !runtime.told().is_empty()).await;
    assert_eq!(
        runtime.told(),
        ["Message from the owner: Use three colours."]
    );
    assert!(run_log(&setup, "GAME-1").contains("Message from the owner: Use three colours."));

    // The run goes on to its end as before.
    release.notify_one();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_work_goes_back_to_its_agent_with_the_message() {
    let setup = setup("");
    let runtime = FakeRuntime::new(vec![started(), summary("Added the level.")]);
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;

    let told = ok(&studio, tell("GAME-1", "Use three colours.")).await;
    assert_eq!(told, json!({"task": "GAME-1", "how": "sent_back"}));
    // It runs again, and its agent is told what to change.
    until(async || runtime.specs().len() == 2).await;
    assert!(
        runtime.specs()[1]
            .prompt
            .contains("The owner sent this back with: Use three colours.")
    );
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    assert!(run_log(&setup, "GAME-1").contains("Message from the owner: Use three colours."));
}

#[tokio::test(flavor = "multi_thread")]
async fn work_in_its_checks_goes_back_and_does_not_wedge() {
    let setup = setup(r#"["sh", "-c", "sleep 1"]"#);
    let runtime = FakeRuntime::new(vec![started(), summary("Added the level.")]);
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "checking").await;

    let told = ok(&studio, tell("GAME-1", "Use three colours.")).await;
    assert_eq!(told["how"], "sent_back");
    until(async || runtime.specs().len() == 2).await;
    assert!(
        runtime.specs()[1]
            .prompt
            .contains("The owner sent this back with: Use three colours.")
    );
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_task_keeps_the_message_for_its_next_run() {
    let setup = setup("");
    let (runtime, release) = FakeRuntime::new(vec![started(), summary("Added the level.")]).held();
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;

    let told = ok(&studio, tell("GAME-1", "Use three colours.")).await;
    assert_eq!(told, json!({"task": "GAME-1", "how": "queued"}));
    ok(&studio, tell("GAME-1", "And keep it short.")).await;
    assert_eq!(state_of(&studio, "GAME-1").await, "queued");
    assert!(runtime.told().is_empty());

    studio.spawn_scheduler();
    until(async || runtime.told().len() == 2).await;
    assert_eq!(
        runtime.told(),
        [
            "Message from the owner: Use three colours.",
            "Message from the owner: And keep it short."
        ]
    );
    release.notify_one();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    // Heard once: a later run does not hear it again.
    assert!(!setup.paths.data_dir.join("talk/told/GAME-1.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_task_and_an_empty_message_are_errors() {
    let setup = setup("");
    let runtime = FakeRuntime::new(vec![started(), summary("Added the level.")]);
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;

    let error = fails(&studio, tell("GAME-1", "Too late.")).await;
    assert!(error.contains("GAME-1 is landed"), "{error}");
    assert!(error.contains("no one to tell"), "{error}");
    let error = fails(&studio, tell("GAME-1", "   ")).await;
    assert_eq!(error, "there is nothing to tell");
    assert!(
        fails(&studio, tell("GAME-9", "Hello"))
            .await
            .contains("GAME-9")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn finished_tasks_have_a_one_line_result() {
    let setup = setup("");
    let runtime = FakeRuntime::new(vec![
        started(),
        summary(
            "## Done\n\nAdded the **level** to `game.rs`. It has three colours.\n\n- tests pass",
        ),
    ]);
    let studio = studio(&setup, &runtime);
    ok(&studio, create("Add a level")).await;
    ok(&studio, create("Add a menu")).await;
    let office = ok(&studio, Request::Office).await;
    // Nothing has finished yet.
    assert_eq!(office["results"], json!({}));

    studio.spawn_scheduler();
    until(async || state_of(&studio, "GAME-1").await == "ready").await;
    let office = ok(&studio, Request::Office).await;
    assert_eq!(office["results"]["GAME-1"], "Added the level to game.rs.");
    // Shipped work keeps its line.
    ok(
        &studio,
        Request::Land {
            ticket: "GAME-1".into(),
        },
    )
    .await;
    let office = ok(&studio, Request::Office).await;
    assert_eq!(office["results"]["GAME-1"], "Added the level to game.rs.");
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

fn bash_then_finish() -> FakeRuntime {
    FakeRuntime::new(vec![
        RunEvent::Permission {
            request_id: "p1".into(),
            tool: "Bash".into(),
            input: json!({ "command": "cargo publish" }),
        },
        summary("Published."),
    ])
}

/// A studio whose approver is `approver`, and whose dev's first call is
/// `bash_then_finish`.
fn with_approver(setup: &Setup, approver: FakeRuntime) -> Arc<Studio> {
    fs::write(
        setup.paths.config_dir.join("hires/approver.md"),
        "---\nname: Approver\nrole: approver\nruntime: fake-approver\nmodel: fake-2\n---\nYou decide.\n",
    )
    .unwrap();
    let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
    runtimes.insert("fake".into(), Arc::new(bash_then_finish()));
    runtimes.insert("fake-approver".into(), Arc::new(approver));
    Studio::open(setup.paths.clone(), runtimes).unwrap()
}

async fn the_ask(studio: &Arc<Studio>) -> Value {
    ok(studio, create("Release")).await;
    studio.spawn_scheduler();
    until(async || {
        !ok(studio, Request::Asks)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    })
    .await;
    ok(studio, Request::Asks).await[0].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_the_approver_leaves_to_the_owner_comes_with_a_plain_question() {
    let setup = setup("");
    let approver = FakeRuntime::new(vec![reply(
        "It publishes the crate.\nQUESTION: Publish the crate?\nDECISION: OWNER: it publishes",
    )]);
    let studio = with_approver(&setup, approver);
    let ask = the_ask(&studio).await;
    assert_eq!(ask["title"], "Publish the crate?");
    assert_eq!(ask["tool"], "Bash");
    // The same shows in the office.
    let office = ok(&studio, Request::Office).await;
    assert_eq!(office["asks"][0]["title"], "Publish the crate?");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_without_a_question_has_an_empty_title() {
    let setup = setup("");
    let approver = FakeRuntime::new(vec![reply("DECISION: OWNER: it publishes")]);
    let studio = with_approver(&setup, approver);
    let ask = the_ask(&studio).await;
    assert_eq!(ask["title"], "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_question_is_cut_to_sixty_characters() {
    let setup = setup("");
    let long = format!(
        "QUESTION: {}?",
        "Publish every crate in the workspace ".repeat(4)
    );
    let approver = FakeRuntime::new(vec![reply(&format!(
        "{long}\nDECISION: OWNER: it publishes"
    ))]);
    let studio = with_approver(&setup, approver);
    let ask = the_ask(&studio).await;
    let title = ask["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 60, "{title}");
    assert!(title.starts_with("Publish every crate"));
}
