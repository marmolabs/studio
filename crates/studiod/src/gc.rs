//! `studio gc`: clears the worktrees that finished tickets leave, and the
//! branches of landed ones.

use crate::studio::StudioError;
use crate::worker::branch_slug;
use studio_core::ticket::{Ticket, TicketState};
use studio_git::Repo;

/// Cleans one project's repo and says what it removed. Worktrees of tickets
/// that are still open, held, ready or failed are never touched.
pub(crate) fn collect(repo: &Repo, tickets: &[Ticket]) -> Result<Vec<String>, StudioError> {
    let mut removed = Vec::new();
    let worktrees = repo.studio_worktrees()?;
    for ticket in tickets {
        let landed = ticket.state == TicketState::Landed;
        if !landed && ticket.state != TicketState::Cancelled {
            continue;
        }
        let branch = format!("studio/{}", branch_slug(&ticket.id.to_string()));
        if let Some(worktree) = worktrees.iter().find(|w| w.branch == branch) {
            repo.remove_worktree(worktree, true)?;
            removed.push(format!(
                "{}: removed worktree {}",
                ticket.id,
                worktree.path.display()
            ));
        }
        // A cancelled ticket's commits stay on its branch for the owner.
        if !landed || !repo.branch_exists(&branch)? {
            continue;
        }
        // A landed ticket's branch goes only once its goal branch holds it.
        let goal = if ticket.goal.trim().is_empty() {
            ticket.id.to_string()
        } else {
            ticket.goal.clone()
        };
        let goal_branch = Repo::goal_branch_name(&goal);
        if repo.delete_branch(&branch, Some(&goal_branch))? {
            removed.push(format!("{}: deleted branch {branch}", ticket.id));
        }
    }
    for worktree in &worktrees {
        let known = tickets
            .iter()
            .any(|t| worktree.branch == format!("studio/{}", branch_slug(&t.id.to_string())));
        // Nothing records whose work this is, so uncommitted changes stop it.
        if !known {
            let line = match repo.remove_worktree(worktree, false) {
                Ok(()) => "removed",
                Err(_) => "kept (it has uncommitted changes)",
            };
            removed.push(format!(
                "no ticket: {line} worktree {}",
                worktree.path.display()
            ));
        }
    }
    repo.prune()?;
    Ok(removed)
}
