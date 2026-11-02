//! Telling a task's agent something: a message from the owner or a lead to
//! the work in hand.
//!
//! How the message gets there depends on where the task is. A task whose agent
//! is working hears it at once: the Claude Code CLI takes a user message that
//! arrives while a turn runs into that turn, or answers it as a turn of its
//! own when it came too late for the current one (the runtime sees to it that
//! the run lasts until that turn has ended). A task that waits in checks,
//! review or ready can't hear anyone, so it goes back to its agent with the
//! message as a note; a queued or held one keeps the message and its agent
//! hears it first thing when it next starts.

use super::{StopReason, Studio, StudioError, file_stem, lock, other};
use crate::api::Response;
use crate::runlog::RunLog;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use studio_core::event::Event;
use studio_core::ids::TicketId;
use studio_core::ticket::TicketState;

/// Who a message is from when it comes from the owner.
const OWNER: &str = "the owner";

impl Studio {
    /// `Request::Tell`: gets `text` to the agent of task `task`. A running
    /// task hears it in its live session, a task that waits in checks, review
    /// or ready goes back with it as a note, a queued one keeps it for its next
    /// run, and a finished one is an error. Whichever it was goes in the run
    /// log as a note, "Message from <who>: <text>". Answers `{"task", "how"}`
    /// where `how` is `live`, `sent_back` or `queued`.
    pub(super) fn tell(self: &Arc<Self>, task: &str, text: &str) -> Result<Response, StudioError> {
        self.tell_from(task, text, OWNER)
    }

    /// `tell`, from `who` (the owner, or a lead by name).
    pub(crate) fn tell_from(
        &self,
        task: &str,
        text: &str,
        who: &str,
    ) -> Result<Response, StudioError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(other("there is nothing to tell"));
        }
        let id = TicketId::new(task);
        let ticket = self.ticket(&id)?;
        let said = format!("Message from {who}: {text}");
        let how = match ticket.state {
            TicketState::Claimed | TicketState::Working | TicketState::Asking { .. } => {
                let stop = lock(&self.active).get(task).cloned().ok_or_else(|| {
                    other(format!(
                        "{task} has no agent running to hear it; try again in a moment"
                    ))
                })?;
                stop.say(said.clone());
                "live"
            }
            TicketState::Checking | TicketState::Reviewing | TicketState::Ready => {
                self.append(
                    &id,
                    Event::ChangesRequested {
                        detail: text.to_string(),
                    },
                )?;
                // The review that was under way is of work that changes now.
                if let Some(stop) = lock(&self.active).get(task) {
                    stop.request(StopReason::SentBack);
                }
                self.wake.notify_one();
                "sent_back"
            }
            TicketState::Queued | TicketState::Held { .. } => {
                self.keep(task, &said)?;
                "queued"
            }
            state => {
                return Err(other(format!(
                    "{task} is {}: its agent has finished, so there is no one to tell",
                    state.kind()
                )));
            }
        };
        RunLog::new(&self.paths.data_dir, task).note(&said);
        Ok(Response::ok(json!({ "task": task, "how": how })))
    }

    /// Where the messages kept for a ticket's agent are.
    fn kept_path(&self, ticket: &str) -> PathBuf {
        self.paths
            .data_dir
            .join("talk")
            .join("told")
            .join(format!("{}.json", file_stem(ticket)))
    }

    /// Keeps `said` for the ticket's agent, after those kept before it.
    fn keep(&self, ticket: &str, said: &str) -> Result<(), StudioError> {
        let path = self.kept_path(ticket);
        let mut kept = self.read_kept(&path);
        kept.push(said.to_string());
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| other(format!("could not keep it: {e}")))?;
        }
        let text = serde_json::to_string(&kept).map_err(|e| other(e.to_string()))?;
        fs::write(&path, text).map_err(|e| other(format!("could not keep it: {e}")))
    }

    /// The messages kept for the ticket's agent, oldest first, which are
    /// forgotten once taken.
    pub(super) fn take_kept(&self, ticket: &str) -> Vec<String> {
        let path = self.kept_path(ticket);
        let kept = self.read_kept(&path);
        let _ = fs::remove_file(&path);
        kept
    }

    fn read_kept(&self, path: &std::path::Path) -> Vec<String> {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::super::NewTicket;
    use super::*;
    use std::collections::HashMap;
    use studio_config::StudioPaths;
    use studio_core::ticket::TicketKind;
    use studio_runtime::Runtime;

    /// A studio with the project `shop` and the hire `dev`, and one queued task.
    fn studio_with_a_task() -> (tempfile::TempDir, Arc<Studio>) {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\n",
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
        let runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        let studio = Studio::open(paths, runtimes).unwrap();
        studio
            .create_ticket(NewTicket {
                project: "shop".into(),
                title: "Add a cart drawer".into(),
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
        (home, studio)
    }

    #[test]
    fn a_message_is_kept_with_who_it_is_from_and_heard_once() {
        let (_home, studio) = studio_with_a_task();
        let told = studio
            .tell_from("SHOP-1", "Keep it small.", "Nora")
            .unwrap();
        assert_eq!(told.data["how"], "queued");
        studio.tell("SHOP-1", "Use three colours.").unwrap();
        assert_eq!(
            studio.take_kept("SHOP-1"),
            [
                "Message from Nora: Keep it small.",
                "Message from the owner: Use three colours."
            ]
        );
        assert!(studio.take_kept("SHOP-1").is_empty());
        assert!(studio.take_kept("SHOP-2").is_empty());
        // The run log has both, in the order they came.
        let log = std::fs::read_to_string(studio.paths.data_dir.join("runs/SHOP-1.jsonl")).unwrap();
        assert!(
            log.find("Message from Nora").unwrap() < log.find("Message from the owner").unwrap()
        );
    }

    #[test]
    fn a_task_nobody_has_or_a_blank_message_is_an_error() {
        let (_home, studio) = studio_with_a_task();
        let blank = studio.tell("SHOP-1", " \n ").err().unwrap();
        assert_eq!(blank.to_string(), "there is nothing to tell");
        assert!(studio.tell("SHOP-7", "Hello").is_err());
        assert!(studio.take_kept("SHOP-1").is_empty());
    }

    #[test]
    fn the_name_of_a_task_never_leads_out_of_the_folder() {
        let (_home, studio) = studio_with_a_task();
        let path = studio.kept_path("../../outside");
        assert!(path.starts_with(studio.paths.data_dir.join("talk/told")));
        assert_eq!(path.file_name().unwrap(), "______outside.json");
    }
}
