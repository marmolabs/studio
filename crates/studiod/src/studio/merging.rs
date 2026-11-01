//! Landing a ready ticket on its goal branch, and merging a goal into its base.

use super::tickets::NewTicket;
use super::{Studio, StudioError, lock, other};
use crate::api::Response;
use crate::worker;
use serde_json::json;
use std::collections::HashMap;
use studio_core::event::Event;
use studio_core::ids::TicketId;
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_git::{LandOutcome, MergeOutcome, Repo};

/// What landing a ready ticket did.
pub(crate) enum Landing {
    Landed {
        ticket: Box<Ticket>,
        branch: String,
        sha: String,
    },
    /// The goal branch conflicts with the ticket in `files`; the ticket went
    /// back to its agent with the branch merged into its worktree.
    SentBack { branch: String, files: Vec<String> },
}

/// Where a ticket filed to merge a branch into its goal comes from, followed
/// by the branch: `merge:master`.
pub(crate) const MERGE_SOURCE: &str = "merge:";

/// The goal a ticket lands under: its goal, or its own id when it has none.
pub(crate) fn goal_of(ticket: &Ticket) -> String {
    if ticket.goal.trim().is_empty() {
        ticket.id.to_string()
    } else {
        ticket.goal.clone()
    }
}

impl Studio {
    /// Merges a ready ticket into its goal branch and removes its worktree.
    /// If the goal branch has moved on and conflicts with it, the branch is
    /// merged into the ticket's worktree instead, and the ticket goes back to
    /// its agent to resolve the conflicts.
    pub(crate) fn land(&self, id: &TicketId) -> Result<Landing, StudioError> {
        let ticket = self.ticket(id)?;
        if ticket.state != TicketState::Ready {
            return Err(other(format!("{id} is {}, not ready", ticket.state.kind())));
        }
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let worktree = worker::find_worktree(&repo, id)?
            .ok_or_else(|| other(format!("{id} has no worktree")))?;
        let branch = repo.ensure_goal_branch(&goal_of(&ticket), &ticket.base_branch)?;
        match repo.land(&worktree, &branch)? {
            LandOutcome::Merged { sha } => {
                let ticket = self.append(id, Event::Landed)?;
                // The worktree goes, so its evidence moves to the data folder;
                // if that fails, the worktree stays for `studio gc`. Anything
                // else left in it came from checks after the last commit.
                let log = crate::runlog::RunLog::new(&self.paths.data_dir, &id.to_string());
                match crate::evidence::keep(&worktree.path, &self.paths.data_dir, &id.to_string()) {
                    Ok(_) => {
                        let _ = repo.remove_worktree(&worktree, true);
                    }
                    Err(err) => log.note(&format!(
                        "kept the worktree: its evidence couldn't be copied: {err}"
                    )),
                }
                Ok(Landing::Landed {
                    ticket: Box::new(ticket),
                    branch,
                    sha,
                })
            }
            LandOutcome::Conflict { files } => {
                let mut worktree = worktree;
                worktree.merge_in(&branch)?;
                // Review then reads only what the ticket adds to the branch.
                worktree.set_base(&branch)?;
                self.append(
                    id,
                    Event::Conflicted {
                        branch: branch.clone(),
                        files: files.clone(),
                    },
                )?;
                Ok(Landing::SentBack { branch, files })
            }
        }
    }

    /// Landed work tickets whose goal branch is already in their base branch.
    /// Open pages ask every second, so git reads the tips once per project and
    /// is only asked about a pair of tips it hasn't answered for yet.
    pub(super) fn merged_tickets(&self) -> Result<Vec<String>, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        Ok(self.merged_among(&tickets))
    }

    /// `merged_tickets` among `tickets`, which the caller has read.
    pub(super) fn merged_among(&self, tickets: &[Ticket]) -> Vec<String> {
        let known = std::mem::take(&mut *lock(&self.merged_tips));
        let mut seen: HashMap<(String, String), bool> = HashMap::new();
        let mut projects = HashMap::new();
        let mut merged = Vec::new();
        for ticket in tickets {
            if ticket.state != TicketState::Landed || ticket.kind == TicketKind::Plan {
                continue;
            }
            let project = projects.entry(ticket.project.clone()).or_insert_with(|| {
                let repo = Repo::open(&self.project(&ticket.project).ok()?.path).ok()?;
                let tips = repo.branch_tips().ok()?;
                Some((repo, tips))
            });
            let Some((repo, tips)) = project else {
                continue;
            };
            let branch = Repo::goal_branch_name(&goal_of(ticket));
            let Some(tip) = tips.get(&branch) else {
                continue;
            };
            let is_merged = match tips.get(&ticket.base_branch) {
                Some(base) => {
                    let pair = (tip.clone(), base.clone());
                    let held = known.get(&pair).copied();
                    *seen
                        .entry(pair)
                        .or_insert_with(|| held.unwrap_or_else(|| repo.is_merged(tip, base)))
                }
                // A tag or a sha has no tip to remember it by.
                None => repo.is_merged(tip, &ticket.base_branch),
            };
            if is_merged {
                merged.push(ticket.id.to_string());
            }
        }
        *lock(&self.merged_tips) = seen;
        merged
    }

    /// A ticket on the goal of `landed` that merges `into` into the goal's
    /// branch and resolves the conflicts, so the goal then merges cleanly.
    /// One that is already open is reused.
    fn merge_ticket(
        &self,
        landed: &Ticket,
        into: &str,
        files: &[String],
    ) -> Result<Ticket, StudioError> {
        let goal = goal_of(landed);
        let source = format!("{MERGE_SOURCE}{into}");
        let open = lock(&self.store).tickets()?.into_iter().find(|t| {
            t.project == landed.project
                && goal_of(t) == goal
                && t.source.as_deref() == Some(source.as_str())
                && !matches!(
                    t.state,
                    TicketState::Landed | TicketState::Cancelled | TicketState::Failed { .. }
                )
        });
        if let Some(open) = open {
            return Ok(open);
        }
        // Whoever did the goal's work knows it best.
        let hire = match self.hire(&landed.hire) {
            Ok(hire) if landed.kind == TicketKind::Work => hire,
            _ => self
                .first_with_role("engineer")
                .ok_or_else(|| other("no hire has the engineer role"))?,
        };
        self.create_ticket(NewTicket {
            project: landed.project.0.clone(),
            title: format!("Merge {into} into the goal and resolve the conflicts"),
            goal,
            hire: hire.id.0,
            base: Some(landed.base_branch.clone()),
            kind: TicketKind::Work,
            parent: None,
            detail: format!(
                "Merging the goal into {into} conflicts in: {}. The studio merges {into} \
                 into this ticket's worktree; once this lands, the goal merges cleanly.",
                files.join(", ")
            ),
            source: Some(source),
        })
    }

    /// Merges the goal branch a landed ticket is on into the branch its work
    /// started from. Tickets of the goal still open come along later.
    pub(super) fn merge(&self, id: &TicketId) -> Result<Response, StudioError> {
        let ticket = self.ticket(id)?;
        if ticket.state != TicketState::Landed {
            return Err(other(format!(
                "{id} is {}; land it first",
                ticket.state.kind()
            )));
        }
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let goal = goal_of(&ticket);
        let branch = Repo::goal_branch_name(&goal);
        let into = ticket.base_branch.clone();
        let open: Vec<String> = lock(&self.store)
            .tickets()?
            .into_iter()
            .filter(|t| t.project == ticket.project && goal_of(t) == goal)
            .filter(|t| !matches!(t.state, TicketState::Landed | TicketState::Cancelled))
            .filter(|t| t.kind == TicketKind::Work)
            .map(|t| t.id.to_string())
            .collect();
        // Only a studio with no merged goal yet can tell when its first was.
        let first = !self.first_merge_path().exists()
            && self.merged_tickets().is_ok_and(|merged| merged.is_empty());
        match repo.merge_goal(&branch, &into)? {
            MergeOutcome::Merged { sha } => {
                if first {
                    self.remember_first_merge(&goal, &into);
                }
                Ok(Response::ok(
                    json!({ "branch": branch, "into": into, "sha": sha, "open": open }),
                ))
            }
            MergeOutcome::Conflict { files } => {
                let fix = self.merge_ticket(&ticket, &into, &files)?;
                Err(other(format!(
                    "{branch} conflicts with {into} in: {}; {} merges {into} into the goal \
                     and resolves them, then merge again",
                    files.join(", "),
                    fix.id
                )))
            }
            MergeOutcome::Dirty { path } => Err(other(format!(
                "{into} is checked out at {} with uncommitted changes; commit or stash them first",
                path.display()
            ))),
        }
    }
}
