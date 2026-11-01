//! Tickets: making and filing them, a ticket's change, and the CI tickets a watcher files.

use super::{StopReason, Studio, StudioError, lock, other};
use crate::api::Response;
use crate::watch::CiWatcher;
use crate::{gc, now_ms, worker};
use serde_json::json;
use std::sync::Arc;
use studio_config::config::ProjectEntry;
use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, TicketId};
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_git::Repo;

pub(super) struct NewTicket {
    pub(super) project: String,
    pub(super) title: String,
    pub(super) goal: String,
    pub(super) hire: String,
    pub(super) base: Option<String>,
    pub(super) kind: TicketKind,
    pub(super) parent: Option<TicketId>,
    pub(super) detail: String,
    pub(super) source: Option<String>,
}

/// At most `max` characters of the first line, with an ellipsis if cut.
pub(super) fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

impl Studio {
    pub(super) fn create_ticket(&self, new: NewTicket) -> Result<Ticket, StudioError> {
        let entry = self.project(&ProjectId::new(&new.project))?;
        self.hire(&HireId::new(&new.hire))?;
        let base = new
            .base
            .or_else(|| entry.integration_branch.clone())
            .unwrap_or_else(|| "HEAD".to_string());
        let id = self.next_ticket_id(&new.project)?;
        let ticket = self.append(
            &id,
            Event::Created {
                id: id.clone(),
                project: ProjectId::new(&new.project),
                title: new.title,
                goal: new.goal,
                hire: HireId::new(&new.hire),
                base_branch: base,
                created_at: now_ms(),
                kind: new.kind,
                parent: new.parent,
                detail: new.detail,
                source: new.source,
            },
        )?;
        self.wake.notify_one();
        Ok(ticket)
    }

    /// A ticket filed by another ticket's agent. It lands on the parent's goal
    /// branch, and each parent may file only so many.
    pub(super) fn file_ticket(
        &self,
        parent: &TicketId,
        title: String,
        detail: String,
        hire: Option<String>,
    ) -> Result<Ticket, StudioError> {
        let parent = self.ticket(parent)?;
        if !self
            .hire(&parent.hire)?
            .tools
            .iter()
            .any(|t| t == "file_ticket")
        {
            return Err(other(format!(
                "{} may not file tickets; its hire doesn't list file_ticket in tools",
                parent.hire
            )));
        }
        if parent.state != TicketState::Working {
            return Err(other(format!(
                "{} is {}; only a working ticket may file tickets",
                parent.id,
                parent.state.kind()
            )));
        }
        let max = self.config().goals.max_filed as usize;
        let filed = lock(&self.store)
            .tickets()?
            .iter()
            .filter(|t| t.parent.as_ref() == Some(&parent.id))
            .count();
        if filed >= max {
            return Err(other(format!(
                "{} has already filed {filed} tickets, the most it may file ({max}); \
                 finish with a summary instead",
                parent.id
            )));
        }
        let hire = match hire {
            Some(hire) => hire,
            None => {
                self.first_with_role("engineer")
                    .ok_or_else(|| other("no hire has the engineer role; pass hire"))?
                    .id
                    .0
            }
        };
        self.create_ticket(NewTicket {
            project: parent.project.0.clone(),
            title,
            goal: parent.goal.clone(),
            hire,
            base: Some(parent.base_branch.clone()),
            kind: TicketKind::Work,
            parent: Some(parent.id.clone()),
            detail,
            source: None,
        })
    }

    /// Stops the agent of a running ticket; its worker then holds it.
    pub(super) fn pause(&self, ticket: &str) -> Result<Response, StudioError> {
        let state = self.ticket(&TicketId::new(ticket))?.state;
        let running = matches!(
            state,
            TicketState::Claimed | TicketState::Working | TicketState::Reviewing
        );
        match lock(&self.active).get(ticket) {
            Some(stop) if running => {
                stop.request(StopReason::Pause);
                Ok(Response::ok(json!({ "ticket": ticket })))
            }
            _ => Err(other(format!("{ticket} has no agent running to pause"))),
        }
    }

    pub(super) fn simple(&self, ticket: &str, event: Event) -> Result<Response, StudioError> {
        let updated = self.append(&TicketId::new(ticket), event)?;
        self.wake.notify_one();
        Ok(Response::ok(updated))
    }

    fn next_ticket_id(&self, project: &str) -> Result<TicketId, StudioError> {
        let prefix: String = project
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(4)
            .collect::<String>()
            .to_ascii_uppercase();
        let prefix = if prefix.is_empty() {
            "T".to_string()
        } else {
            prefix
        };
        let count = lock(&self.store)
            .tickets()?
            .iter()
            .filter(|t| t.project.0 == project)
            .count();
        Ok(TicketId::new(format!("{prefix}-{}", count + 1)))
    }

    /// `git diff --stat` of a ticket's worktree, or `None` when it is gone.
    pub(super) fn stat(&self, id: &TicketId) -> Result<Option<String>, StudioError> {
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        Ok(match worker::find_worktree(&repo, id)? {
            Some(worktree) => Some(worktree.diff_stat()?),
            None => None,
        })
    }

    pub(super) fn diff(&self, id: &TicketId) -> Result<Response, StudioError> {
        const MAX_DIFF: usize = 1024 * 1024;
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let diff = match worker::find_worktree(&repo, id)? {
            Some(worktree) => {
                let mut diff = worktree.diff()?;
                if diff.len() > MAX_DIFF {
                    let cut = diff.floor_char_boundary(MAX_DIFF);
                    diff.truncate(cut);
                    diff.push_str("\n… (diff cut at 1 MB)\n");
                }
                Some(diff)
            }
            None => None,
        };
        Ok(Response::ok(diff))
    }

    pub(super) fn evidence(&self, id: &TicketId) -> Result<Response, StudioError> {
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let files = match worker::find_worktree(&repo, id)? {
            Some(worktree) => crate::evidence::all(&worktree.path),
            None => crate::evidence::list_kept(&self.paths.data_dir, &id.to_string()),
        };
        Ok(Response::ok(files))
    }

    pub(super) fn gc(&self) -> Result<Response, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let mut removed = Vec::new();
        for project in &self.config().projects {
            let mine: Vec<Ticket> = tickets
                .iter()
                .filter(|t| t.project.0 == project.name)
                .cloned()
                .collect();
            let repo = match Repo::open(&project.path) {
                Ok(repo) => repo,
                Err(err) => {
                    removed.push(format!("{}: skipped, {err}", project.name));
                    continue;
                }
            };
            removed.extend(gc::collect(&repo, &mine)?);
        }
        Ok(Response::ok(removed))
    }

    /// Files a ticket for the first engineer, remembering where it came from.
    pub(crate) fn file_ci_ticket(
        &self,
        project: &str,
        title: String,
        detail: String,
        source: String,
    ) -> Result<Ticket, StudioError> {
        let hire = self
            .first_with_role("engineer")
            .ok_or_else(|| other("no hire has the engineer role"))?;
        self.create_ticket(NewTicket {
            project: project.to_string(),
            title,
            goal: String::new(),
            hire: hire.id.0,
            base: None,
            kind: TicketKind::Work,
            parent: None,
            detail,
            source: Some(source),
        })
    }

    /// Tickets of the project that the CI watcher filed.
    pub(crate) fn ci_tickets(&self, project: &str) -> Result<Vec<Ticket>, StudioError> {
        Ok(lock(&self.store)
            .tickets()?
            .into_iter()
            .filter(|t| t.project.0 == project)
            .filter(|t| t.source.as_deref().is_some_and(|s| s.starts_with("ci:")))
            .collect())
    }

    /// Checks CI for the watched projects now and then, until the process ends.
    /// Returns None when no project asks for it.
    pub fn spawn_ci_watcher(
        self: &Arc<Self>,
        watcher: CiWatcher,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if !self.config().projects.iter().any(|p| p.watch_ci) {
            return None;
        }
        let studio = Arc::clone(self);
        let every = self.config().watchers.ci_interval();
        Some(tokio::spawn(async move {
            loop {
                watcher.tick(&studio).await;
                tokio::time::sleep(every).await;
            }
        }))
    }

    pub(crate) fn watched_projects(&self) -> Vec<ProjectEntry> {
        self.config()
            .projects
            .iter()
            .filter(|p| p.watch_ci)
            .cloned()
            .collect()
    }
}
