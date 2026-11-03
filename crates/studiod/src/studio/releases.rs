//! Releasing a project: the final checks on the tip of its integration
//! branch, the owner's go-ahead, the project's release commands, and the tag.
//!
//! A release is a record in `<data>/releases/<project>.json`, rewritten (by
//! rename) at every step, so it outlives a restart. The task that drives a
//! release lives only in memory: a release still under way when studiod
//! starts again is failed.

use super::shipcheck::Checkout;
use super::{Studio, StudioError, lock, other, shorten};
use crate::api::{ReleaseState, ReleaseStep, ReleaseView, Response};
use crate::budget::DAY_MS;
use crate::{checks, now_ms};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_config::ProjectConfig;
use studio_config::config::ProjectEntry;
use studio_core::ids::ProjectId;
use studio_git::Repo;
use tokio::sync::watch;

const STOPPED: &str = "studiod stopped";
/// Longest name a release command gets among the steps.
const STEP_NAME_MAX: usize = 40;

/// The releases under way, by project.
#[derive(Default)]
pub(super) struct Releasing {
    /// Held while a release record is read and rewritten, so two changes
    /// never mix.
    active: Mutex<BTreeMap<String, Active>>,
}

struct Active {
    /// The release's `started_at`, which names it among a project's releases.
    id: u64,
    signal: watch::Sender<Signal>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Signal {
    Wait,
    Approved,
    Cancelled,
}

/// Everything a release needs, fixed when it starts.
struct Job {
    project: String,
    id: u64,
    version: String,
    tag: String,
    commit: String,
    repo: PathBuf,
    verify: Vec<Vec<String>>,
    release: Vec<Vec<String>>,
    env: Vec<(String, String)>,
}

enum Course {
    Published,
    /// Cancelled; the record already says so.
    Stopped,
}

impl Studio {
    /// `Request::StartRelease`: refuses what can't be released, otherwise
    /// starts the final checks in the background and answers the release.
    pub(super) fn start_release(
        self: &Arc<Self>,
        project: &str,
        version: &str,
    ) -> Result<Response, StudioError> {
        let entry = self.project(&ProjectId::new(project))?;
        let config = ProjectConfig::load(&entry.path)
            .map_err(|e| other(e.to_string()))?
            .unwrap_or_default();
        if config.release.is_empty() {
            return Err(other(format!(
                "{project} has no release commands. Add them to {}, such as \
                 release = [[\"./publish.sh\"]]; each runs with VERSION set",
                entry.path.join(".studio.toml").display()
            )));
        }
        let mut active = lock(&self.releasing.active);
        if let Some(under_way) = active.get(project) {
            let version = read(&self.paths.data_dir, project)
                .into_iter()
                .find(|r| r.started_at == under_way.id)
                .map(|r| r.version)
                .unwrap_or_default();
            return Err(other(format!(
                "{project} already has release {version} under way; cancel it first"
            )));
        }
        check_version(version)?;
        let tag = tag_name(version);
        let repo = Repo::open(&entry.path)?;
        if !repo.is_tag_name(&tag)? {
            return Err(other(format!("{tag} is not a name git accepts for a tag")));
        }
        if repo.tag_exists(&tag)? {
            return Err(other(format!(
                "{project} already has a tag {tag}; pick a version it hasn't used"
            )));
        }
        let branch = entry.integration_branch.as_deref().unwrap_or("HEAD");
        let commit = repo.resolve(branch)?;

        let mut history = read(&self.paths.data_dir, project);
        let id = history
            .iter()
            .map(|r| r.started_at + 1)
            .max()
            .unwrap_or(0)
            .max(now_ms());
        let mut steps = vec![step("Final checks"), step("Your go-ahead")];
        steps.extend(
            config
                .release
                .iter()
                .map(|c| step(&shorten(&c.join(" "), STEP_NAME_MAX))),
        );
        steps.push(step(&format!("Tag {tag}")));
        let view = ReleaseView {
            project: project.to_string(),
            version: version.to_string(),
            state: ReleaseState::Checking,
            steps,
            started_at: id,
            finished_at: None,
            error: None,
        };
        history.insert(0, view.clone());
        write(&self.paths.data_dir, project, &history)
            .map_err(|e| other(format!("can't save the release: {e}")))?;
        let (signal, rx) = watch::channel(Signal::Wait);
        active.insert(project.to_string(), Active { id, signal });
        drop(active);

        let mut env = config.env_for(&entry.path);
        env.push(("VERSION".to_string(), version.to_string()));
        let job = Job {
            project: project.to_string(),
            id,
            version: version.to_string(),
            tag,
            commit,
            repo: entry.path.clone(),
            verify: config.verify.clone(),
            release: config.release.clone(),
            env,
        };
        let studio = Arc::clone(self);
        tokio::spawn(async move { studio.run_release(job, rx).await });
        Ok(Response::ok(view))
    }

    /// `Request::ApproveRelease`: the owner's go-ahead for a release that
    /// waits for it.
    pub(super) fn approve_release(&self, project: &str) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let active = lock(&self.releasing.active);
        let Some(under_way) = active.get(project) else {
            return Err(other(format!("{project} has no release waiting for you")));
        };
        let dir = &self.paths.data_dir;
        let current = find(dir, project, under_way.id)?;
        match current.state {
            ReleaseState::Waiting => {}
            ReleaseState::Checking => {
                return Err(other(format!(
                    "the final checks of {} are still running; approve it once it waits for you",
                    current.version
                )));
            }
            _ => {
                return Err(other(format!(
                    "{} is already being published",
                    current.version
                )));
            }
        }
        let view = edit(dir, project, under_way.id, |v| {
            v.state = ReleaseState::Publishing;
            mark(v, 1);
        })?;
        under_way.signal.send_replace(Signal::Approved);
        Ok(Response::ok(view))
    }

    /// `Request::CancelRelease`: stops a release that is checking or waiting.
    pub(super) fn cancel_release(&self, project: &str) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let mut active = lock(&self.releasing.active);
        let Some(under_way) = active.get(project) else {
            return Err(other(format!("{project} has no release under way")));
        };
        let dir = &self.paths.data_dir;
        let current = find(dir, project, under_way.id)?;
        if current.state == ReleaseState::Publishing {
            return Err(other(format!(
                "{} is already being published and can't be stopped",
                current.version
            )));
        }
        let view = edit(dir, project, under_way.id, |v| {
            v.state = ReleaseState::Cancelled;
            v.finished_at = Some(now_ms());
        })?;
        under_way.signal.send_replace(Signal::Cancelled);
        active.remove(project);
        Ok(Response::ok(view))
    }

    /// `Request::Releases`: every release of `project`, or of all projects,
    /// newest first.
    pub(super) fn list_releases(&self, project: Option<&str>) -> Result<Response, StudioError> {
        let names: Vec<String> = match project {
            Some(name) => {
                self.project(&ProjectId::new(name))?;
                vec![name.to_string()]
            }
            None => self
                .config()
                .projects
                .iter()
                .map(|p| p.name.clone())
                .collect(),
        };
        let mut all: Vec<ReleaseView> = names
            .iter()
            .flat_map(|name| read(&self.paths.data_dir, name))
            .collect();
        all.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Ok(Response::ok(all))
    }

    /// What the Office shows of a project's releases: whether it has release
    /// commands, the release under way (or one that failed or was cancelled
    /// in the last day), and the latest that published.
    pub(super) fn release_fields(
        &self,
        entry: &ProjectEntry,
    ) -> (bool, Option<ReleaseView>, Option<ReleaseView>) {
        let releasable = ProjectConfig::load(&entry.path)
            .ok()
            .flatten()
            .is_some_and(|config| !config.release.is_empty());
        let mut history = read(&self.paths.data_dir, &entry.name);
        history.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        let now = now_ms();
        let release = history
            .first()
            .filter(|r| match r.state {
                ReleaseState::Released => false,
                ReleaseState::Failed | ReleaseState::Cancelled => r
                    .finished_at
                    .is_some_and(|at| now.saturating_sub(at) < DAY_MS),
                _ => true,
            })
            .cloned();
        let last = history
            .into_iter()
            .find(|r| r.state == ReleaseState::Released);
        (releasable, release, last)
    }

    /// Drives one release to its end, in the background.
    async fn run_release(self: Arc<Self>, job: Job, mut rx: watch::Receiver<Signal>) {
        let repo = match Repo::open(&job.repo) {
            Ok(repo) => repo,
            Err(err) => return self.finish_failed(&job, err.to_string()),
        };
        let checkout = match Checkout::new(&repo, &job.id.to_string(), &job.commit) {
            Ok(checkout) => checkout,
            Err(err) => {
                return self.finish_failed(&job, format!("can't check out the release: {err}"));
            }
        };
        let result = self.drive(&job, &mut rx, &repo, checkout.dir()).await;
        // The checkout goes before the release is marked as ended.
        drop(checkout);
        match result {
            Ok(Course::Published) => self.finish(&job, |v| {
                v.state = ReleaseState::Released;
                v.finished_at = Some(now_ms());
            }),
            Ok(Course::Stopped) => {}
            Err(why) => self.finish_failed(&job, why),
        }
    }

    /// The release's steps in order; an error is why it failed.
    async fn drive(
        &self,
        job: &Job,
        rx: &mut watch::Receiver<Signal>,
        repo: &Repo,
        dir: &Path,
    ) -> Result<Course, String> {
        for command in &job.verify {
            let run = checks::run(dir, command, &job.env);
            tokio::select! {
                result = run => result?,
                () = cancelled(rx) => return Ok(Course::Stopped),
            }
        }
        let waiting = self.change(job, |v| {
            mark(v, 0);
            v.state = ReleaseState::Waiting;
        });
        if !waiting {
            return Ok(Course::Stopped);
        }
        self.notifier.tell(format!(
            "{} {} passed its final checks and waits for your go-ahead",
            job.project, job.version
        ));
        loop {
            match *rx.borrow_and_update() {
                Signal::Approved => break,
                Signal::Cancelled => return Ok(Course::Stopped),
                Signal::Wait => {}
            }
            if rx.changed().await.is_err() {
                return Ok(Course::Stopped);
            }
        }
        for (n, command) in job.release.iter().enumerate() {
            checks::run(dir, command, &job.env).await?;
            self.change(job, |v| mark(v, 2 + n));
        }
        repo.tag(&job.tag, &job.commit, &format!("Release {}", job.version))
            .map_err(|e| format!("can't tag {}: {e}", job.tag))?;
        self.change(job, |v| mark(v, 2 + job.release.len()));
        Ok(Course::Published)
    }

    /// Edits a release that is still under way; false when it is not (it
    /// was cancelled).
    fn change(&self, job: &Job, f: impl FnOnce(&mut ReleaseView)) -> bool {
        let active = lock(&self.releasing.active);
        if active.get(&job.project).is_none_or(|a| a.id != job.id) {
            return false;
        }
        match edit(&self.paths.data_dir, &job.project, job.id, f) {
            Ok(_) => true,
            Err(err) => {
                eprintln!("release {}: {err}", job.project);
                true
            }
        }
    }

    /// Ends a release that is still under way.
    fn finish(&self, job: &Job, f: impl FnOnce(&mut ReleaseView)) {
        let mut active = lock(&self.releasing.active);
        if active.get(&job.project).is_none_or(|a| a.id != job.id) {
            return;
        }
        if let Err(err) = edit(&self.paths.data_dir, &job.project, job.id, f) {
            eprintln!("release {}: {err}", job.project);
        }
        active.remove(&job.project);
    }

    fn finish_failed(&self, job: &Job, why: String) {
        self.finish(job, |v| {
            v.state = ReleaseState::Failed;
            v.finished_at = Some(now_ms());
            v.error = Some(why);
        });
    }
}

/// Fails every release that was under way when studiod stopped, and clears
/// the checkouts it left.
pub(super) fn recover(data_dir: &Path, projects: &[ProjectEntry]) {
    for project in projects {
        let mut history = read(data_dir, &project.name);
        let mut changed = false;
        for release in history.iter_mut().filter(|r| !ended(r.state)) {
            release.state = ReleaseState::Failed;
            release.finished_at = Some(now_ms());
            release.error = Some(STOPPED.to_string());
            changed = true;
            if let Ok(repo) = Repo::open(&project.path) {
                let dir = repo.release_worktree_path(&release.started_at.to_string());
                let _ = repo.remove_release_worktree(&dir);
            }
        }
        if changed && let Err(err) = write(data_dir, &project.name, &history) {
            eprintln!("releases of {}: {err}", project.name);
        }
    }
}

/// Resolves once the owner has cancelled the release.
async fn cancelled(rx: &mut watch::Receiver<Signal>) {
    loop {
        if *rx.borrow_and_update() == Signal::Cancelled {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

fn ended(state: ReleaseState) -> bool {
    matches!(
        state,
        ReleaseState::Released | ReleaseState::Failed | ReleaseState::Cancelled
    )
}

fn step(name: &str) -> ReleaseStep {
    ReleaseStep {
        name: name.to_string(),
        done: false,
        at: None,
    }
}

/// Marks step `n` done now.
fn mark(release: &mut ReleaseView, n: usize) {
    if let Some(step) = release.steps.get_mut(n) {
        step.done = true;
        step.at = Some(now_ms());
    }
}

/// Letters, digits, dots and dashes only, starting with a letter or digit.
fn check_version(version: &str) -> Result<(), StudioError> {
    if version.is_empty() {
        return Err(other("give a version to release, such as 0.4"));
    }
    let plain = version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !plain || !version.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return Err(other(format!(
            "'{version}' is not a version the studio can tag: use letters, digits, \
             dots and dashes only, such as 0.4 or 1.2-beta"
        )));
    }
    Ok(())
}

/// The tag for a version: `v` in front, unless it starts with one.
fn tag_name(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn file_of(data_dir: &Path, project: &str) -> PathBuf {
    let name: String = project
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    data_dir.join("releases").join(format!("{name}.json"))
}

/// A project's releases as saved, newest first; none if there is no file.
fn read(data_dir: &Path, project: &str) -> Vec<ReleaseView> {
    let path = file_of(data_dir, project);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            eprintln!("{}: {err}; its releases are not shown", path.display());
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

fn write(data_dir: &Path, project: &str, releases: &[ReleaseView]) -> std::io::Result<()> {
    let path = file_of(data_dir, project);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, serde_json::to_vec_pretty(releases)?)?;
    std::fs::rename(&staging, &path)
}

fn find(data_dir: &Path, project: &str, id: u64) -> Result<ReleaseView, StudioError> {
    read(data_dir, project)
        .into_iter()
        .find(|r| r.started_at == id)
        .ok_or_else(|| other(format!("the release of {project} is not on record")))
}

/// Changes one saved release and returns it as changed.
fn edit(
    data_dir: &Path,
    project: &str,
    id: u64,
    f: impl FnOnce(&mut ReleaseView),
) -> Result<ReleaseView, StudioError> {
    let mut history = read(data_dir, project);
    let release = history
        .iter_mut()
        .find(|r| r.started_at == id)
        .ok_or_else(|| other(format!("the release of {project} is not on record")))?;
    f(release);
    let changed = release.clone();
    write(data_dir, project, &history)
        .map_err(|e| other(format!("can't save the release of {project}: {e}")))?;
    Ok(changed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_plain() {
        for good in ["0.4", "1", "v1.2.3", "2.0-beta1", "r7"] {
            assert!(check_version(good).is_ok(), "{good}");
        }
        for bad in ["", " 0.4", "1 2", "-1", ".1", "1/2", "1_2", "é1", "1.0\n"] {
            assert!(check_version(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_version_keeps_its_v() {
        assert_eq!(tag_name("0.4"), "v0.4");
        assert_eq!(tag_name("v0.4"), "v0.4");
    }

    #[test]
    fn releases_survive_in_a_file_per_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path(), "game").is_empty());
        let release = ReleaseView {
            project: "game".into(),
            version: "0.4".into(),
            state: ReleaseState::Waiting,
            steps: vec![step("Final checks")],
            started_at: 5,
            finished_at: None,
            error: None,
        };
        write(dir.path(), "game", std::slice::from_ref(&release)).unwrap();
        assert_eq!(read(dir.path(), "game"), vec![release]);
        assert!(read(dir.path(), "other").is_empty());
        let changed = edit(dir.path(), "game", 5, |v| mark(v, 0)).unwrap();
        assert!(changed.steps[0].done && changed.steps[0].at.is_some());
        assert!(edit(dir.path(), "game", 6, |_| {}).is_err());
    }
}
