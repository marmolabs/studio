#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use studio_core::ids::TicketId;
use studio_git::{LandOutcome, MergeOutcome, RebaseOutcome, Repo};
use tempfile::TempDir;

/// Runs git for test setup with the developer's global and system config hidden.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `master` with one commit. Local config pins everything the library's
/// own git calls would otherwise pick up from the developer's global config.
fn init() -> (TempDir, Repo) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "master"]);
    let hooks = root.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for (key, value) in [
        ("user.name", "Test Owner"),
        ("user.email", "owner@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", hooks.to_str().unwrap()),
    ] {
        git(root, &["config", key, value]);
    }
    fs::write(root.join("README.md"), "hello\n").unwrap();
    fs::write(root.join("gone.txt"), "bye\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "Initial"]);
    let repo = Repo::open(root).unwrap();
    (dir, repo)
}

fn commit_on_master(root: &Path, file: &str, content: &str) -> String {
    fs::write(root.join(file), content).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "Master change"]);
    git(root, &["rev-parse", "HEAD"])
}

fn hook(repo: &Repo, name: &str, body: &str) {
    let path = repo.root().join(".git").join("hooks").join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn ticket(id: &str) -> TicketId {
    TicketId::new(id)
}

#[test]
fn worktree_pins_its_base() {
    let (_dir, repo) = init();
    let base = repo.resolve("master").unwrap();
    let wt = repo.create_worktree(&ticket("HUE-12"), "master").unwrap();
    assert_eq!(wt.base_commit, base);
    assert_eq!(wt.branch, "studio/hue-12");
    assert!(wt.path.ends_with(".studio/worktrees/hue-12"));

    commit_on_master(repo.root(), "README.md", "moved\n");
    let listed = repo.studio_worktrees().unwrap();
    assert_eq!(listed, vec![wt]);
    assert_eq!(listed[0].base_commit, base);
}

#[test]
fn file_at_base_reads_the_shipped_version() {
    let (_dir, repo) = init();
    let wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join("README.md"), "changed\n").unwrap();
    fs::write(wt.path.join("new.txt"), "new\n").unwrap();
    assert_eq!(
        wt.file_at_base("README.md").unwrap(),
        Some(b"hello\n".to_vec())
    );
    assert_eq!(wt.file_at_base("new.txt").unwrap(), None);
}

#[test]
fn creating_the_same_ticket_twice_fails() {
    let (_dir, repo) = init();
    repo.create_worktree(&ticket("t1"), "master").unwrap();
    assert!(repo.create_worktree(&ticket("t1"), "master").is_err());
}

#[test]
fn changes_include_modified_new_and_deleted_files() {
    let (_dir, repo) = init();
    let wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join("README.md"), "changed\n").unwrap();
    fs::write(wt.path.join("new.txt"), "new\n").unwrap();
    fs::remove_file(wt.path.join("gone.txt")).unwrap();

    let files = wt.changed_files().unwrap();
    let expected: Vec<PathBuf> = ["README.md", "gone.txt", "new.txt"]
        .iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(files, expected);

    let diff = wt.diff().unwrap();
    assert!(diff.contains("+changed"));
    assert!(diff.contains("+new"));
    assert!(diff.contains("-bye"));
    // Nothing was staged for real.
    assert_eq!(git(&wt.path, &["diff", "--cached", "--name-only"]), "");
}

#[test]
fn commit_strips_attribution_and_skips_empty_changes() {
    let (_dir, repo) = init();
    let wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    assert_eq!(wt.commit("Nothing").unwrap(), None);

    fs::write(wt.path.join("README.md"), "changed\n").unwrap();
    let sha = wt
        .commit("Change readme\n\nCo-Authored-By: Claude <noreply@anthropic.com>")
        .unwrap()
        .unwrap();
    assert_eq!(git(&wt.path, &["rev-parse", "HEAD"]), sha);
    assert_eq!(
        git(&wt.path, &["log", "-1", "--format=%B"]),
        "Change readme"
    );
    assert_eq!(git(&wt.path, &["log", "-1", "--format=%an"]), "Test Owner");
}

#[test]
fn hooks_run_on_commit() {
    let (_dir, repo) = init();
    hook(
        &repo,
        "commit-msg",
        "grep -qi 'co-authored-by' \"$1\" && exit 1\nexit 0",
    );
    let wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join("a.txt"), "a\n").unwrap();
    // The trailer is stripped before the hook sees the message.
    assert!(
        wt.commit("Add a\n\nCo-Authored-By: Bot <b@example.com>")
            .is_ok()
    );

    hook(&repo, "commit-msg", "exit 1");
    fs::write(wt.path.join("b.txt"), "b\n").unwrap();
    assert!(wt.commit("Add b").is_err());
}

#[test]
fn rebase_moves_the_base() {
    let (_dir, repo) = init();
    let mut wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join("ticket.txt"), "work\n").unwrap();
    wt.commit("Ticket work").unwrap();
    let new_master = commit_on_master(repo.root(), "other.txt", "other\n");

    let outcome = wt.rebase_onto("master").unwrap();
    assert_eq!(
        outcome,
        RebaseOutcome::Clean {
            new_base: new_master.clone()
        }
    );
    assert_eq!(wt.base_commit, new_master);
    assert!(wt.path.join("other.txt").exists());
    assert_eq!(repo.studio_worktrees().unwrap()[0].base_commit, new_master);
}

#[test]
fn rebase_conflict_leaves_the_worktree_alone() {
    let (_dir, repo) = init();
    let mut wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join("README.md"), "ticket\n").unwrap();
    let before = wt.commit("Ticket edit").unwrap().unwrap();
    commit_on_master(repo.root(), "README.md", "master\n");
    let base = wt.base_commit.clone();

    match wt.rebase_onto("master").unwrap() {
        RebaseOutcome::Conflict { files } => assert_eq!(files, vec!["README.md"]),
        other => panic!("expected conflict, got {other:?}"),
    }
    assert_eq!(git(&wt.path, &["rev-parse", "HEAD"]), before);
    assert_eq!(wt.base_commit, base);
    assert_eq!(git(&wt.path, &["status", "--porcelain"]), "");
}

#[test]
fn tickets_land_on_one_goal_branch() {
    let (_dir, repo) = init();
    let goal = repo.ensure_goal_branch("Daily mode", "master").unwrap();
    assert_eq!(goal, "studio/goal/daily-mode");
    assert_eq!(
        repo.ensure_goal_branch("Daily mode", "master").unwrap(),
        goal
    );

    let a = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(a.path.join("a.txt"), "a\n").unwrap();
    a.commit("Add a").unwrap();
    let b = repo.create_worktree(&ticket("t2"), "master").unwrap();
    fs::write(b.path.join("b.txt"), "b\n").unwrap();
    b.commit("Add b").unwrap();
    assert!(matches!(
        repo.land(&a, &goal).unwrap(),
        LandOutcome::Merged { .. }
    ));
    assert!(matches!(
        repo.land(&b, &goal).unwrap(),
        LandOutcome::Merged { .. }
    ));
    let landed = repo.resolve(&goal).unwrap();
    let tree = git(repo.root(), &["ls-tree", "--name-only", &goal]);
    assert!(tree.contains("a.txt") && tree.contains("b.txt"));

    let c = repo.create_worktree(&ticket("t3"), "master").unwrap();
    fs::write(c.path.join("a.txt"), "different\n").unwrap();
    c.commit("Clash with a").unwrap();
    match repo.land(&c, &goal).unwrap() {
        LandOutcome::Conflict { files } => assert_eq!(files, vec!["a.txt"]),
        other => panic!("expected conflict, got {other:?}"),
    }
    assert_eq!(repo.resolve(&goal).unwrap(), landed);

    // Goal worktrees are not ticket worktrees, and the owner's checkout is untouched.
    assert_eq!(repo.studio_worktrees().unwrap().len(), 3);
    assert_eq!(git(repo.root(), &["branch", "--show-current"]), "master");
}

#[test]
fn remove_and_prune() {
    let (_dir, repo) = init();
    let a = repo.create_worktree(&ticket("t1"), "master").unwrap();
    let b = repo.create_worktree(&ticket("t2"), "master").unwrap();
    repo.remove_worktree(&a, false).unwrap();
    fs::remove_dir_all(&b.path).unwrap();
    repo.prune().unwrap();
    assert!(repo.studio_worktrees().unwrap().is_empty());
}

#[test]
fn studio_dir_is_excluded_once() {
    let (_dir, repo) = init();
    repo.create_worktree(&ticket("t1"), "master").unwrap();
    repo.create_worktree(&ticket("t2"), "master").unwrap();
    let exclude = fs::read_to_string(repo.root().join(".git/info/exclude")).unwrap();
    assert_eq!(
        exclude.lines().filter(|l| l.trim() == ".studio/").count(),
        1
    );
    assert_eq!(git(repo.root(), &["status", "--porcelain"]), "");
}

#[test]
fn evidence_is_never_committed() {
    let (_dir, repo) = init();
    let tree = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::create_dir_all(tree.path.join(".studio-evidence/0")).unwrap();
    fs::write(tree.path.join(".studio-evidence/0/title.png"), "x").unwrap();
    assert!(tree.changed_files().unwrap().is_empty());
    assert_eq!(tree.commit("nothing").unwrap(), None);
    // Asking again, as the studio does for older worktrees, adds no duplicate.
    repo.exclude(".studio-evidence/").unwrap();
    let exclude = fs::read_to_string(repo.root().join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches(".studio-evidence/").count(), 1);
}

#[test]
fn open_rejects_a_plain_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Repo::open(dir.path()).is_err());
}

#[test]
fn current_branch_is_none_when_detached() {
    let (dir, repo) = init();
    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("master"));
    git(dir.path(), &["checkout", "-q", "--detach"]);
    assert_eq!(repo.current_branch().unwrap(), None);
}

/// A goal branch with one landed ticket that adds `file`.
fn landed_goal(repo: &Repo, file: &str) -> String {
    let goal = repo.ensure_goal_branch("Daily mode", "master").unwrap();
    let wt = repo.create_worktree(&ticket("t1"), "master").unwrap();
    fs::write(wt.path.join(file), "work\n").unwrap();
    wt.commit("Ticket work").unwrap();
    assert!(matches!(
        repo.land(&wt, &goal).unwrap(),
        LandOutcome::Merged { .. }
    ));
    goal
}

#[test]
fn a_goal_merges_into_the_checked_out_target() {
    let (_dir, repo) = init();
    let goal = landed_goal(&repo, "level.txt");
    match repo.merge_goal(&goal, "master").unwrap() {
        MergeOutcome::Merged { sha } => assert_eq!(git(repo.root(), &["rev-parse", "HEAD"]), sha),
        other => panic!("expected a merge, got {other:?}"),
    }
    assert!(repo.root().join("level.txt").exists());
}

#[test]
fn a_goal_is_merged_only_once_its_target_holds_it() {
    let (_dir, repo) = init();
    let goal = landed_goal(&repo, "level.txt");
    assert!(!repo.is_merged(&goal, "master"));
    assert!(!repo.is_merged("studio/goal/missing", "master"));
    assert!(!repo.is_merged(&goal, "no-such-branch"));
    repo.merge_goal(&goal, "master").unwrap();
    assert!(repo.is_merged(&goal, "master"));
}

#[test]
fn branch_tips_name_local_branches_and_head() {
    let (_dir, repo) = init();
    let goal = landed_goal(&repo, "level.txt");
    git(repo.root(), &["tag", "v1"]);
    let tips = repo.branch_tips().unwrap();
    assert_eq!(tips[&goal], git(repo.root(), &["rev-parse", &goal]));
    assert_eq!(tips["master"], git(repo.root(), &["rev-parse", "master"]));
    assert_eq!(tips["HEAD"], tips["master"]);
    assert!(!tips.contains_key("v1"));
}

#[test]
fn a_dirty_checkout_is_left_alone() {
    let (_dir, repo) = init();
    let goal = landed_goal(&repo, "level.txt");
    let before = git(repo.root(), &["rev-parse", "HEAD"]);
    fs::write(repo.root().join("README.md"), "owner's edit\n").unwrap();
    assert!(matches!(
        repo.merge_goal(&goal, "master").unwrap(),
        MergeOutcome::Dirty { .. }
    ));
    assert_eq!(git(repo.root(), &["rev-parse", "HEAD"]), before);
    assert_eq!(
        fs::read_to_string(repo.root().join("README.md")).unwrap(),
        "owner's edit\n"
    );
}

#[test]
fn a_target_not_checked_out_merges_in_a_short_lived_worktree() {
    let (_dir, repo) = init();
    git(repo.root(), &["branch", "release"]);
    let goal = landed_goal(&repo, "level.txt");
    let MergeOutcome::Merged { sha } = repo.merge_goal(&goal, "release").unwrap() else {
        panic!("expected a merge");
    };
    assert_eq!(git(repo.root(), &["rev-parse", "release"]), sha);
    assert!(!repo.root().join("level.txt").exists());
    assert!(!git(repo.root(), &["worktree", "list"]).contains("merge-release"));
}

#[test]
fn a_conflicting_goal_is_backed_out() {
    let (_dir, repo) = init();
    let goal = landed_goal(&repo, "README.md");
    let before = commit_on_master(repo.root(), "README.md", "master\n");
    assert_eq!(
        repo.merge_goal(&goal, "master").unwrap(),
        MergeOutcome::Conflict {
            files: vec!["README.md".into()]
        }
    );
    assert_eq!(git(repo.root(), &["rev-parse", "HEAD"]), before);
    assert_eq!(git(repo.root(), &["status", "--porcelain"]), "");
}

#[test]
fn a_long_goal_gets_a_short_branch_that_git_accepts() {
    let (_tmp, repo) = init();
    let goal = "Make the web UI look and feel like a polished product ".repeat(40);
    let branch = Repo::goal_branch_name(&goal);
    assert!(branch.len() <= "studio/goal/".len() + 60, "{branch}");
    assert!(!branch.ends_with('-'));
    assert_eq!(repo.ensure_goal_branch(&goal, "HEAD").unwrap(), branch);
}
