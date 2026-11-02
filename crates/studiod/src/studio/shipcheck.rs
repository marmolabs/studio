//! Checks before shipping: once every task of a goal is done, the studio runs
//! the project's checks on the goal's branch in a fresh checkout and keeps the
//! result until the branch moves. Shipping a goal whose checks failed still
//! works; the web says so first.
//!
//! A result is the file `<data>/ship/<plan ticket>.json`, rewritten at each
//! change. It names the commit the goal's branch was at when the checks began;
//! when the branch is somewhere else the checks run again.
//!
//! This also holds what setups, ship checks and releases share: the fresh
//! checkout they run in and the way a command is tried and summed up.

use super::{MERGE_SOURCE, Studio, file_stem, goal_of, lock, shorten};
use crate::api::{CheckState, CommandRun, ShipCheck};
use crate::{checks, now_ms};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_config::ProjectConfig;
use studio_core::ids::ProjectId;
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_git::Repo;

/// How often the goals with a result are looked at for a branch that moved.
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// The last lines of a command's output that a try keeps.
const TAIL_LINES: usize = 40;

/// The most characters a try keeps of its output, from the end.
const TAIL_CHARS: usize = 4000;

/// Longest summary of a try that is only a line of its output.
const SUMMARY_MAX: usize = 80;

/// What the ship checks keep in memory.
#[derive(Default)]
pub(super) struct State {
    /// The kept results by plan ticket, read from their files at first use.
    /// Held while one is changed and written, so two changes never mix.
    records: Mutex<Option<BTreeMap<String, Record>>>,
    /// The goals whose checks run now, by plan ticket.
    running: Mutex<HashSet<String>>,
    /// When the goals were last looked at for a branch that moved.
    looked: Mutex<Option<Instant>>,
}

/// A goal's kept result and what it was run on.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    project: String,
    /// The goal's branch.
    branch: String,
    /// The commit the branch was at when the checks began.
    commit: String,
    #[serde(flatten)]
    check: ShipCheck,
}

/// A throwaway checkout of one commit, under the project's own `.studio`
/// folder, for checks and releases to run in. It is removed when dropped.
pub(super) struct Checkout {
    repo: Repo,
    dir: PathBuf,
}

impl Checkout {
    /// A fresh checkout of `rev` called `name`; the error says why it could
    /// not be made.
    pub(super) fn new(repo: &Repo, name: &str, rev: &str) -> Result<Checkout, String> {
        let dir = repo
            .create_release_worktree(name, rev)
            .map_err(|e| e.to_string())?;
        Ok(Checkout {
            repo: repo.clone(),
            dir,
        })
    }

    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        if let Err(err) = self.repo.remove_release_worktree(&self.dir) {
            eprintln!("checkout: can't remove {}: {err}", self.dir.display());
        }
    }
}

/// Tries `command` in `dir` the way a release's final checks run it, and says
/// how it went.
pub(super) async fn try_command(
    dir: &Path,
    command: &[String],
    env: &[(String, String)],
) -> CommandRun {
    run_of(checks::tried(dir, command, env).await)
}

/// Tries a capture `command` in `dir` the way a ticket's capture runs it, with
/// `{out}` standing for `out`. A capture that passed says how many files it
/// wrote.
pub(super) async fn try_capture(
    dir: &Path,
    command: &[String],
    out: &Path,
    env: &[(String, String)],
) -> CommandRun {
    let _ = std::fs::create_dir_all(out);
    let mut run = run_of(checks::tried_capture(dir, command, out, env).await);
    let made = count_files(out);
    if run.passed && made > 0 {
        let files = if made == 1 { "file" } else { "files" };
        run.summary = format!("{made} {files}");
    }
    run
}

fn run_of(tried: checks::Tried) -> CommandRun {
    let mut summary = summary_of(&tried.output);
    if summary.is_empty() {
        summary = if tried.passed { "passed" } else { "failed" }.to_string();
    }
    CommandRun {
        passed: tried.passed,
        seconds: tried.seconds,
        summary,
        output_tail: tail_of(&tried.output),
    }
}

/// What an output came to in a few words: how many tests passed (and failed)
/// when it says so, else its last line that is not blank.
fn summary_of(output: &str) -> String {
    if let Some((passed, failed)) = test_counts(output) {
        return match failed {
            Some(failed) if failed > 0 => format!("{passed} passed, {failed} failed"),
            _ => format!("{passed} passed"),
        };
    }
    let last = output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    shorten(last, SUMMARY_MAX)
}

/// The tests that passed, and those that failed if it says, from the lines
/// test runners print, such as `test result: ok. 112 passed; 0 failed` or
/// `3 failed, 112 passed in 4.1s`. Where a run prints one such line for each
/// part of the project, the parts are added up; otherwise the last line
/// counts.
fn test_counts(output: &str) -> Option<(u64, Option<u64>)> {
    let counted: Vec<(&str, u64, Option<u64>)> = output
        .lines()
        .filter_map(|line| {
            let words: Vec<&str> = line
                .split(|c: char| c.is_whitespace() || ",;:()".contains(c))
                .filter(|word| !word.is_empty())
                .collect();
            let after = |names: &[&str]| {
                words.windows(2).find_map(|pair| {
                    let n: u64 = pair[0].parse().ok()?;
                    let word = pair[1].to_ascii_lowercase();
                    names.iter().any(|name| word.starts_with(name)).then_some(n)
                })
            };
            Some((line, after(&["pass"])?, after(&["fail"])))
        })
        .collect();
    let parts: Vec<_> = counted
        .iter()
        .filter(|(line, ..)| line.contains("test result:"))
        .collect();
    if parts.len() > 1 {
        let passed = parts.iter().map(|(_, passed, _)| passed).sum();
        let failed = parts.iter().map(|(.., failed)| failed.unwrap_or(0)).sum();
        return Some((passed, Some(failed)));
    }
    counted.last().map(|(_, passed, failed)| (*passed, *failed))
}

/// The last lines of `output`, at most `TAIL_LINES` and `TAIL_CHARS`.
fn tail_of(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let text = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let start = text.len().saturating_sub(TAIL_CHARS);
    let start = (start..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[start..].to_string()
}

/// How many files `dir` holds, in it and in the folders in it.
fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => count_files(&entry.path()),
            Ok(kind) if kind.is_file() => 1,
            _ => 0,
        })
        .sum()
}

/// The goal that `landed` belongs to, when landing it left every task of the
/// goal done: its plan ticket and its branch. A goal is its plan ticket, the
/// tickets that plan filed and the tickets filed to merge into its branch; it
/// is done when the plan landed, it has tasks, and each task that was not
/// cancelled landed.
fn finished_goal(tickets: &[Ticket], landed: &Ticket) -> Option<(Ticket, String)> {
    let plan = if landed.kind == TicketKind::Plan {
        landed
    } else if let Some(parent) = &landed.parent {
        tickets
            .iter()
            .find(|t| &t.id == parent && t.kind == TicketKind::Plan)?
    } else {
        let merging = landed
            .source
            .as_deref()
            .is_some_and(|s| s.starts_with(MERGE_SOURCE));
        tickets.iter().find(|t| {
            merging
                && t.kind == TicketKind::Plan
                && t.project == landed.project
                && goal_of(t) == goal_of(landed)
        })?
    };
    let goal = goal_of(plan);
    let tasks: Vec<&Ticket> = tickets
        .iter()
        .filter(|t| t.project == plan.project && t.kind == TicketKind::Work)
        .filter(|t| {
            t.parent.as_ref() == Some(&plan.id)
                || (t.parent.is_none()
                    && t.source
                        .as_deref()
                        .is_some_and(|s| s.starts_with(MERGE_SOURCE))
                    && goal_of(t) == goal)
        })
        .filter(|t| t.state != TicketState::Cancelled)
        .collect();
    let done = plan.state == TicketState::Landed
        && !tasks.is_empty()
        && tasks.iter().all(|t| t.state == TicketState::Landed);
    done.then(|| (plan.clone(), Repo::goal_branch_name(&goal)))
}

impl Studio {
    /// The checks of each goal whose tasks are all done, by the goal's plan
    /// ticket, for the office. A goal that has none is left out. Now and then
    /// this also looks for a goal branch that moved since its checks began and
    /// runs them again.
    pub(super) fn ship_checks(self: &Arc<Self>) -> BTreeMap<String, ShipCheck> {
        let kept = self.with_records(|records| records.clone());
        self.look_for_moved_branches(&kept);
        kept.into_iter().map(|(goal, r)| (goal, r.check)).collect()
    }

    /// Called when `ticket` landed: if that finished its goal's tasks, the
    /// goal's checks run on its branch, unless they already ran on the branch
    /// as it is.
    pub(super) fn after_landing(self: &Arc<Self>, ticket: &Ticket) {
        let Ok(tickets) = lock(&self.store).tickets() else {
            return;
        };
        let Some((plan, branch)) = finished_goal(&tickets, ticket) else {
            return;
        };
        self.start_ship_check(&plan.id.to_string(), &plan.project.0, &branch);
    }

    /// Looks at the goals with a result, at most every `LOOK_EVERY`, and runs
    /// the checks again for those whose branch is not where they were run, or
    /// whose checks were cut off by a restart.
    fn look_for_moved_branches(self: &Arc<Self>, kept: &BTreeMap<String, Record>) {
        {
            let mut looked = lock(&self.shipcheck_state.looked);
            if looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
                return;
            }
            *looked = Some(Instant::now());
        }
        let mut projects: HashMap<String, Option<HashMap<String, String>>> = HashMap::new();
        for (goal, record) in kept {
            let running = lock(&self.shipcheck_state.running).contains(goal);
            if running {
                continue;
            }
            let tips = projects.entry(record.project.clone()).or_insert_with(|| {
                let entry = self.project(&ProjectId::new(&record.project)).ok()?;
                Repo::open(&entry.path).ok()?.branch_tips().ok()
            });
            // A branch that is gone, such as one merged and cleared, keeps
            // its result.
            let Some(tip) = tips.as_ref().and_then(|tips| tips.get(&record.branch)) else {
                continue;
            };
            let cut_off = record.check.state == CheckState::Running;
            if *tip != record.commit || cut_off {
                self.start_ship_check(goal, &record.project, &record.branch);
            }
        }
    }

    /// Starts the checks of the goal `goal` on `branch` in the background, unless
    /// they are running or already ran on the branch as it is.
    fn start_ship_check(self: &Arc<Self>, goal: &str, project: &str, branch: &str) {
        let Ok(entry) = self.project(&ProjectId::new(project)) else {
            return;
        };
        let Ok(repo) = Repo::open(&entry.path) else {
            return;
        };
        let Ok(commit) = repo.resolve(branch) else {
            return;
        };
        let known = self.with_records(|records| records.get(goal).cloned());
        if let Some(known) = &known
            && known.commit == commit
            && known.check.state != CheckState::Running
        {
            return;
        }
        if !lock(&self.shipcheck_state.running).insert(goal.to_string()) {
            return;
        }
        let record = Record {
            project: project.to_string(),
            branch: branch.to_string(),
            commit,
            check: ShipCheck {
                state: CheckState::Running,
                summary: "running the checks".to_string(),
                at: now_ms(),
            },
        };
        self.keep_result(goal, &record);
        // Without a runtime to run in (a test of something else) the goal
        // just isn't checked.
        if tokio::runtime::Handle::try_current().is_err() {
            lock(&self.shipcheck_state.running).remove(goal);
            return;
        }
        let studio = Arc::clone(self);
        let goal = goal.to_string();
        tokio::spawn(async move {
            let check = studio.run_ship_check(&goal, &record).await;
            studio.keep_result(&goal, &Record { check, ..record });
            lock(&studio.shipcheck_state.running).remove(&goal);
        });
    }

    /// Runs the project's checks on the commit a goal's branch was at, in a
    /// fresh checkout, and says how they went. They stop at the first that
    /// fails.
    async fn run_ship_check(&self, goal: &str, record: &Record) -> ShipCheck {
        let finish = |state: CheckState, summary: String| ShipCheck {
            state,
            summary,
            at: now_ms(),
        };
        let failed = |summary: String| finish(CheckState::Failed, summary);
        let entry = match self.project(&ProjectId::new(&record.project)) {
            Ok(entry) => entry,
            Err(err) => return failed(err.to_string()),
        };
        let config = match ProjectConfig::load(&entry.path) {
            Ok(config) => config.unwrap_or_default(),
            Err(err) => return failed(format!(".studio.toml doesn't load: {err}")),
        };
        let repo = match Repo::open(&entry.path) {
            Ok(repo) => repo,
            Err(err) => return failed(err.to_string()),
        };
        let name = format!("ship-{}-{}", file_stem(goal), now_ms());
        let checkout = match Checkout::new(&repo, &name, &record.commit) {
            Ok(checkout) => checkout,
            Err(err) => return failed(format!("can't check out the goal: {err}")),
        };
        let env = config.env_for(&entry.path);
        let mut summaries = Vec::new();
        for command in &config.verify {
            let run = try_command(checkout.dir(), command, &env).await;
            if !run.passed {
                let line = shorten(&command.join(" "), 60);
                return failed(format!("{line} failed"));
            }
            summaries.push(run.summary);
        }
        let summary = match summaries.as_slice() {
            [] => "no checks to run".to_string(),
            [only] => only.clone(),
            many => format!("{} checks pass", many.len()),
        };
        finish(CheckState::Passed, summary)
    }

    /// Reads the kept results from their files, at first, and lets `f` look at
    /// them.
    fn with_records<T>(&self, f: impl FnOnce(&mut BTreeMap<String, Record>) -> T) -> T {
        let mut records = lock(&self.shipcheck_state.records);
        let records = records.get_or_insert_with(|| load(&self.ship_dir()));
        f(records)
    }

    /// Keeps `record` as the result for `goal`, in memory and in its file.
    fn keep_result(&self, goal: &str, record: &Record) {
        self.with_records(|records| {
            records.insert(goal.to_string(), record.clone());
            let path = self.ship_dir().join(format!("{}.json", file_stem(goal)));
            if let Err(err) = save(&path, goal, record) {
                eprintln!("ship check of {goal}: {err}");
            }
        });
    }

    fn ship_dir(&self) -> PathBuf {
        self.paths.data_dir.join("ship")
    }
}

/// What a result's file holds besides the result: the plan ticket it is for.
#[derive(Serialize, Deserialize)]
struct Stored {
    goal: String,
    #[serde(flatten)]
    record: Record,
}

/// Every result in `dir`; a file that can't be read is skipped.
fn load(dir: &Path) -> BTreeMap<String, Record> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let stored: Stored = serde_json::from_str(&text).ok()?;
            Some((stored.goal, stored.record))
        })
        .collect()
}

/// Writes a result to `path` (by rename, so a reader never sees half of it).
fn save(path: &Path, goal: &str, record: &Record) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let stored = Stored {
        goal: goal.to_string(),
        record: record.clone(),
    };
    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, serde_json::to_vec_pretty(&stored)?)?;
    std::fs::rename(&staging, path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
pub(super) mod tests {
    use super::*;
    use crate::studio::testing::studio;
    use crate::studio::tickets::NewTicket;
    use std::process::Command;
    use studio_core::event::Event;
    use studio_core::ids::RunId;

    /// Runs git in `dir`, with no config but what the test sets.
    pub(in crate::studio) fn git(dir: &Path, args: &[&str]) -> String {
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

    /// Makes `dir` a git repository on `master` with one commit of `files`.
    pub(in crate::studio) fn repo(dir: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "master"]);
        for (k, v) in [
            ("user.name", "Owner"),
            ("user.email", "owner@example.com"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(dir, &["config", k, v]);
        }
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "Initial"]);
    }

    #[test]
    fn a_runner_that_counts_tests_says_so_and_others_give_their_last_line() {
        let cargo = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored\n\
                     running 9 tests\ntest result: ok. 9 passed; 0 failed; 0 ignored\n\n\
                     real 4.2\n";
        assert_eq!(summary_of(cargo), "12 passed");
        let failing = "test result: FAILED. 3 passed; 2 failed; 0 ignored\n";
        assert_eq!(summary_of(failing), "3 passed, 2 failed");
        assert_eq!(
            summary_of("collected 4 items\n\n112 passed in 1.20s\n"),
            "112 passed"
        );
        assert_eq!(
            summary_of("2 failed, 7 passed in 0.31s\n"),
            "7 passed, 2 failed"
        );
        let jest = "Test Suites: 2 passed, 2 total\nTests:       5 passed, 5 total\n";
        assert_eq!(summary_of(jest), "5 passed");
        assert_eq!(summary_of("compiling\nlint clean\n\n  \n"), "lint clean");
        assert_eq!(summary_of(""), "");
        let long = "x".repeat(200);
        assert!(summary_of(&long).chars().count() <= SUMMARY_MAX);
    }

    #[test]
    fn a_try_keeps_the_last_forty_lines() {
        let output: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let tail = tail_of(&output);
        assert_eq!(tail.lines().count(), 40);
        assert!(tail.starts_with("line 61\n") && tail.ends_with("line 100"));
        assert_eq!(tail_of("short\n"), "short");
        let wide = "é".repeat(5000);
        let cut = tail_of(&wide);
        assert!(cut.len() <= TAIL_CHARS && cut.chars().all(|c| c == 'é'));
    }

    #[tokio::test]
    async fn a_command_is_tried_in_the_checkout_and_summed_up() {
        let dir = tempfile::tempdir().unwrap();
        let sh = |script: &str| ["sh", "-c", script].map(String::from).to_vec();
        let passed = try_command(dir.path(), &sh("echo '112 passed'"), &[]).await;
        assert!(passed.passed);
        assert_eq!(passed.summary, "112 passed");
        assert_eq!(passed.output_tail, "112 passed");
        let failed = try_command(dir.path(), &sh("echo boom >&2; exit 1"), &[]).await;
        assert!(!failed.passed);
        assert_eq!(failed.summary, "boom");
        let out = dir.path().join("shots");
        let shoot = [
            "sh",
            "-c",
            "echo a > \"$0/a.txt\"; echo b > \"$0/b.txt\"",
            "{out}",
        ]
        .map(String::from);
        let run = try_capture(dir.path(), &shoot, &out, &[]).await;
        assert!(run.passed);
        assert_eq!(run.summary, "2 files");
    }

    /// A studio with the project `shop` in a real git repository, whose
    /// `.studio.toml` holds `verify`, and a hire `dev` to give tickets to.
    pub(in crate::studio) fn shop(verify: &str) -> (tempfile::TempDir, Arc<Studio>) {
        let (home, studio) = studio(&["shop"]);
        repo(
            &home.path().join("shop"),
            &[
                ("README.md", "shop\n"),
                (".studio.toml", &format!("verify = [{verify}]\n")),
            ],
        );
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        std::fs::write(
            home.path().join("hires/dev.md"),
            "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nCode.\n",
        )
        .unwrap();
        studio.reload_config();
        (home, studio)
    }

    /// A ticket of the project `shop`, taken through `events`.
    fn ticket(
        studio: &Arc<Studio>,
        kind: TicketKind,
        goal: &str,
        parent: Option<&Ticket>,
        events: &[Event],
    ) -> Ticket {
        let created = studio
            .create_ticket(NewTicket {
                project: "shop".into(),
                title: format!("{kind:?}"),
                goal: goal.into(),
                hire: "dev".into(),
                base: Some("master".into()),
                kind,
                parent: parent.map(|p| p.id.clone()),
                detail: String::new(),
                source: None,
                version: None,
            })
            .unwrap();
        advance(studio, created, events)
    }

    fn advance(studio: &Arc<Studio>, mut ticket: Ticket, events: &[Event]) -> Ticket {
        for event in events {
            ticket = studio.append(&ticket.id, event.clone()).unwrap();
        }
        ticket
    }

    fn plan_lands() -> Vec<Event> {
        vec![
            Event::Claimed,
            Event::RunStarted {
                run: RunId::new("r"),
            },
            Event::Landed,
        ]
    }

    fn work_lands() -> Vec<Event> {
        let mut events = plan_lands();
        events.splice(
            2..2,
            [
                Event::ChecksStarted,
                Event::ChecksPassed,
                Event::ReviewPassed,
            ],
        );
        events
    }

    fn tickets(studio: &Arc<Studio>) -> Vec<Ticket> {
        lock(&studio.store).tickets().unwrap()
    }

    fn named<'a>(all: &'a [Ticket], ticket: &Ticket) -> &'a Ticket {
        all.iter().find(|t| t.id == ticket.id).unwrap()
    }

    #[test]
    fn a_goal_is_finished_when_its_plan_and_every_task_landed() {
        let (_home, studio) = shop("");
        let plan = ticket(&studio, TicketKind::Plan, "Add a cart", None, &plan_lands());
        let first = ticket(
            &studio,
            TicketKind::Work,
            "Add a cart",
            Some(&plan),
            &work_lands(),
        );
        let second = ticket(&studio, TicketKind::Work, "Add a cart", Some(&plan), &[]);

        // One task still waits.
        assert!(finished_goal(&tickets(&studio), &first).is_none());
        // Once it lands the goal is finished, whichever ticket says so.
        let second = advance(&studio, second, &work_lands());
        let all = tickets(&studio);
        let (found, branch) = finished_goal(&all, named(&all, &second)).unwrap();
        assert_eq!(found.id, plan.id);
        assert_eq!(branch, "studio/goal/add-a-cart");
        assert!(finished_goal(&all, named(&all, &plan)).is_some());

        // A cancelled task does not hold the goal back.
        ticket(
            &studio,
            TicketKind::Work,
            "Add a cart",
            Some(&plan),
            &[Event::Cancelled],
        );
        let all = tickets(&studio);
        assert!(finished_goal(&all, named(&all, &second)).is_some());
        // One that is not done does.
        ticket(
            &studio,
            TicketKind::Work,
            "Add a cart",
            Some(&plan),
            &[Event::Claimed],
        );
        let all = tickets(&studio);
        assert!(finished_goal(&all, named(&all, &second)).is_none());
    }

    #[test]
    fn a_ticket_with_no_plan_is_no_goal_and_a_plan_with_no_tasks_is_not_done() {
        let (_home, studio) = shop("");
        let alone = ticket(&studio, TicketKind::Work, "", None, &work_lands());
        assert!(finished_goal(&tickets(&studio), &alone).is_none());
        let plan = ticket(&studio, TicketKind::Plan, "Empty", None, &plan_lands());
        assert!(finished_goal(&tickets(&studio), &plan).is_none());
    }

    /// Waits until the goal's check has left `Running`.
    async fn finished_check(studio: &Arc<Studio>, goal: &str) -> ShipCheck {
        for _ in 0..600 {
            if let Some(check) = studio.ship_checks().get(goal)
                && check.state != CheckState::Running
            {
                return check.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the check of {goal} never finished");
    }

    /// Puts `cart.txt` on the branch of the goal, as landing a task does.
    fn land_cart_on(repo_dir: &Path, branch: &str) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let at = work.to_str().unwrap();
        git(repo_dir, &["worktree", "add", "-q", at, branch]);
        std::fs::write(work.join("cart.txt"), "cart\n").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "Cart"]);
        git(repo_dir, &["worktree", "remove", "--force", at]);
    }

    /// The checkouts the checks left in the project.
    fn checkouts(repo_dir: &Path) -> usize {
        std::fs::read_dir(repo_dir.join(".studio/releases")).map_or(0, |dir| dir.count())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_last_task_landing_runs_the_checks_and_keeps_the_result() {
        let (home, studio) = shop(r#"["sh", "-c", "test -f cart.txt && echo '4 passed'"]"#);
        let repo_dir = home.path().join("shop");
        let plan = ticket(&studio, TicketKind::Plan, "Add a cart", None, &plan_lands());
        let task = ticket(
            &studio,
            TicketKind::Work,
            "Add a cart",
            Some(&plan),
            &work_lands(),
        );
        let goal = plan.id.to_string();
        let branch = Repo::goal_branch_name("Add a cart");
        git(&repo_dir, &["branch", &branch, "master"]);
        land_cart_on(&repo_dir, &branch);
        assert!(studio.ship_checks().is_empty());

        studio.after_landing(&task);
        assert_eq!(studio.ship_checks()[&goal].state, CheckState::Running);
        let check = finished_check(&studio, &goal).await;
        assert_eq!(check.state, CheckState::Passed);
        assert_eq!(check.summary, "4 passed");
        assert!(check.at > 0);
        assert_eq!(checkouts(&repo_dir), 0);

        // The result is a file named for the plan ticket.
        let file = studio
            .paths
            .data_dir
            .join("ship")
            .join(format!("{goal}.json"));
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(stored["state"], "passed");
        assert_eq!(stored["project"], "shop");
        assert_eq!(stored["branch"], branch);
        assert_eq!(stored["goal"], goal);
        assert_eq!(load(&studio.paths.data_dir.join("ship")).len(), 1);

        // Landing again with the branch where it was does not run it again.
        studio.after_landing(&task);
        assert_eq!(studio.ship_checks()[&goal].at, check.at);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn landing_the_last_task_for_real_starts_the_checks() {
        let (home, studio) = shop(r#"["sh", "-c", "test -f cart.txt && echo '1 passed'"]"#);
        let repo_dir = home.path().join("shop");
        let plan = ticket(&studio, TicketKind::Plan, "Add a cart", None, &plan_lands());
        let ready = &work_lands()[..5];
        let first = ticket(&studio, TicketKind::Work, "Add a cart", Some(&plan), ready);
        let second = ticket(&studio, TicketKind::Work, "Add a cart", Some(&plan), ready);
        let repo = Repo::open(&repo_dir).unwrap();
        for (n, task) in [&first, &second].into_iter().enumerate() {
            let worktree = repo.create_worktree(&task.id, "master").unwrap();
            std::fs::write(worktree.path.join(format!("cart{n}.txt")), "cart\n").unwrap();
            if n == 0 {
                std::fs::write(worktree.path.join("cart.txt"), "cart\n").unwrap();
            }
            worktree.commit("Cart").unwrap();
        }

        // The first landing leaves a task, so nothing is checked.
        studio.land(&first.id).unwrap();
        assert!(studio.ship_checks().is_empty());
        // The second finishes the goal.
        studio.land(&second.id).unwrap();
        let check = finished_check(&studio, &plan.id.to_string()).await;
        assert_eq!(check.state, CheckState::Passed);
        assert_eq!(check.summary, "1 passed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_check_names_itself_and_a_moved_branch_is_checked_again() {
        let (home, studio) = shop(r#"["sh", "-c", "test -f cart.txt"]"#);
        let repo_dir = home.path().join("shop");
        let plan = ticket(&studio, TicketKind::Plan, "Add a cart", None, &plan_lands());
        let task = ticket(
            &studio,
            TicketKind::Work,
            "Add a cart",
            Some(&plan),
            &work_lands(),
        );
        let goal = plan.id.to_string();
        let branch = Repo::goal_branch_name("Add a cart");
        git(&repo_dir, &["branch", &branch, "master"]);

        studio.after_landing(&task);
        let check = finished_check(&studio, &goal).await;
        assert_eq!(check.state, CheckState::Failed);
        assert_eq!(check.summary, "sh -c test -f cart.txt failed");
        assert_eq!(checkouts(&repo_dir), 0);

        // The branch moves; the office notices at its next look at the goals.
        land_cart_on(&repo_dir, &branch);
        *lock(&studio.shipcheck_state.looked) = None;
        studio.ship_checks();
        let check = finished_check(&studio, &goal).await;
        assert_eq!(check.state, CheckState::Passed);
        assert_eq!(check.summary, "passed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_result_cut_off_by_a_restart_runs_again() {
        let (home, studio) = shop(r#"["true"]"#);
        let repo_dir = home.path().join("shop");
        let branch = Repo::goal_branch_name("Add a cart");
        git(&repo_dir, &["branch", &branch, "master"]);
        let record = Record {
            project: "shop".into(),
            commit: git(&repo_dir, &["rev-parse", &branch]),
            branch,
            check: ShipCheck {
                state: CheckState::Running,
                summary: "running the checks".into(),
                at: 5,
            },
        };
        let dir = studio.paths.data_dir.join("ship");
        save(&dir.join("SHOP-1.json"), "SHOP-1", &record).unwrap();
        // The studio reads what an earlier one left, and finishes it.
        assert_eq!(studio.ship_checks()["SHOP-1"].state, CheckState::Running);
        let check = finished_check(&studio, "SHOP-1").await;
        assert_eq!(check.state, CheckState::Passed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_goal_with_no_branch_has_no_result() {
        let (_home, studio) = shop("");
        studio.start_ship_check("SHOP-1", "shop", "studio/goal/none");
        assert!(studio.ship_checks().is_empty());
    }
}
