//! Setting a project up through its lead: the lead reads the repository and
//! proposes how the project runs, the studio tries what it proposes, and the
//! owner picks what to keep.
//!
//! A setup is the file `<data>/setup/<project>.json`, a `SetupView`, which
//! goes from reading to checking to ready (or failed) and ends accepted. The
//! lead's run is `leadrun::run` with the studio's tools in setup mode, whose
//! one tool, `propose_setup`, comes here as `Request::ProposeSetup`.

use super::briefing::reply_rules;
use super::settings::repo_and_branch;
use super::shipcheck::{Checkout, try_capture, try_command};
use super::{Studio, StudioError, file_stem, lock, other};
use crate::api::{
    FileKind, NewLead, Request, Response, SetupChoice, SetupProposal, SetupState, SetupView,
    TalkCard, TalkFrom,
};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_config::config::ProjectEntry;
use studio_config::{Checks, NewProject, ProjectConfig};
use studio_core::ids::{HireId, ProjectId};
use studio_core::rule::Hire;
use studio_git::Repo;

/// What a setup run may spend, in dollars.
const SETUP_USD: f64 = 0.60;

/// How many turns a setup run may take.
const SETUP_TURNS: u32 = 20;

/// The most checks, release commands or protected paths a proposal may hold.
const MOST: usize = 20;

/// What the setups keep in memory.
#[derive(Default)]
pub(super) struct State {
    /// Each project's setup as last kept, or none if it has no file. Held
    /// while a setup is read, changed and written, so two changes never mix.
    views: Mutex<BTreeMap<String, Option<SetupView>>>,
    /// The projects whose setup run is under way.
    running: Mutex<HashSet<String>>,
}

type Views = BTreeMap<String, Option<SetupView>>;

impl Studio {
    /// `Request::SetupProject`: adds the project at `path`, as `AddProject`
    /// does but without guessing its checks, makes its lead the hire `lead` or
    /// a new hire made from `new_lead`, posts the lead's greeting to the
    /// project's conversation and starts the setup run. A project already in
    /// `studio.toml` whose setup failed, or never began, is set up again.
    /// Answers the `SetupView`.
    pub(super) fn setup_project(
        self: &Arc<Self>,
        path: &str,
        name: Option<String>,
        lead: Option<String>,
        new_lead: Option<NewLead>,
    ) -> Result<Response, StudioError> {
        let lead = lead.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        let made = match (&lead, &new_lead) {
            (Some(_), Some(_)) => {
                return Err(other(
                    "pick a lead that is already hired, or hire a new one, not both",
                ));
            }
            (None, None) => return Err(other("give the project a lead: pick one or hire one")),
            (Some(lead), None) => {
                self.hire(&HireId::new(lead))?;
                None
            }
            (None, Some(new)) => Some(self.new_lead(new)?),
        };

        let (repo, branch) = repo_and_branch(path, None)?;
        let root = repo
            .root()
            .canonicalize()
            .unwrap_or(repo.root().to_path_buf());
        let existing = self
            .config()
            .projects
            .iter()
            .find(|p| p.path.canonicalize().ok().as_deref() == Some(root.as_path()))
            .cloned();
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let project = NewProject {
            path: &root,
            name: name.as_deref(),
            integration_branch: Some(&branch),
            daily_usd: None,
            pack: None,
        };
        match &existing {
            Some(entry) => self.can_set_up_again(&entry.name)?,
            None => {
                studio_config::check_project(&self.paths, &project)
                    .map_err(|e| other(e.to_string()))?;
            }
        }

        // Everything that could be refused has been; now make things.
        let hire_id = match (&lead, &made) {
            (Some(lead), _) => lead.clone(),
            (None, Some((id, text))) => {
                self.save_file(FileKind::Hires, id, text)?;
                id.clone()
            }
            (None, None) => return Err(other("give the project a lead: pick one or hire one")),
        };
        let entry = match existing {
            Some(entry) => entry,
            None => match self.add_bare_project(&project) {
                Ok(entry) => entry,
                Err(err) => {
                    if made.is_some() {
                        let _ = self.delete_file(FileKind::Hires, &hire_id);
                    }
                    return Err(err);
                }
            },
        };
        self.set_project_lead(&entry.name, Some(hire_id.clone()))?;
        let hire = self.hire(&HireId::new(&hire_id))?;

        let view = SetupView {
            project: entry.name.clone(),
            lead: Some(hire_id),
            state: SetupState::Reading,
            proposal: None,
            error: None,
        };
        self.reset_setup(&view);
        let studio = Arc::clone(self);
        let (project, repo_path) = (entry.name.clone(), entry.path.clone());
        tokio::spawn(async move { studio.run_setup(project, hire, repo_path).await });
        Ok(Response::ok(view))
    }

    /// `Request::SetupStatus`: how the setup of `project` stands. Answers the
    /// `SetupView`.
    pub(super) fn setup_status(&self, project: &str) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let view = self
            .setup_view(project)
            .ok_or_else(|| other(format!("{project} wasn't set up through its lead")))?;
        Ok(Response::ok(view))
    }

    /// `Request::AcceptSetup`: writes what the owner chose to the project's
    /// `.studio.toml` (keeping what else is in it) and the daily limit to
    /// `studio.toml`, marks the setup accepted, and with `plan_first` starts the
    /// first goal with the lead. Answers the `SetupView`.
    pub(super) fn accept_setup(
        self: &Arc<Self>,
        project: &str,
        choice: SetupChoice,
    ) -> Result<Response, StudioError> {
        let entry = self.project(&ProjectId::new(project))?;
        let view = self
            .setup_view(project)
            .ok_or_else(|| other(format!("{project} wasn't set up through its lead")))?;
        match view.state {
            SetupState::Ready => {}
            SetupState::Accepted => {
                return Err(other(format!("{project} is already set up")));
            }
            SetupState::Failed => {
                return Err(other(format!(
                    "setting up {project} failed: {}",
                    view.error.unwrap_or_default()
                )));
            }
            SetupState::Reading | SetupState::Checking => {
                return Err(other(format!(
                    "the lead is still setting up {project}; wait until it is ready"
                )));
            }
        }
        // Whatever could be refused is, before anything is written.
        let first = match (choice.plan_first, view.proposal.and_then(|p| p.first_goal)) {
            (true, None) => {
                return Err(other(format!(
                    "the lead has no first goal to plan for {project}"
                )));
            }
            (true, Some(goal)) => Some(goal),
            (false, _) => None,
        };
        if let Some(usd) = choice.daily_usd
            && (!usd.is_finite() || usd <= 0.0)
        {
            return Err(other(format!(
                "the daily limit must be more than zero, not {usd}"
            )));
        }
        let checks = Checks {
            verify: choice.verify,
            capture: choice.capture,
            release: choice.release,
            protected: choice.protected,
        };
        let current = std::fs::read_to_string(entry.path.join(".studio.toml")).ok();
        studio_config::with_checks(current.as_deref(), &checks)
            .map_err(|e| other(e.to_string()))?;

        studio_config::write_checks(&entry.path, &checks).map_err(|e| other(e.to_string()))?;
        if let Some(usd) = choice.daily_usd {
            let _writing = lock(&self.settings);
            studio_config::set_project_daily_usd(&self.paths, project, Some(usd))
                .map_err(|e| other(e.to_string()))?;
            self.reload_config();
        }
        let accepted = self
            .change_setup(project, |view| view.state = SetupState::Accepted)
            .ok_or_else(|| other(format!("{project} wasn't set up through its lead")))?;

        if let Some(goal) = first {
            let lead = accepted.lead.clone().or(entry.lead);
            let started = self.dispatch(Request::CreateGoal {
                project: project.to_string(),
                text: goal.text.clone(),
                hire: lead.clone(),
                detail: goal.why.clone(),
                version: goal.version.clone(),
            })?;
            if !started.ok {
                return Err(other(format!(
                    "{project} is set up, but its first goal didn't start: {}",
                    started.error.unwrap_or_default()
                )));
            }
            if let Some(lead) = lead {
                let text = format!(
                    "I'm planning \"{}\". The tasks show up here once they're filed.",
                    goal.text
                );
                self.announce(project, TalkFrom::Lead { hire: lead }, &text, None);
            }
        }
        Ok(Response::ok(accepted))
    }

    /// `Request::ProposeSetup`: stores the lead's proposal for `project` when
    /// it is sound. The setup run goes on to try it once the lead is done.
    pub(super) fn propose_setup(
        &self,
        project: &str,
        proposal: SetupProposal,
    ) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let proposal = sound(proposal)?;
        let mut views = lock(&self.setup_state.views);
        self.load_into(&mut views, project);
        let view = views
            .get_mut(project)
            .and_then(Option::as_mut)
            .filter(|view| view.state == SetupState::Reading)
            .ok_or_else(|| other(format!("{project} isn't being set up right now")))?;
        if view.proposal.is_some() {
            return Err(other(format!(
                "you have proposed a setup already. Finish with a reply to the owner {}",
                found_rules()
            )));
        }
        view.proposal = Some(proposal);
        self.write_setup(view);
        Ok(Response::ok(format!(
            "Thanks, the studio will try these in a fresh checkout now. Finish with a reply to \
             the owner {}",
            found_rules()
        )))
    }

    /// How each project's setup stands, by project name, for the office. A
    /// project that was never set up through its lead is left out.
    pub(super) fn setups(&self) -> BTreeMap<String, SetupView> {
        self.config()
            .projects
            .iter()
            .filter_map(|p| Some((p.name.clone(), self.setup_view(&p.name)?)))
            .collect()
    }

    /// Whether the setup of `project` may be started again: not while it runs,
    /// waits for the owner or is done.
    fn can_set_up_again(&self, project: &str) -> Result<(), StudioError> {
        match self.setup_view(project).map(|view| view.state) {
            None | Some(SetupState::Failed) => Ok(()),
            Some(SetupState::Reading | SetupState::Checking) => {
                Err(other(format!("{project} is being set up already")))
            }
            Some(SetupState::Ready) => Err(other(format!(
                "{project} is set up and waits for your choice"
            ))),
            Some(SetupState::Accepted) => Err(other(format!("{project} is already set up"))),
        }
    }

    /// A hire for a new lead, as (id, file text). Refused when another hire
    /// has the same name or id.
    fn new_lead(&self, new: &NewLead) -> Result<(String, String), StudioError> {
        let (id, text) = new.hire_file().map_err(other)?;
        let name = new.name.trim();
        let taken = self
            .all_hires()
            .into_iter()
            .any(|h| h.id.0 == id || h.name.trim().to_lowercase() == name.to_lowercase());
        if taken {
            return Err(other(format!(
                "a hire called {name} already exists; pick another name"
            )));
        }
        Ok((id, text))
    }

    /// Adds the project to `studio.toml` with no `.studio.toml` of its own, and
    /// answers its entry.
    fn add_bare_project(&self, project: &NewProject<'_>) -> Result<ProjectEntry, StudioError> {
        let name = {
            let _writing = lock(&self.settings);
            let added = studio_config::add_project_entry(&self.paths, NewProject { ..*project })
                .map_err(|e| other(e.to_string()))?;
            self.reload_config();
            added.name
        };
        self.wake.notify_one();
        self.project(&ProjectId::new(&name))
    }

    /// The setup run: the lead greets the owner, reads the project and
    /// proposes how it runs, then the studio tries what it proposed.
    async fn run_setup(self: Arc<Self>, project: String, hire: Hire, repo: PathBuf) {
        lock(&self.setup_state.running).insert(project.clone());
        if let Err(why) = self.set_up(&project, &hire, &repo).await {
            let text = format!("Setting up {project} stopped: {why}");
            self.announce(&project, TalkFrom::Studio, &text, None);
            self.change_setup(&project, |view| {
                view.state = SetupState::Failed;
                view.error = Some(why);
            });
        }
        lock(&self.setup_state.running).remove(&project);
    }

    async fn set_up(
        self: &Arc<Self>,
        project: &str,
        hire: &Hire,
        repo: &Path,
    ) -> Result<(), String> {
        let who = lead_name(hire);
        let from = TalkFrom::Lead {
            hire: hire.id.0.clone(),
        };
        let greeting = format!(
            "Hi, I'm {who}. I'm reading {project} to see how it builds, tests and ships. \
             This takes a few minutes."
        );
        self.announce(project, from.clone(), &greeting, None);

        let reply = leadrun::run(
            self,
            LeadRun {
                project,
                hire,
                cwd: repo,
                prompt: prompt(project),
                tools: Tools::Setup,
                cap_usd: SETUP_USD,
                max_turns: SETUP_TURNS,
            },
        )
        .await;
        // A lead that proposed and then ran out of turns has still proposed.
        let proposed = self.setup_view(project).and_then(|view| view.proposal);
        let Some(proposal) = proposed else {
            return Err(match reply {
                Err(why) => why,
                Ok(_) => format!("{who} finished without proposing how {project} runs"),
            });
        };
        let said = reply
            .ok()
            .map(|reply| reply.text)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| proposal.summary.clone());

        self.change_setup(project, |view| view.state = SetupState::Checking);
        self.try_proposal(project, repo, &proposal).await?;
        let card = TalkCard::Setup {
            project: project.to_string(),
        };
        self.announce(project, from, &said, Some(card));
        self.change_setup(project, |view| view.state = SetupState::Ready);
        Ok(())
    }

    /// Tries each check and the capture of `proposal` in a fresh checkout of
    /// the project's branch, the way a release's final checks run, and keeps
    /// what came of each as it goes. Release commands are not run.
    async fn try_proposal(
        &self,
        project: &str,
        repo: &Path,
        proposal: &SetupProposal,
    ) -> Result<(), String> {
        let entry = self
            .project(&ProjectId::new(project))
            .map_err(|e| e.to_string())?;
        let repo = Repo::open(repo).map_err(|e| e.to_string())?;
        let rev = entry.integration_branch.as_deref().unwrap_or("HEAD");
        let name = format!("setup-{}-{}", file_stem(project), now_ms());
        let checkout = Checkout::new(&repo, &name, rev)
            .map_err(|e| format!("can't check out {project} to try the checks: {e}"))?;
        // The environment the project's own file gives, if it has one.
        let config = ProjectConfig::load(&entry.path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let env = config.env_for(&entry.path);

        for (n, check) in proposal.verify.iter().enumerate() {
            let tried = try_command(checkout.dir(), &check.command, &env).await;
            self.change_setup(project, |view| {
                if let Some(check) = view
                    .proposal
                    .as_mut()
                    .and_then(|proposal| proposal.verify.get_mut(n))
                {
                    check.tried = Some(tried);
                }
            });
        }
        if let Some(capture) = &proposal.capture {
            let out = checkout.dir().join(".studio-capture");
            let tried = try_capture(checkout.dir(), &capture.command, &out, &env).await;
            self.change_setup(project, |view| {
                if let Some(capture) = view
                    .proposal
                    .as_mut()
                    .and_then(|proposal| proposal.capture.as_mut())
                {
                    capture.tried = Some(tried);
                }
            });
        }
        Ok(())
    }

    /// Posts to the project's conversation; a conversation that can't be
    /// written to doesn't stop the setup.
    fn announce(&self, project: &str, from: TalkFrom, text: &str, card: Option<TalkCard>) {
        if let Err(err) = self.post(project, from, text, card) {
            eprintln!("setup of {project}: {err}");
        }
    }

    /// Where a project's setup is kept.
    fn setup_file(&self, project: &str) -> PathBuf {
        self.paths
            .data_dir
            .join("setup")
            .join(format!("{}.json", file_stem(project)))
    }

    /// The setup of `project`, if it has one. A setup that was reading or
    /// checking when the studio stopped never finishes, so it is failed.
    pub(super) fn setup_view(&self, project: &str) -> Option<SetupView> {
        let mut views = lock(&self.setup_state.views);
        self.load_into(&mut views, project);
        views.get(project).cloned().flatten()
    }

    /// Reads the setup of `project` from its file into `views`, unless it is
    /// there already.
    fn load_into(&self, views: &mut Views, project: &str) {
        if views.contains_key(project) {
            return;
        }
        let mut view = std::fs::read_to_string(self.setup_file(project))
            .ok()
            .and_then(|text| serde_json::from_str::<SetupView>(&text).ok());
        let running = lock(&self.setup_state.running).contains(project);
        if let Some(view) = &mut view
            && matches!(view.state, SetupState::Reading | SetupState::Checking)
            && !running
        {
            view.state = SetupState::Failed;
            view.error = Some("the studio stopped before the setup was done".to_string());
            self.write_setup(view);
        }
        views.insert(project.to_string(), view);
    }

    /// Changes the setup of `project` and keeps it; answers it as changed, or
    /// none if the project has no setup.
    fn change_setup(&self, project: &str, f: impl FnOnce(&mut SetupView)) -> Option<SetupView> {
        let mut views = lock(&self.setup_state.views);
        self.load_into(&mut views, project);
        let view = views.get_mut(project)?.as_mut()?;
        f(view);
        self.write_setup(view);
        Some(view.clone())
    }

    /// Starts `view` as the project's setup, in place of any before it.
    fn reset_setup(&self, view: &SetupView) {
        let mut views = lock(&self.setup_state.views);
        lock(&self.setup_state.running).insert(view.project.clone());
        self.write_setup(view);
        views.insert(view.project.clone(), Some(view.clone()));
    }

    /// Writes a setup to its file (by rename, so a reader never sees half of
    /// it). The setup stays in memory whether or not this works.
    fn write_setup(&self, view: &SetupView) {
        let path = self.setup_file(&view.project);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let staging = path.with_extension("json.tmp");
                std::fs::write(&staging, serde_json::to_vec_pretty(view)?)?;
                std::fs::rename(&staging, &path)
            });
        if let Err(err) = written {
            eprintln!("can't save the setup of {}: {err}", view.project);
        }
    }
}

/// What to call the lead in a message: its name, else its id.
fn lead_name(hire: &Hire) -> String {
    let name = hire.name.trim();
    if name.is_empty() {
        hire.id.0.clone()
    } else {
        name.to_string()
    }
}

/// How the lead words its last words to the owner: as it does in the
/// conversation, about what it found.
fn found_rules() -> String {
    reply_rules("what you found")
}

/// What the lead is asked.
fn prompt(project: &str) -> String {
    let reply = found_rules();
    format!(
        "You are setting up the project {project} for its owner. Your working directory is its \
repository.

Read the README, the CI configuration, the build files and the scripts, and any .studio.toml \
that is already there. Then call propose_setup once, with what you found:
- summary: one or two plain sentences on what the project is and how it is built.
- verify: the checks that show a change works, such as the build, the tests and the linter. \
Prefer what the CI runs. Each is one program with its arguments, as a list, with no shell \
syntax; for a pipe or a variable use [\"sh\", \"-c\", \"...\"]. The studio runs each in a \
fresh checkout, so pick commands that work from a clean copy and finish in a few minutes. \
Leave out anything that publishes, deploys or needs a login.
- capture: only if the project can take pictures of itself. It is a command that gets {{out}}, \
the folder to write them into.
- release: the commands the CI runs to publish a release, in order, and release_source: where \
you found them (such as \"your CI\"). Leave both empty if there are none. The studio does not \
run these now.
- protected: paths an agent must ask the owner about before changing them, such as migrations, \
secrets, release settings and the CI files, each with a reason.
- daily_usd: a daily spending limit for the project, in dollars. Keep it small, 3 to 10.
- first_goal: the first thing worth planning, in a sentence, and why. Add a version if the \
project has one in mind.

Do not guess: leave out what you could not find. Do not change any file. After you have called \
propose_setup, reply to the owner {reply} Don't list the checks: a card shows them."
    )
}

/// The proposal as the studio keeps it, or why it can't be: a command with no
/// program, a glob that doesn't compile, a limit that isn't money. The lead
/// reads the reason, so it says what to fix. Whatever the lead says about
/// having tried a command is dropped.
fn sound(mut proposal: SetupProposal) -> Result<SetupProposal, StudioError> {
    let command = |what: &str, command: &[String]| {
        if command
            .first()
            .is_none_or(|program| program.trim().is_empty())
        {
            return Err(other(format!(
                "{what} has no program to run. A command is a list: the program first, then its \
                 arguments, such as [\"make\", \"test\"]."
            )));
        }
        Ok(())
    };
    proposal.summary = proposal.summary.trim().to_string();
    if proposal.summary.is_empty() {
        return Err(other(
            "write a summary of what you found, in a sentence or two",
        ));
    }
    if proposal.verify.len() > MOST
        || proposal.release.len() > MOST
        || proposal.protected.len() > MOST
    {
        return Err(other(format!(
            "give at most {MOST} checks, release commands and paths"
        )));
    }
    for check in &mut proposal.verify {
        command("a check", &check.command)?;
        check.tried = None;
    }
    proposal.capture = proposal.capture.take().filter(|c| !c.command.is_empty());
    if let Some(capture) = &mut proposal.capture {
        command("the capture", &capture.command)?;
        capture.tried = None;
    }
    for release in &proposal.release {
        command("a release command", release)?;
    }
    for path in &proposal.protected {
        studio_config::check_glob(&path.glob).map_err(|e| {
            other(format!(
                "the protected path {} isn't a glob: {e}",
                path.glob
            ))
        })?;
    }
    if let Some(usd) = proposal.daily_usd
        && (!usd.is_finite() || usd <= 0.0)
    {
        return Err(other(format!(
            "daily_usd must be dollars above zero, not {usd}"
        )));
    }
    proposal.first_goal = proposal
        .first_goal
        .take()
        .filter(|goal| !goal.text.trim().is_empty());
    proposal.release_source = proposal.release_source.trim().to_string();
    Ok(proposal)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::api::{FirstGoal, ProposedCommand, ProtectedPath};
    use crate::studio::shipcheck::tests::{git, repo};
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::time::Duration;
    use studio_config::StudioPaths;
    use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};
    use tokio::sync::Notify;

    /// A studio over a git repository `shop`, with a lead's run played by a
    /// fake runtime that waits until the test lets it finish.
    struct Lab {
        home: tempfile::TempDir,
        _repo_home: tempfile::TempDir,
        repo: PathBuf,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
        finish: Arc<Notify>,
    }

    fn started() -> RunEvent {
        RunEvent::Started {
            session: "s1".into(),
            model: Some("fake-1".into()),
        }
    }

    fn finished(outcome: Outcome, summary: Option<&str>, usd: f64) -> RunEvent {
        RunEvent::Finished {
            outcome,
            session: Some("s1".into()),
            cost: Cost {
                usd,
                reported: true,
                input_tokens: 1_000,
                output_tokens: 200,
                cumulative: true,
            },
            summary: summary.map(String::from),
        }
    }

    fn says(words: &str) -> Vec<RunEvent> {
        vec![started(), finished(Outcome::Success, Some(words), 0.31)]
    }

    /// The studio's files, with the hire `nora` (a lead) if `nora`, and the
    /// fake runtime as the runtime of every hire.
    fn lab(events: Vec<RunEvent>, nora: bool) -> Lab {
        let home = tempfile::tempdir().unwrap();
        let repo_home = tempfile::tempdir().unwrap();
        let dir = repo_home.path().join("shop");
        repo(
            &dir,
            &[("README.md", "# Shop\n"), ("test.sh", "echo '3 passed'\n")],
        );
        std::fs::write(
            home.path().join("studio.toml"),
            "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
             [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        if nora {
            std::fs::write(
                home.path().join("hires/nora.md"),
                "---\nname: Nora\nrole: lead\nruntime: fake\nmodel: fake-1\ntools: [file_ticket]\n---\n\
                 You lead.\n",
            )
            .unwrap();
        }
        let (runtime, finish) = FakeRuntime::new(events).with_mcp().held();
        let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        runtimes.insert("fake".into(), Arc::new(runtime.clone()));
        runtimes.insert("claude-code".into(), Arc::new(runtime.clone()));
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        let studio = Studio::open(paths, runtimes).unwrap();
        Lab {
            home,
            _repo_home: repo_home,
            repo: dir,
            studio,
            runtime,
            finish,
        }
    }

    fn mira() -> NewLead {
        NewLead {
            name: "Mira".into(),
            model: "fake-1".into(),
        }
    }

    fn sh(script: &str) -> Vec<String> {
        ["sh", "-c", script].map(String::from).to_vec()
    }

    fn command(script: &str, why: &str) -> ProposedCommand {
        ProposedCommand {
            command: sh(script),
            why: why.into(),
            tried: None,
        }
    }

    /// What a lead proposes for the shop.
    fn proposal() -> SetupProposal {
        SetupProposal {
            summary: "A small shop with a test script.".into(),
            verify: vec![
                command("echo '3 passed'", "It runs the tests."),
                command("echo 'lint failed' >&2; exit 1", "It lints."),
            ],
            capture: Some(ProposedCommand {
                command: ["sh", "-c", "echo shot > \"$0/a.txt\"", "{out}"]
                    .map(String::from)
                    .to_vec(),
                why: "It takes a picture.".into(),
                tried: None,
            }),
            release: vec![vec!["./publish.sh".into()]],
            release_source: "your CI".into(),
            protected: vec![ProtectedPath {
                glob: "migrations/**".into(),
                why: "Data.".into(),
            }],
            daily_usd: Some(2.5),
            first_goal: Some(FirstGoal {
                text: "Add a cart".into(),
                version: Some("0.2".into()),
                why: "Shoppers need one.".into(),
            }),
        }
    }

    async fn ok(studio: &Arc<Studio>, request: Request) -> Value {
        let response = studio.handle(request).await;
        assert!(response.ok, "{:?}", response.error);
        response.data
    }

    async fn refused(studio: &Arc<Studio>, request: Request) -> String {
        let response = studio.handle(request).await;
        assert!(!response.ok, "should be refused: {:?}", response.data);
        response.error.unwrap()
    }

    fn setup_request(lab: &Lab, lead: Option<&str>, new_lead: Option<NewLead>) -> Request {
        Request::SetupProject {
            path: lab.repo.display().to_string(),
            name: Some("shop".into()),
            lead: lead.map(String::from),
            new_lead,
        }
    }

    async fn status(studio: &Arc<Studio>) -> SetupView {
        serde_json::from_value(
            ok(
                studio,
                Request::SetupStatus {
                    project: "shop".into(),
                },
            )
            .await,
        )
        .unwrap()
    }

    /// Waits until the setup of the shop is in one of `states`.
    async fn until(studio: &Arc<Studio>, states: &[SetupState]) -> SetupView {
        for _ in 0..600 {
            let view = status(studio).await;
            if states.contains(&view.state) {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the setup never got to {states:?}");
    }

    /// Lets the lead's run finish, after it has proposed.
    async fn propose_and_finish(lab: &Lab, proposal: SetupProposal) {
        let answer = ok(
            &lab.studio,
            Request::ProposeSetup {
                project: "shop".into(),
                proposal,
            },
        )
        .await;
        // It is told again how to word its reply, as the conversation's lead is.
        let answer = answer.as_str().unwrap();
        assert!(answer.starts_with("Thanks"), "{answer}");
        assert!(answer.contains(&reply_rules("what you found")), "{answer}");
        lab.finish.notify_one();
    }

    fn messages(lab: &Lab) -> Vec<crate::api::TalkMessage> {
        lab.studio.messages("shop", None).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_lead_reads_the_project_and_the_studio_tries_what_it_proposes() {
        let lab = lab(
            says("It builds with a script. Two checks, one fails."),
            false,
        );
        let view = ok(&lab.studio, setup_request(&lab, None, Some(mira()))).await;
        assert_eq!(view["state"], "reading");
        assert_eq!(view["lead"], "mira");

        // The lead is a hire with a lead's role, the name and model given.
        let hire = lab.studio.hire(&HireId::new("mira")).unwrap();
        assert_eq!(
            (hire.name.as_str(), hire.role.as_str(), hire.model.as_str()),
            ("Mira", "lead", "fake-1")
        );
        assert_eq!(hire.tools, ["file_ticket"]);
        assert_eq!(hire.runtime, "claude-code");
        // It leads the project, which has no checks file of its own yet.
        assert_eq!(lab.studio.lead_of("shop").unwrap().id.0, "mira");
        assert!(!lab.repo.join(".studio.toml").exists());

        // It greets the owner as it starts.
        for _ in 0..200 {
            if !messages(&lab).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let greeting = &messages(&lab)[0];
        assert_eq!(
            greeting.from,
            TalkFrom::Lead {
                hire: "mira".into()
            }
        );
        assert!(
            greeting.text.starts_with("Hi, I'm Mira."),
            "{}",
            greeting.text
        );

        propose_and_finish(&lab, proposal()).await;
        let view = until(&lab.studio, &[SetupState::Ready]).await;
        assert!(view.error.is_none());
        let proposal = view.proposal.unwrap();
        let tried = |command: &ProposedCommand| command.tried.clone().unwrap();
        let tests = tried(&proposal.verify[0]);
        assert!(tests.passed);
        assert_eq!(tests.summary, "3 passed");
        assert_eq!(tests.output_tail, "3 passed");
        let lint = tried(&proposal.verify[1]);
        assert!(!lint.passed);
        assert_eq!(lint.summary, "lint failed");
        let capture = tried(proposal.capture.as_ref().unwrap());
        assert!(capture.passed);
        assert_eq!(capture.summary, "1 file");
        // What it released with is kept as proposed, and not tried.
        assert_eq!(proposal.release, [["./publish.sh"]]);
        assert_eq!(proposal.release_source, "your CI");
        assert_eq!(proposal.daily_usd, Some(2.5));
        assert_eq!(proposal.first_goal.unwrap().version.as_deref(), Some("0.2"));

        // The lead's last words come with a card about the setup.
        let all = messages(&lab);
        assert_eq!(all.len(), 2);
        assert_eq!(
            all[1].text,
            "It builds with a script. Two checks, one fails."
        );
        assert_eq!(
            all[1].card,
            Some(TalkCard::Setup {
                project: "shop".into()
            })
        );
        // The office has it too.
        assert_eq!(lab.studio.setups()["shop"].state, SetupState::Ready);
        // The tries left nothing behind.
        assert!(std::fs::read_dir(lab.repo.join(".studio/releases")).map_or(0, |d| d.count()) == 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_run_is_the_leads_read_only_setup_run_in_the_repository() {
        let lab = lab(says("Done."), true);
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        propose_and_finish(&lab, proposal()).await;
        until(&lab.studio, &[SetupState::Ready]).await;

        let specs = lab.runtime.specs();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.cwd, lab.repo.canonicalize().unwrap());
        assert_eq!(spec.max_turns, Some(20));
        assert_eq!(spec.max_budget_usd, Some(0.6));
        assert!(spec.strict_mcp);
        assert!(spec.disallowed_tools.contains(&"Bash".to_string()));
        for word in [
            "README",
            "CI configuration",
            "build files",
            "scripts",
            "propose_setup",
        ] {
            assert!(spec.prompt.contains(word), "{word}: {}", spec.prompt);
        }
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(spec.mcp_config.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        let args = config["mcpServers"]["studio"]["args"].as_array().unwrap();
        assert_eq!(args[1], "--setup");
        assert_eq!(args[2], "shop");
        // It is charged to the project's talk, like any lead run.
        let budget = ok(&lab.studio, Request::Budget).await;
        assert_eq!(budget["spent_24h"]["by_ticket"], json!({"shop/talk": 0.31}));
    }

    #[test]
    fn the_lead_is_asked_to_reply_as_it_is_in_the_conversation() {
        let prompt = prompt("shop");
        for rule in [
            "in plain words, in the first person",
            "at most three short sentences of about 15 words each",
            "about what you found",
            "One thought to a sentence: no semicolons and no lists",
            "Don't list the checks: a card shows them",
        ] {
            assert!(prompt.contains(rule), "{rule}: {prompt}");
        }
        // The rules are the conversation's, not a word count of its own.
        assert!(prompt.contains(&reply_rules("what you found")), "{prompt}");
        assert!(!prompt.contains("50 words"), "{prompt}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_existing_lead_takes_the_project_and_a_taken_name_is_refused() {
        let lab = lab(says("Done."), true);
        // Nora, by name or by the id the name would make.
        for name in ["Nora", "nora", " NORA ", "Nora!"] {
            let new = NewLead {
                name: name.into(),
                model: "fake-1".into(),
            };
            let why = refused(&lab.studio, setup_request(&lab, None, Some(new))).await;
            assert!(why.contains("already exists"), "{name}: {why}");
        }
        // Nothing was made: no second hire, no project.
        assert_eq!(lab.studio.all_hires().len(), 1);
        assert!(lab.studio.config().projects.is_empty());
        // Both or neither is a mistake too.
        let both = refused(&lab.studio, setup_request(&lab, Some("nora"), Some(mira()))).await;
        assert!(both.contains("not both"), "{both}");
        let neither = refused(&lab.studio, setup_request(&lab, None, None)).await;
        assert!(neither.contains("give the project a lead"), "{neither}");
        let unknown = refused(&lab.studio, setup_request(&lab, Some("ghost"), None)).await;
        assert!(unknown.contains("ghost"), "{unknown}");
        assert!(lab.studio.config().projects.is_empty());

        let view = ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        assert_eq!(view["lead"], "nora");
        assert_eq!(lab.studio.lead_of("shop").unwrap().id.0, "nora");
        // The project can't be set up twice at once.
        let again = refused(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        assert!(again.contains("being set up already"), "{again}");
        lab.finish.notify_one();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lead_that_proposes_nothing_fails_the_setup_in_plain_words() {
        let lab = lab(says("I had a look."), true);
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        lab.finish.notify_one();
        let view = until(&lab.studio, &[SetupState::Failed]).await;
        assert_eq!(
            view.error.as_deref(),
            Some("Nora finished without proposing how shop runs")
        );
        assert!(view.proposal.is_none());
        let last = messages(&lab).pop().unwrap();
        assert_eq!(last.from, TalkFrom::Studio);
        assert_eq!(
            last.text,
            "Setting up shop stopped: Nora finished without proposing how shop runs"
        );
        // A failed setup can be started again.
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        assert_eq!(status(&lab.studio).await.state, SetupState::Reading);
        lab.finish.notify_one();
        until(&lab.studio, &[SetupState::Failed]).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_that_fails_fails_the_setup_with_its_reason() {
        let failed = Outcome::Failed {
            kind: studio_runtime::FailureKind::Crash,
            message: "the model fell over".into(),
        };
        let lab = lab(vec![started(), finished(failed, None, 0.02)], true);
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        lab.finish.notify_one();
        let view = until(&lab.studio, &[SetupState::Failed]).await;
        assert_eq!(
            view.error.as_deref(),
            Some("Nora failed: the model fell over")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_proposal_that_cannot_run_is_sent_back_to_the_lead() {
        let lab = lab(says("Done."), true);
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        let propose = |proposal: SetupProposal| Request::ProposeSetup {
            project: "shop".into(),
            proposal,
        };
        let mut no_summary = proposal();
        no_summary.summary = "  ".into();
        assert!(
            refused(&lab.studio, propose(no_summary))
                .await
                .contains("summary")
        );
        let mut no_program = proposal();
        no_program.verify[0].command = vec![" ".into()];
        assert!(
            refused(&lab.studio, propose(no_program))
                .await
                .contains("no program")
        );
        let mut bad_glob = proposal();
        bad_glob.protected[0].glob = "src/[".into();
        assert!(
            refused(&lab.studio, propose(bad_glob))
                .await
                .contains("src/[")
        );
        let mut no_money = proposal();
        no_money.daily_usd = Some(-1.0);
        assert!(
            refused(&lab.studio, propose(no_money))
                .await
                .contains("daily_usd")
        );
        assert!(status(&lab.studio).await.proposal.is_none());

        // What it says it tried is not believed, and an empty capture is none.
        let mut bragging = proposal();
        bragging.verify[0].tried = Some(studio_core_run());
        bragging.capture = Some(ProposedCommand::default());
        ok(&lab.studio, propose(bragging)).await;
        let kept = status(&lab.studio).await.proposal.unwrap();
        assert!(kept.verify[0].tried.is_none() && kept.capture.is_none());
        // Only once.
        let twice = refused(&lab.studio, propose(proposal())).await;
        assert!(twice.contains("already"), "{twice}");
        assert!(twice.contains(&reply_rules("what you found")), "{twice}");
        lab.finish.notify_one();
        until(&lab.studio, &[SetupState::Ready]).await;
        // And not when nothing is being set up.
        let late = refused(&lab.studio, propose(proposal())).await;
        assert!(late.contains("isn't being set up"), "{late}");
    }

    fn studio_core_run() -> crate::api::CommandRun {
        crate::api::CommandRun {
            passed: true,
            seconds: 1,
            summary: "all good".into(),
            output_tail: String::new(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_with_no_lead_run_yet_has_no_status() {
        let lab = lab(says("Done."), true);
        let project = format!(
            "\n[[projects]]\nname = \"shop\"\npath = \"{}\"\n",
            lab.repo.display()
        );
        let toml = lab.home.path().join("studio.toml");
        std::fs::write(&toml, std::fs::read_to_string(&toml).unwrap() + &project).unwrap();
        lab.studio.reload_config();
        let why = refused(
            &lab.studio,
            Request::SetupStatus {
                project: "shop".into(),
            },
        )
        .await;
        assert_eq!(why, "shop wasn't set up through its lead");
        assert!(lab.studio.setups().is_empty());
        let nobody = refused(
            &lab.studio,
            Request::SetupStatus {
                project: "ghost".into(),
            },
        )
        .await;
        assert!(nobody.contains("ghost"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_project_nobody_set_up_is_set_up_by_its_lead_and_its_files_stay_as_they_were() {
        // The shop is in studio.toml already, led by Nora, with checks of its
        // own in its .studio.toml.
        let lab = lab(says("Done."), true);
        let own = "verify = [[\"make\"]]\nauto_land = true\n";
        std::fs::write(lab.repo.join(".studio.toml"), own).unwrap();
        git(&lab.repo, &["add", "-A"]);
        git(&lab.repo, &["commit", "-q", "-m", "Checks"]);
        let studio_toml = lab.home.path().join("studio.toml");
        let project = format!(
            "\n[[projects]]\nname = \"shop\"\npath = \"{}\"\nlead = \"nora\"\n",
            lab.repo.display()
        );
        std::fs::write(
            &studio_toml,
            std::fs::read_to_string(&studio_toml).unwrap() + &project,
        )
        .unwrap();
        lab.studio.reload_config();
        let before = std::fs::read_to_string(&studio_toml).unwrap();
        assert!(lab.studio.setups().is_empty());

        // Its lead sets it up, as for a project that was just added.
        let view = ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        assert_eq!(view["state"], "reading");
        assert_eq!(view["lead"], "nora");
        propose_and_finish(&lab, proposal()).await;
        let ready = until(&lab.studio, &[SetupState::Ready]).await;
        assert!(ready.proposal.unwrap().verify[0].tried.is_some());

        // Nothing of the project's was written: not its file of checks, and not
        // the studio's own, whose project is still the one it was, led by Nora.
        assert_eq!(
            std::fs::read_to_string(lab.repo.join(".studio.toml")).unwrap(),
            own
        );
        assert_eq!(std::fs::read_to_string(&studio_toml).unwrap(), before);
        assert_eq!(lab.studio.config().projects.len(), 1);
        assert_eq!(lab.studio.lead_of("shop").unwrap().id.0, "nora");

        // Only the owner's choice changes the project's file.
        ok(
            &lab.studio,
            Request::AcceptSetup {
                project: "shop".into(),
                choice: choice(),
            },
        )
        .await;
        let accepted = std::fs::read_to_string(lab.repo.join(".studio.toml")).unwrap();
        assert!(!accepted.contains("make"), "{accepted}");
        assert!(accepted.contains("auto_land = true"), "{accepted}");
        // And a setup is asked for once: it is done now.
        let again = refused(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        assert_eq!(again, "shop is already set up");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_cut_off_by_a_restart_is_failed_when_the_studio_reads_it() {
        let lab = lab(says("Done."), true);
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        lab.finish.notify_one();
        until(&lab.studio, &[SetupState::Failed]).await;

        // The file says checking, as an earlier studio left it; no run is
        // under way in the studio that reads it now.
        let paths = StudioPaths::resolve(Some(lab.home.path().to_path_buf()));
        let file = paths.data_dir.join("setup/shop.json");
        let mut kept: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        kept["state"] = json!("checking");
        kept["error"] = Value::Null;
        std::fs::write(&file, kept.to_string()).unwrap();
        let later = Studio::open(paths, HashMap::new()).unwrap();
        let view = later.setup_view("shop").unwrap();
        assert_eq!(view.state, SetupState::Failed);
        assert_eq!(
            view.error.as_deref(),
            Some("the studio stopped before the setup was done")
        );
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(on_disk["state"], "failed");
    }

    /// A shop that is set up and ready, with `.studio.toml` as `before` says.
    async fn ready(before: Option<&str>) -> Lab {
        let lab = lab(says("Done."), true);
        if let Some(before) = before {
            std::fs::write(lab.repo.join(".studio.toml"), before).unwrap();
            git(&lab.repo, &["add", "-A"]);
            git(&lab.repo, &["commit", "-q", "-m", "Checks"]);
        }
        ok(&lab.studio, setup_request(&lab, Some("nora"), None)).await;
        propose_and_finish(&lab, proposal()).await;
        until(&lab.studio, &[SetupState::Ready]).await;
        lab
    }

    fn choice() -> SetupChoice {
        SetupChoice {
            verify: vec![sh("./test.sh")],
            capture: Vec::new(),
            release: vec![vec!["./publish.sh".into()]],
            protected: vec!["migrations/**".into()],
            daily_usd: Some(2.5),
            plan_first: false,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepting_writes_the_checks_into_a_file_that_has_comments_already() {
        let before = "# How the shop is checked.\n\
            verify = [[\"make\"]] # the old way\n\
            auto_land = true\n\
            \n\
            # The cache.\n\
            [env]\n\
            CACHE = \"{repo}/.cache\"\n";
        let lab = ready(Some(before)).await;
        let accepted = ok(
            &lab.studio,
            Request::AcceptSetup {
                project: "shop".into(),
                choice: choice(),
            },
        )
        .await;
        assert_eq!(accepted["state"], "accepted");

        let text = std::fs::read_to_string(lab.repo.join(".studio.toml")).unwrap();
        assert!(
            text.starts_with("# How the shop is checked.\nverify = [\n"),
            "{text}"
        );
        assert!(
            text.contains("] # the old way\nauto_land = true\n"),
            "{text}"
        );
        assert!(
            text.contains("# The cache.\n[env]\nCACHE = \"{repo}/.cache\"\n"),
            "{text}"
        );
        assert!(!text.contains("make"));
        let config = ProjectConfig::load(&lab.repo).unwrap().unwrap();
        assert_eq!(config.verify, [sh("./test.sh")]);
        assert!(config.capture.is_empty());
        assert_eq!(config.release, [["./publish.sh"]]);
        assert_eq!(config.protected, ["migrations/**"]);
        assert!(config.auto_land);

        // The daily cap is the project's, in studio.toml.
        let studio_toml = std::fs::read_to_string(lab.home.path().join("studio.toml")).unwrap();
        assert!(studio_toml.contains("daily_usd = 2.5\n"), "{studio_toml}");
        assert_eq!(lab.studio.config().projects[0].daily_usd, Some(2.5));
        assert_eq!(status(&lab.studio).await.state, SetupState::Accepted);
        // Nothing planned, since the owner didn't ask.
        assert!(
            lab.studio
                .handle(Request::Tickets)
                .await
                .data
                .as_array()
                .unwrap()
                .is_empty()
        );
        // And only once.
        let again = refused(
            &lab.studio,
            Request::AcceptSetup {
                project: "shop".into(),
                choice: choice(),
            },
        )
        .await;
        assert_eq!(again, "shop is already set up");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepting_a_project_with_no_file_makes_one() {
        let lab = ready(None).await;
        ok(
            &lab.studio,
            Request::AcceptSetup {
                project: "shop".into(),
                choice: SetupChoice {
                    capture: sh("./shots.sh {out}"),
                    daily_usd: None,
                    ..choice()
                },
            },
        )
        .await;
        let config = ProjectConfig::load(&lab.repo).unwrap().unwrap();
        assert_eq!(config.capture, sh("./shots.sh {out}"));
        assert_eq!(lab.studio.config().projects[0].daily_usd, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn accepting_with_plan_first_starts_the_first_goal_with_the_lead() {
        let lab = ready(None).await;
        ok(
            &lab.studio,
            Request::AcceptSetup {
                project: "shop".into(),
                choice: SetupChoice {
                    plan_first: true,
                    ..choice()
                },
            },
        )
        .await;
        let tickets = ok(&lab.studio, Request::Tickets).await;
        let tickets = tickets.as_array().unwrap();
        assert_eq!(tickets.len(), 1);
        let plan = &tickets[0];
        assert_eq!(plan["kind"], "plan");
        assert_eq!(plan["hire"], "nora");
        assert_eq!(plan["goal"], "Add a cart");
        assert_eq!(plan["version"], "0.2");
        assert_eq!(plan["detail"], "Shoppers need one.");
        let last = messages(&lab).pop().unwrap();
        assert_eq!(
            last.from,
            TalkFrom::Lead {
                hire: "nora".into()
            }
        );
        assert!(last.text.contains("Add a cart"), "{}", last.text);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_choice_that_cannot_be_kept_is_refused_before_anything_is_written() {
        let lab = ready(None).await;
        let accept = |choice: SetupChoice| Request::AcceptSetup {
            project: "shop".into(),
            choice,
        };
        let no_goal = {
            let mut bare = proposal();
            bare.first_goal = None;
            lab.studio.change_setup("shop", |v| v.proposal = Some(bare));
            refused(
                &lab.studio,
                accept(SetupChoice {
                    plan_first: true,
                    ..choice()
                }),
            )
            .await
        };
        assert!(no_goal.contains("no first goal"), "{no_goal}");
        let blank = refused(
            &lab.studio,
            accept(SetupChoice {
                verify: vec![vec![String::new()]],
                ..choice()
            }),
        )
        .await;
        assert!(blank.contains("no program"), "{blank}");
        let money = refused(
            &lab.studio,
            accept(SetupChoice {
                daily_usd: Some(0.0),
                ..choice()
            }),
        )
        .await;
        assert!(money.contains("more than zero"), "{money}");
        assert!(!lab.repo.join(".studio.toml").exists());
        assert_eq!(status(&lab.studio).await.state, SetupState::Ready);
        // A setup that isn't ready can't be accepted.
        lab.studio
            .change_setup("shop", |v| v.state = SetupState::Checking);
        let early = refused(&lab.studio, accept(choice())).await;
        assert!(early.contains("still setting up"), "{early}");
    }

    #[test]
    fn a_leads_name_makes_its_hire_file() {
        let lab_dir = tempfile::tempdir().unwrap();
        let paths = StudioPaths::resolve(Some(lab_dir.path().to_path_buf()));
        std::fs::write(
            paths.config_dir.join("studio.toml"),
            "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n",
        )
        .unwrap();
        let studio = Studio::open(paths, HashMap::new()).unwrap();
        let (id, text) = studio
            .new_lead(&NewLead {
                name: "Mira Lee".into(),
                model: "opus".into(),
            })
            .unwrap();
        assert_eq!(id, "mira-lee");
        assert!(text.starts_with("---\nname: \"Mira Lee\"\nrole: lead\nruntime: claude-code\nmodel: \"opus\"\ntools: [file_ticket]\n---\n"));
        studio_config::check_hire(&id, &text).unwrap();
        for bad in ["", "   ", "日本", &"x".repeat(41)] {
            let why = studio
                .new_lead(&NewLead {
                    name: bad.into(),
                    model: "opus".into(),
                })
                .unwrap_err()
                .to_string();
            assert!(
                why.contains("name") || why.contains("letters"),
                "{bad:?}: {why}"
            );
        }
        let why = studio
            .new_lead(&NewLead {
                name: "Mira".into(),
                model: " ".into(),
            })
            .unwrap_err()
            .to_string();
        assert!(why.contains("model"), "{why}");
    }
}
