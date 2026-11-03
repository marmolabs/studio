//! What the office says about finished work and each lead's day: a line on
//! what a task came to, and a standup from each lead.
//!
//! A lead's standup is one or two short sentences on the project's day. It is
//! written by one cheap, tool-less call to the lead's hire (on a small model,
//! capped at `STANDUP_USD`, charged to the project's `{project}/talk` task),
//! at most once every 6 hours and only when something happened since the last
//! one. A project with no lead, or a call that fails, gets a plain line made
//! from the same facts. The standups are kept in `<data>/talk/standups.json`.

use super::{Studio, StudioError, lock};
use crate::api::{ReleaseState, Standup};
use crate::budget::DAY_MS;
use crate::leadrun::talk_task;
use crate::today::{self, Day};
use crate::{now_ms, quick, runview};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_config::config::ProjectEntry;
use studio_core::rule::Hire;
use studio_core::ticket::{Ticket, TicketKind};

/// The least time between two standups of a project.
const EVERY_MS: u64 = 6 * 60 * 60 * 1000;

/// How often the studio looks for a standup that is due.
const LOOK_EVERY: Duration = Duration::from_secs(5 * 60);

/// The model a standup is written on, whatever the lead's own is.
const CHEAP_MODEL: &str = "haiku";

/// The most a standup may cost.
const STANDUP_USD: f64 = 0.02;

/// The most characters of a standup.
const WORDS_MAX: usize = 300;

/// The most finished tasks the lead is shown by name.
const TASKS_SHOWN: usize = 8;

/// What the standups keep in memory.
#[derive(Default)]
pub(super) struct State {
    /// The standups as last read from their file or written to it.
    kept: Mutex<Option<BTreeMap<String, Standup>>>,
    /// When the studio last looked for a standup that is due.
    looked: Mutex<Option<Instant>>,
    /// The projects whose standup is being written now.
    writing: Mutex<BTreeSet<String>>,
}

impl Studio {
    /// A line on what each finished task (done, ready or shipped) came to,
    /// by ticket id: the first sentence of its agent's last words, at most 140
    /// characters, with the markdown taken out. A task that ended without any
    /// has none.
    pub(super) fn results(&self, tickets: &[Ticket]) -> BTreeMap<String, String> {
        let runs = self.paths.data_dir.join("runs");
        tickets
            .iter()
            .filter(|t| matches!(t.state.kind(), "ready" | "landed"))
            .filter_map(|t| {
                let id = t.id.to_string();
                Some((id.clone(), runview::result_of(&runs, &id)?))
            })
            .collect()
    }

    /// The latest standup of each project's lead, by project name: one or two
    /// short sentences, written at most once every 6 hours and only when
    /// something happened. A project without one is left out.
    pub(super) fn standups(&self) -> BTreeMap<String, Standup> {
        let mut kept = lock(&self.standup_state.kept);
        kept.get_or_insert_with(|| self.read_standups()).clone()
    }

    fn standups_path(&self) -> PathBuf {
        self.paths.data_dir.join("talk").join("standups.json")
    }

    fn read_standups(&self) -> BTreeMap<String, Standup> {
        std::fs::read_to_string(self.standups_path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Keeps `standup` as the latest of `project`.
    fn keep_standup(&self, project: &str, standup: Standup) -> Result<(), StudioError> {
        let mut kept = lock(&self.standup_state.kept);
        let all = kept.get_or_insert_with(|| self.read_standups());
        all.insert(project.to_string(), standup);
        let path = self.standups_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| super::other(format!("can't keep the standups: {e}")))?;
        }
        let text = serde_json::to_string_pretty(&*all)
            .map_err(|e| super::other(format!("can't keep the standups: {e}")))?;
        std::fs::write(&path, text)
            .map_err(|e| super::other(format!("can't keep the standups: {e}")))
    }

    /// Looks for standups that are due, now and then: the scheduler calls this
    /// on every pass, and it only acts once in `LOOK_EVERY`. The first look is
    /// that long after the studio starts. The standups are written in a task
    /// of their own.
    pub(super) fn spawn_standups(self: &Arc<Self>) {
        {
            let mut looked = lock(&self.standup_state.looked);
            match *looked {
                None => {
                    *looked = Some(Instant::now());
                    return;
                }
                Some(at) if at.elapsed() < LOOK_EVERY => return,
                Some(_) => *looked = Some(Instant::now()),
            }
        }
        let studio = Arc::clone(self);
        tokio::spawn(async move { studio.write_standups().await });
    }

    /// Writes the standup of every project whose is due. Returns the projects
    /// that got one.
    pub(crate) async fn write_standups(self: &Arc<Self>) -> Vec<String> {
        self.write_standups_at(now_ms()).await
    }

    /// `write_standups` as of `now` (epoch milliseconds).
    async fn write_standups_at(self: &Arc<Self>, now: u64) -> Vec<String> {
        let mut written = Vec::new();
        for entry in self.config().projects.clone() {
            // One at a time for each project, however the calls overlap.
            if !lock(&self.standup_state.writing).insert(entry.name.clone()) {
                continue;
            }
            let wrote = self.write_standup(&entry, now).await;
            lock(&self.standup_state.writing).remove(&entry.name);
            match wrote {
                Ok(true) => written.push(entry.name),
                Ok(false) => {}
                Err(err) => eprintln!("standup of {}: {err}", entry.name),
            }
        }
        written
    }

    /// The standup of one project, when it is due and something happened.
    async fn write_standup(
        self: &Arc<Self>,
        entry: &ProjectEntry,
        now: u64,
    ) -> Result<bool, StudioError> {
        let last = self.standups().get(&entry.name).map(|s| s.at);
        if last.is_some_and(|at| now.saturating_sub(at) < EVERY_MS) {
            return Ok(false);
        }
        // What happened since the last standup, or in the last day.
        let day_ago = now.saturating_sub(DAY_MS);
        let since = last.map_or(day_ago, |at| (at + 1).max(day_ago));
        let (tickets, events) = {
            let store = lock(&self.store);
            (
                store.tickets()?,
                store.events_from(since, usize::MAX, |_| true)?,
            )
        };
        let day = today::project_day(&entry.name, &tickets, &events);
        if day.is_quiet() {
            return Ok(false);
        }
        let waiting = self.waiting_for_owner(entry, &tickets);
        let words = match self.lead_of(&entry.name) {
            Some(lead) => {
                let prompt = prompt(&entry.name, &day, &tickets, waiting, &self.paths.data_dir);
                self.lead_standup(entry, &lead, &prompt).await
            }
            None => None,
        };
        let text = words.unwrap_or_else(|| fallback(&day, waiting));
        self.keep_standup(&entry.name, Standup { at: now, text })?;
        Ok(true)
    }

    /// What the lead says, from one cheap call; None if the call gave nothing.
    async fn lead_standup(
        self: &Arc<Self>,
        entry: &ProjectEntry,
        lead: &Hire,
        prompt: &str,
    ) -> Option<String> {
        let cheap = Hire {
            model: CHEAP_MODEL.to_string(),
            escalate: Vec::new(),
            effort: None,
            ..lead.clone()
        };
        let cwd = if entry.path.is_dir() {
            entry.path.clone()
        } else {
            self.paths.data_dir.clone()
        };
        let said = quick::ask_model_capped(
            self,
            &talk_task(&entry.name),
            &entry.name,
            &cheap,
            &cwd,
            prompt,
            STANDUP_USD,
        )
        .await
        .ok()?;
        let said = runview::plain(&said);
        let said = said.trim();
        if said.is_empty() {
            return None;
        }
        Some(if said.chars().count() > WORDS_MAX {
            let head: String = said.chars().take(WORDS_MAX - 1).collect();
            format!("{}…", head.trim_end())
        } else {
            said.to_string()
        })
    }

    /// How many things of `entry`'s project wait for the owner: asks, work
    /// ready to ship and a release waiting for a go-ahead.
    fn waiting_for_owner(&self, entry: &ProjectEntry, tickets: &[Ticket]) -> usize {
        let ours = |ticket: &str| {
            tickets
                .iter()
                .any(|t| t.id.to_string() == ticket && t.project.0 == entry.name)
        };
        let asks = lock(&self.asks)
            .values()
            .filter(|a| ours(&a.view.ticket))
            .count();
        let ready = tickets
            .iter()
            .filter(|t| t.project.0 == entry.name)
            .filter(|t| t.kind == TicketKind::Work && t.state.kind() == "ready")
            .count();
        let release = self
            .release_fields(entry)
            .1
            .is_some_and(|r| r.state == ReleaseState::Waiting);
        asks + ready + usize::from(release)
    }
}

/// What the lead is asked to write.
fn prompt(
    project: &str,
    day: &Day,
    tickets: &[Ticket],
    waiting: usize,
    data_dir: &std::path::Path,
) -> String {
    let runs = data_dir.join("runs");
    let mut facts = Vec::new();
    for id in day.done.iter().take(TASKS_SHOWN) {
        let Some(ticket) = tickets.iter().find(|t| &t.id.to_string() == id) else {
            continue;
        };
        facts.push(match runview::result_of(&runs, id) {
            Some(result) => format!("- Done: {} ({result})", ticket.title),
            None => format!("- Done: {}", ticket.title),
        });
    }
    if day.done.len() > TASKS_SHOWN {
        facts.push(format!("- and {} more done", day.done.len() - TASKS_SHOWN));
    }
    if day.sent_back > 0 {
        facts.push(format!("- Sent back for changes: {}", day.sent_back));
    }
    if day.failed > 0 {
        facts.push(format!("- Failed: {}", day.failed));
    }
    if day.planned > 0 {
        facts.push(format!("- Goals planned: {}", day.planned));
    }
    if waiting > 0 {
        facts.push(format!("- Waiting for the owner: {waiting}"));
    }
    format!(
        "You lead the project {project} and write your standup for the owner.\n\n\
         Since the last one:\n{}\n\n\
         Write the standup: one or two short sentences in plain words, as yourself talking \
         to the owner. Say what got done and what waits for them, if anything. Use only these \
         facts. No markdown, no lists, no greeting. Don't use any tools. Reply with only the \
         sentences.",
        facts.join("\n")
    )
}

/// The standup when the lead has none to give: what happened and what waits,
/// such as "2 tasks done, 1 sent back. 1 waits for you."
fn fallback(day: &Day, waiting: usize) -> String {
    let mut words = day.words();
    if let Some(first) = words.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    let mut text = format!("{words}.");
    match waiting {
        0 => {}
        1 => text.push_str(" 1 waits for you."),
        n => text.push_str(&format!(" {n} wait for you.")),
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use studio_config::StudioPaths;
    use studio_core::event::Event;
    use studio_core::ids::TicketId;
    use studio_core::ticket::TicketState;
    use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};

    fn words(text: &str) -> Vec<RunEvent> {
        vec![
            RunEvent::Text { text: text.into() },
            RunEvent::Finished {
                outcome: Outcome::Success,
                session: Some("s1".into()),
                cost: Cost {
                    usd: 0.004,
                    reported: true,
                    ..Cost::default()
                },
                summary: Some(text.into()),
            },
        ]
    }

    /// A studio with the project `shop`, which has the lead `nora` when
    /// `led`, and a hire `dev`, all on `runtime`.
    fn studio_with(led: bool, runtime: &FakeRuntime) -> (tempfile::TempDir, Arc<Studio>) {
        let home = tempfile::tempdir().unwrap();
        let lead = if led { "lead = \"nora\"\n" } else { "" };
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\n{lead}",
                home.path().join("shop").display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        std::fs::write(
            home.path().join("hires/dev.md"),
            "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nYou write code.\n",
        )
        .unwrap();
        std::fs::write(
            home.path().join("hires/nora.md"),
            "---\nname: Nora\nrole: lead\nruntime: fake\nmodel: fake-big\neffort: high\n---\nYou lead the shop.\n",
        )
        .unwrap();
        let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        runtimes.insert("fake".into(), Arc::new(runtime.clone()));
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        (home, Studio::open(paths, runtimes).unwrap())
    }

    /// A task of `shop` that has passed review.
    fn done(studio: &Studio, title: &str) -> TicketId {
        let ticket = studio
            .create_ticket(super::super::NewTicket {
                project: "shop".into(),
                title: title.into(),
                goal: String::new(),
                hire: "dev".into(),
                base: None,
                kind: TicketKind::Work,
                parent: None,
                detail: String::new(),
                source: None,
                version: None,
            })
            .unwrap();
        for event in [
            Event::Claimed,
            Event::RunStarted {
                run: studio_core::ids::RunId::new("s1"),
            },
            Event::ChecksStarted,
            Event::ChecksPassed,
            Event::ReviewPassed,
        ] {
            studio.append(&ticket.id, event).unwrap();
        }
        ticket.id
    }

    #[tokio::test]
    async fn a_lead_writes_the_standup_on_a_cheap_model_and_the_task_pays() {
        let fake = FakeRuntime::new(words("Two tasks are done.  **Both** wait for you."));
        let (_home, studio) = studio_with(true, &fake);
        done(&studio, "Add a cart drawer");
        done(&studio, "Fix the total");
        assert!(studio.standups().is_empty());

        assert_eq!(studio.write_standups().await, ["shop"]);

        let standup = studio.standups()["shop"].clone();
        assert_eq!(standup.text, "Two tasks are done. Both wait for you.");
        assert!(standup.at > 0);
        // One call, to the lead, on the cheap model and under its cap.
        let specs = fake.specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].model.as_deref(), Some(CHEAP_MODEL));
        assert_eq!(specs[0].max_budget_usd, Some(STANDUP_USD));
        assert_eq!(specs[0].max_turns, Some(1));
        assert_eq!(specs[0].effort, None);
        assert_eq!(
            specs[0].append_system_prompt.as_deref(),
            Some("You lead the shop.")
        );
        assert!(specs[0].prompt.contains("Add a cart drawer"));
        assert!(specs[0].prompt.contains("Fix the total"));
        // What it cost is charged to the project's talk task.
        let spent = lock(&studio.store)
            .ticket_spent(&talk_task("shop"))
            .unwrap();
        assert!((spent - 0.004).abs() < 1e-9, "{spent}");
        // It is kept on disk, for the next time the studio starts.
        let file =
            std::fs::read_to_string(studio.paths.data_dir.join("talk/standups.json")).unwrap();
        assert!(file.contains("Both wait for you."));
    }

    #[tokio::test]
    async fn a_standup_is_written_at_most_once_in_six_hours_and_only_after_something_happened() {
        let fake = FakeRuntime::new(words("All good."));
        let (_home, studio) = studio_with(true, &fake);
        // A quiet day: nothing is written, nothing is asked.
        assert!(studio.write_standups().await.is_empty());
        assert!(fake.specs().is_empty());
        assert!(studio.standups().is_empty());

        done(&studio, "Add a cart drawer");
        let now = now_ms();
        assert_eq!(studio.write_standups_at(now).await, ["shop"]);
        // More work, but the last standup is fresh.
        tokio::time::sleep(Duration::from_millis(5)).await;
        done(&studio, "Fix the total");
        assert!(
            studio
                .write_standups_at(now + EVERY_MS - 1)
                .await
                .is_empty()
        );
        assert_eq!(fake.specs().len(), 1);

        // Six hours on, with work since, it is written again.
        let later = now + EVERY_MS + 1;
        assert_eq!(studio.write_standups_at(later).await, ["shop"]);
        assert_eq!(fake.specs().len(), 2);
        assert_eq!(studio.standups()["shop"].at, later);
        // Another six hours on, with nothing since, it is not.
        assert!(
            studio
                .write_standups_at(later + EVERY_MS + 1)
                .await
                .is_empty()
        );
        assert_eq!(fake.specs().len(), 2);
        assert_eq!(studio.standups()["shop"].at, later);
    }

    #[tokio::test]
    async fn a_project_without_a_lead_gets_a_plain_standup_from_the_facts() {
        let fake = FakeRuntime::new(words("Never asked."));
        let (_home, studio) = studio_with(false, &fake);
        let first = done(&studio, "Add a cart drawer");
        done(&studio, "Fix the total");
        studio
            .append(
                &first,
                Event::ChangesRequested {
                    detail: "Redo it".into(),
                },
            )
            .unwrap();
        assert_eq!(studio.write_standups().await, ["shop"]);
        assert!(fake.specs().is_empty());
        // The first task went back; the second still waits to be shipped.
        assert_eq!(
            studio.standups()["shop"].text,
            "2 tasks done, 1 sent back. 1 waits for you."
        );
    }

    #[tokio::test]
    async fn a_call_that_fails_leaves_the_plain_standup() {
        let fake = FakeRuntime::new(vec![RunEvent::Finished {
            outcome: Outcome::Failed {
                kind: studio_runtime::FailureKind::Crash,
                message: "it broke".into(),
            },
            session: None,
            cost: Cost::default(),
            summary: None,
        }]);
        let (_home, studio) = studio_with(true, &fake);
        done(&studio, "Add a cart drawer");
        assert_eq!(studio.write_standups().await, ["shop"]);
        assert_eq!(fake.specs().len(), 1);
        assert_eq!(
            studio.standups()["shop"].text,
            "1 task done. 1 waits for you."
        );
    }

    #[test]
    fn the_plain_standup_reads_like_a_person_wrote_it() {
        let day = Day {
            done: vec!["A".into(), "B".into()],
            sent_back: 1,
            ..Day::default()
        };
        assert_eq!(
            fallback(&day, 1),
            "2 tasks done, 1 sent back. 1 waits for you."
        );
        assert_eq!(fallback(&day, 0), "2 tasks done, 1 sent back.");
        assert_eq!(
            fallback(&day, 3),
            "2 tasks done, 1 sent back. 3 wait for you."
        );
    }

    #[test]
    fn standups_are_read_back_from_their_file() {
        let (home, studio) = studio_with(true, &FakeRuntime::default());
        assert!(studio.standups().is_empty());
        std::fs::create_dir_all(home.path().join("data/talk")).unwrap();
        std::fs::write(
            home.path().join("data/talk/standups.json"),
            json!({"shop": {"at": 9, "text": "2 tasks done."}}).to_string(),
        )
        .unwrap();
        let reopened = studio_with_home(home.path());
        assert_eq!(reopened.standups()["shop"].text, "2 tasks done.");
    }

    fn studio_with_home(home: &std::path::Path) -> Arc<Studio> {
        let paths = StudioPaths::resolve(Some(home.to_path_buf()));
        Studio::open(paths, HashMap::new()).unwrap()
    }

    #[test]
    fn results_are_given_for_done_ready_and_shipped_tasks_only() {
        let (_home, studio) = studio_with(true, &FakeRuntime::default());
        let ticket = |id: &str, state: TicketState| -> Ticket {
            let mut ticket: Ticket = serde_json::from_value(json!({
                "id": id, "project": "shop", "title": "t", "goal": "", "hire": "dev",
                "state": "queued", "attempts": 0, "base_branch": "master", "created_at": 0,
            }))
            .unwrap();
            ticket.state = state;
            ticket
        };
        let tickets = [
            ticket("SHOP-1", TicketState::Ready),
            ticket("SHOP-2", TicketState::Landed),
            ticket("SHOP-3", TicketState::Working),
            ticket("SHOP-4", TicketState::Landed),
        ];
        let log = |id: &str, summary: &str| {
            let line = json!({"at": 1, "event": {"type": "finished",
                "outcome": {"outcome": "success"}, "summary": summary}});
            let runs = studio.paths.data_dir.join("runs");
            std::fs::create_dir_all(&runs).unwrap();
            std::fs::write(runs.join(format!("{id}.jsonl")), format!("{line}\n")).unwrap();
        };
        log("SHOP-1", "Added the **cart**. More.");
        log("SHOP-2", "## Done\n\nFixed the total!");
        log("SHOP-3", "Still going.");
        // SHOP-4 ended without any words.
        let results = studio.results(&tickets);
        assert_eq!(
            results.into_iter().collect::<Vec<_>>(),
            [
                ("SHOP-1".to_string(), "Added the cart.".to_string()),
                ("SHOP-2".to_string(), "Fixed the total!".to_string()),
            ]
        );
    }
}
