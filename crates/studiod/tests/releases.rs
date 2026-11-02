//! Releases through the daemon: final checks, the owner's go-ahead, the
//! project's release commands and the tag, in a temporary git repository.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use studio_config::StudioPaths;
use studio_runtime::Runtime;
use studiod::Studio;
use studiod::api::{Request, Response};
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
    _dirs: (TempDir, TempDir, TempDir),
    repo: PathBuf,
    /// Where the release commands write.
    out: PathBuf,
    paths: StudioPaths,
}

/// A repository whose `.studio.toml` has `verify` and `release` as TOML
/// lists of commands, and a studio home that knows it.
fn setup(verify: &str, release: &str) -> Setup {
    let repo_dir = tempfile::tempdir().unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let repo = repo_dir.path().to_path_buf();
    let out = out_dir.path().to_path_buf();
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
    let out_text = out.display().to_string();
    let verify = verify.replace("{out}", &out_text);
    let release = release.replace("{out}", &out_text);
    fs::write(
        repo.join(".studio.toml"),
        format!("verify = [{verify}]\nrelease = [{release}]\n\n[env]\nSHIP = \"yes\"\n"),
    )
    .unwrap();

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
    let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
    Setup {
        repo,
        out,
        paths,
        _dirs: (repo_dir, out_dir, home),
    }
}

fn open(setup: &Setup) -> Arc<Studio> {
    let runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
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

/// The error a request is refused with.
async fn refused(studio: &Arc<Studio>, request: Request) -> String {
    let response = call(studio, request).await;
    assert!(
        !response.ok,
        "should have been refused: {:?}",
        response.data
    );
    response.error.unwrap()
}

fn start(version: &str) -> Request {
    Request::StartRelease {
        project: "game".into(),
        version: version.into(),
    }
}

fn approve() -> Request {
    Request::ApproveRelease {
        project: "game".into(),
    }
}

fn cancel() -> Request {
    Request::CancelRelease {
        project: "game".into(),
    }
}

async fn releases(studio: &Arc<Studio>) -> Vec<Value> {
    let all = ok(
        studio,
        Request::Releases {
            project: Some("game".into()),
        },
    )
    .await;
    all.as_array().unwrap().clone()
}

/// The newest release.
async fn latest(studio: &Arc<Studio>) -> Value {
    releases(studio).await.remove(0)
}

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

async fn until_state(studio: &Arc<Studio>, state: &str) -> Value {
    until(async || latest(studio).await["state"] == state).await;
    latest(studio).await
}

fn done(release: &Value) -> Vec<bool> {
    release["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["done"].as_bool().unwrap())
        .collect()
}

fn names(release: &Value) -> Vec<String> {
    release["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect()
}

async fn project_view(studio: &Arc<Studio>) -> Value {
    ok(studio, Request::Office).await["projects"][0].clone()
}

/// Release commands that write the version, the project's env and the commit
/// they ran on into `{out}`.
const PUBLISH: &str = r#"["sh", "-c", "printf '%s %s' \"$VERSION\" \"$SHIP\" > {out}/published.txt && git rev-parse HEAD > {out}/commit.txt"], ["sh", "-c", "touch {out}/second.txt"]"#;

/// A check that waits until `{out}/go` exists, so a test can hold a release
/// in its final checks.
const WAIT_FOR_GO: &str =
    r#"["sh", "-c", "touch {out}/checking; while [ ! -f {out}/go ]; do sleep 0.05; done"]"#;

#[tokio::test]
async fn a_release_waits_after_its_checks_then_publishes_and_tags_on_approval() {
    let s = setup(r#"["sh", "-c", "touch {out}/checked"]"#, PUBLISH);
    let studio = open(&s);
    let tip = git(&s.repo, &["rev-parse", "master"]);

    let started = ok(&studio, start("0.4")).await;
    assert_eq!(started["version"], "0.4");
    assert_eq!(started["project"], "game");
    assert!(started["started_at"].as_u64().unwrap() > 0);

    let waiting = until_state(&studio, "waiting").await;
    let steps = names(&waiting);
    assert_eq!(steps.len(), 5);
    assert_eq!(steps[0], "Final checks");
    assert_eq!(steps[1], "Your go-ahead");
    // A release command is named by what it runs, cut short.
    assert!(steps[2].starts_with("sh -c printf"), "{}", steps[2]);
    assert!(steps[2].ends_with('…') && steps[2].chars().count() <= 40);
    assert!(steps[3].starts_with("sh -c touch"), "{}", steps[3]);
    assert_eq!(steps[4], "Tag v0.4");
    assert_eq!(done(&waiting), [true, false, false, false, false]);
    assert!(waiting["steps"][0]["at"].as_u64().unwrap() > 0);
    assert!(s.out.join("checked").exists());
    // Nothing is published or tagged before the go-ahead.
    assert!(!s.out.join("published.txt").exists());
    assert_eq!(git(&s.repo, &["tag", "-l"]), "");

    // The Office shows it under way, releasable, with nothing published yet.
    let view = project_view(&studio).await;
    assert_eq!(view["name"], "game");
    assert_eq!(view["releasable"], true);
    assert_eq!(view["release"]["state"], "waiting");
    assert_eq!(view["release"]["version"], "0.4");
    assert!(view["last_release"].is_null());

    ok(&studio, approve()).await;
    let released = until_state(&studio, "released").await;
    assert_eq!(done(&released), [true; 5]);
    assert!(released["finished_at"].as_u64().is_some());
    assert!(released["error"].is_null());

    // The commands ran with the version and the project's env, on the tip.
    assert_eq!(
        fs::read_to_string(s.out.join("published.txt")).unwrap(),
        "0.4 yes"
    );
    assert_eq!(
        fs::read_to_string(s.out.join("commit.txt")).unwrap().trim(),
        tip
    );
    assert!(s.out.join("second.txt").exists());

    // An annotated tag, made as the owner, on that commit.
    assert_eq!(git(&s.repo, &["tag", "-l"]), "v0.4");
    assert_eq!(git(&s.repo, &["cat-file", "-t", "v0.4"]), "tag");
    assert_eq!(git(&s.repo, &["rev-parse", "v0.4^{commit}"]), tip);
    assert_eq!(
        git(
            &s.repo,
            &["for-each-ref", "--format=%(taggername)", "refs/tags/v0.4"]
        ),
        "Owner"
    );
    // The throwaway checkout is gone.
    assert!(!git(&s.repo, &["worktree", "list"]).contains("releases"));

    let view = project_view(&studio).await;
    assert!(view["release"].is_null());
    assert_eq!(view["last_release"]["version"], "0.4");
    assert_eq!(view["last_release"]["state"], "released");

    // It is on record, and a new release may start.
    assert_eq!(releases(&studio).await.len(), 1);
    assert!(
        refused(&studio, start("0.4"))
            .await
            .contains("already has a tag v0.4")
    );
}

#[tokio::test]
async fn a_version_with_a_v_keeps_it_and_the_tag_is_on_the_checked_commit() {
    let s = setup("", PUBLISH);
    let studio = open(&s);
    let tip = git(&s.repo, &["rev-parse", "master"]);
    ok(&studio, start("v1.0-rc1")).await;
    until_state(&studio, "waiting").await;
    // Work landing on the branch while the release waits is not released.
    fs::write(s.repo.join("later.txt"), "later\n").unwrap();
    git(&s.repo, &["add", "-A"]);
    git(&s.repo, &["commit", "-q", "-m", "Later"]);
    ok(&studio, approve()).await;
    until_state(&studio, "released").await;
    assert_eq!(git(&s.repo, &["tag", "-l"]), "v1.0-rc1");
    assert_eq!(git(&s.repo, &["rev-parse", "v1.0-rc1^{commit}"]), tip);
    assert_eq!(
        fs::read_to_string(s.out.join("commit.txt")).unwrap().trim(),
        tip
    );
    assert_eq!(
        fs::read_to_string(s.out.join("published.txt")).unwrap(),
        "v1.0-rc1 yes"
    );
}

#[tokio::test]
async fn failing_checks_fail_the_release_with_the_reason() {
    let s = setup(
        r#"["sh", "-c", "echo nothing to see"], ["sh", "-c", "echo the tests broke >&2; exit 1"]"#,
        PUBLISH,
    );
    let studio = open(&s);
    ok(&studio, start("0.5")).await;
    let failed = until_state(&studio, "failed").await;
    let error = failed["error"].as_str().unwrap();
    assert!(error.contains("the tests broke"), "{error}");
    assert!(error.contains("echo the tests broke"), "{error}");
    assert_eq!(done(&failed), [false; 5]);
    assert!(failed["finished_at"].as_u64().is_some());
    assert!(!s.out.join("published.txt").exists());
    assert_eq!(git(&s.repo, &["tag", "-l"]), "");
    assert!(!git(&s.repo, &["worktree", "list"]).contains("releases"));

    // Failed in the last day, so the Office still shows it; none published.
    let view = project_view(&studio).await;
    assert_eq!(view["release"]["state"], "failed");
    assert!(view["last_release"].is_null());

    // Nothing is waiting to be approved or cancelled.
    assert!(refused(&studio, approve()).await.contains("no release"));
    assert!(refused(&studio, cancel()).await.contains("no release"));
}

#[tokio::test]
async fn a_failing_release_command_fails_the_release_before_the_tag() {
    let s = setup(
        "",
        r#"["sh", "-c", "touch {out}/first.txt"], ["sh", "-c", "echo upload refused >&2; exit 3"], ["sh", "-c", "touch {out}/third.txt"]"#,
    );
    let studio = open(&s);
    ok(&studio, start("2.0")).await;
    until_state(&studio, "waiting").await;
    ok(&studio, approve()).await;
    let failed = until_state(&studio, "failed").await;
    let error = failed["error"].as_str().unwrap();
    assert!(error.contains("upload refused"), "{error}");
    assert!(error.contains("exit 3"), "{error}");
    // The first command ran, the failing one stopped it.
    assert_eq!(done(&failed), [true, true, true, false, false, false]);
    assert!(s.out.join("first.txt").exists());
    assert!(!s.out.join("third.txt").exists());
    assert_eq!(git(&s.repo, &["tag", "-l"]), "");
    let view = project_view(&studio).await;
    assert_eq!(view["release"]["state"], "failed");
    assert!(view["last_release"].is_null());
}

#[tokio::test]
async fn a_release_can_be_cancelled_while_it_checks_and_while_it_waits() {
    let s = setup(WAIT_FOR_GO, PUBLISH);
    let studio = open(&s);
    ok(&studio, start("0.6")).await;
    until(async || s.out.join("checking").exists()).await;

    // Still checking: it can't be approved yet, but it can be stopped.
    assert!(refused(&studio, approve()).await.contains("still running"));
    let cancelled = ok(&studio, cancel()).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert!(cancelled["finished_at"].as_u64().is_some());
    // The checks were stopped: letting them go on changes nothing.
    fs::write(s.out.join("go"), "").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(latest(&studio).await["state"], "cancelled");
    assert!(!s.out.join("published.txt").exists());
    let view = project_view(&studio).await;
    assert_eq!(view["release"]["state"], "cancelled");

    // The same version can start again, and be cancelled while it waits.
    ok(&studio, start("0.6")).await;
    until_state(&studio, "waiting").await;
    let cancelled = ok(&studio, cancel()).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert!(refused(&studio, approve()).await.contains("no release"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!s.out.join("published.txt").exists());
    assert_eq!(git(&s.repo, &["tag", "-l"]), "");
    let all = releases(&studio).await;
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|r| r["state"] == "cancelled"));
    assert!(!git(&s.repo, &["worktree", "list"]).contains("releases"));
}

#[tokio::test]
async fn what_cannot_be_released_is_refused_with_a_reason() {
    let s = setup(WAIT_FOR_GO, PUBLISH);
    let studio = open(&s);
    git(&s.repo, &["tag", "v0.1"]);

    let unknown = Request::StartRelease {
        project: "nope".into(),
        version: "1.0".into(),
    };
    assert!(
        refused(&studio, unknown)
            .await
            .contains("no project named nope")
    );
    for odd in ["", "1 0", "1/0", "-1", "1.0;x", "é"] {
        let why = refused(&studio, start(odd)).await;
        assert!(why.contains("version"), "{odd:?} was refused with: {why}");
    }
    assert!(
        refused(&studio, start("0.1"))
            .await
            .contains("already has a tag v0.1")
    );
    assert!(
        refused(&studio, start("v0.1"))
            .await
            .contains("already has a tag v0.1")
    );
    assert!(releases(&studio).await.is_empty());

    // One release at a time.
    ok(&studio, start("0.2")).await;
    let why = refused(&studio, start("0.3")).await;
    assert!(why.contains("already has release 0.2 under way"), "{why}");
    ok(&studio, cancel()).await;
    ok(&studio, start("0.3")).await;
    ok(&studio, cancel()).await;
}

#[tokio::test]
async fn a_project_without_release_commands_says_how_to_add_them() {
    let s = setup("", "");
    let studio = open(&s);
    let why = refused(&studio, start("1.0")).await;
    assert!(why.contains("no release commands"), "{why}");
    assert!(why.contains(".studio.toml"), "{why}");
    assert!(why.contains("release = "), "{why}");
    let view = project_view(&studio).await;
    assert_eq!(view["releasable"], false);
    assert!(view["release"].is_null());
    assert!(releases(&studio).await.is_empty());
}

#[tokio::test]
async fn a_restart_fails_a_release_that_was_checking() {
    let s = setup(WAIT_FOR_GO, PUBLISH);
    let first = open(&s);
    ok(&first, start("0.7")).await;
    until(async || s.out.join("checking").exists()).await;
    assert_eq!(latest(&first).await["state"], "checking");

    // studiod starts again over the same home: the release did not survive.
    let second = open(&s);
    let failed = latest(&second).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"], "studiod stopped");
    assert_eq!(failed["version"], "0.7");
    assert!(failed["finished_at"].as_u64().is_some());
    let view = project_view(&second).await;
    assert_eq!(view["release"]["state"], "failed");
    assert_eq!(view["release"]["error"], "studiod stopped");
    assert!(refused(&second, approve()).await.contains("no release"));

    // The version can be released again.
    let again = ok(&second, start("0.7")).await;
    assert_eq!(again["state"], "checking");
    assert_eq!(releases(&second).await.len(), 2);
}

#[tokio::test]
async fn a_restart_fails_a_release_that_was_waiting() {
    let s = setup("", PUBLISH);
    let first = open(&s);
    ok(&first, start("0.8")).await;
    until_state(&first, "waiting").await;

    let second = open(&s);
    let failed = latest(&second).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"], "studiod stopped");
    assert!(refused(&second, approve()).await.contains("no release"));
    assert!(!s.out.join("published.txt").exists());
    assert_eq!(git(&s.repo, &["tag", "-l"]), "");
    assert!(!git(&s.repo, &["worktree", "list"]).contains("releases"));
}

#[tokio::test]
async fn releases_list_newest_first_across_projects() {
    let s = setup("", PUBLISH);
    let studio = open(&s);
    for version in ["1.0", "1.1"] {
        ok(&studio, start(version)).await;
        until_state(&studio, "waiting").await;
        ok(&studio, approve()).await;
        until_state(&studio, "released").await;
    }
    let all = ok(&studio, Request::Releases { project: None }).await;
    let versions: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["version"].as_str().unwrap())
        .collect();
    assert_eq!(versions, ["1.1", "1.0"]);
    let view = project_view(&studio).await;
    assert_eq!(view["last_release"]["version"], "1.1");
    assert!(
        refused(
            &studio,
            Request::Releases {
                project: Some("nope".into())
            }
        )
        .await
        .contains("no project named nope")
    );
}
