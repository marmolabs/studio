//! What a lead is told when it takes a turn: a short brief, how its project
//! stands, the last words of the conversation, and what the owner just said.
//!
//! It is plain text, written for the lead to read. The engine that runs the
//! turn is in `talk`.

use super::{Studio, StudioError, lock};
use crate::api::{TalkCard, TalkFrom, TalkMessage};
use crate::budget::DAY_MS;
use crate::now_ms;
use std::collections::BTreeSet;
use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId};
use studio_core::rule::Hire;
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_git::Repo;

/// How many earlier messages of the conversation the lead is shown.
const HISTORY: usize = 20;

/// The most goals, and tasks in a goal, the lead is told about.
const GOALS: usize = 8;
const TASKS: usize = 12;

/// How a lead words what it says to the owner, in a conversation and when it
/// sets a project up: in plain words, in the first person, in at most three
/// short sentences of about 15 words each, about `about`, one thought to a
/// sentence. It reads on from "Reply", or from "reply to the owner".
pub(super) fn reply_rules(about: &str) -> String {
    format!(
        "in plain words, in the first person, in at most three short sentences of about 15 \
         words each, about {about}. One thought to a sentence: no semicolons and no lists."
    )
}

/// What a lead is told about how to answer: short, in the first person, about
/// what the owner asked, and without the figures the page already shows.
fn brief(project: &str) -> String {
    let reply = reply_rules("what the owner asked");
    format!(
        "You lead {project} for the owner. Reply {reply} Never speak of yourself as \"the \
         lead\" or in the third person. Don't repeat figures the page already shows, such as \
         what was spent or how many tasks there are, unless the owner asks for them. Use your \
         tools when the owner asks you to act, and say what you did in one line: a plan or an \
         action has a card of its own that carries the detail. Name tasks by their title, not \
         their id."
    )
}

impl Studio {
    /// The prompt of a turn of `project`'s lead: the brief, the project's
    /// state, the last `HISTORY` messages of `before` and then `new`, the
    /// owner's messages it has not answered yet.
    pub(super) fn talk_prompt(
        &self,
        project: &str,
        before: &[TalkMessage],
        new: &[TalkMessage],
    ) -> String {
        let state = self
            .project_state(project)
            .unwrap_or_else(|e| format!("(How the project stands can't be read now: {e}.)"));
        let earlier = &before[before.len().saturating_sub(HISTORY)..];
        let mut prompt = format!("{}\n\n## How {project} stands\n{state}\n", brief(project));
        if !earlier.is_empty() {
            prompt.push_str("\n## The conversation so far\n");
            for message in earlier {
                prompt.push_str(&format!(
                    "{}: {}\n",
                    self.speaker(message),
                    line_of(message)
                ));
            }
        }
        prompt.push_str(if new.len() == 1 {
            "\n## The owner's new message\n"
        } else {
            "\n## The owner's new messages\n"
        });
        for message in new {
            prompt.push_str(&message.text);
            prompt.push('\n');
        }
        prompt
    }

    /// Who said a message, as the lead reads it.
    fn speaker(&self, message: &TalkMessage) -> String {
        match &message.from {
            TalkFrom::Owner => "Owner".to_string(),
            TalkFrom::Studio => "Studio".to_string(),
            TalkFrom::Lead { hire } => self
                .hire(&HireId::new(hire))
                .map(|h| named(&h))
                .unwrap_or_else(|_| hire.clone()),
        }
    }

    /// How `project` stands now: its goals and tasks, what waits for the
    /// owner, the release, what got done today and what it has spent.
    pub(super) fn project_state(&self, project: &str) -> Result<String, StudioError> {
        let entry = self.project(&ProjectId::new(project))?;
        let now = now_ms();
        let since = now.saturating_sub(DAY_MS);
        let (tickets, landed, spent) = {
            let store = lock(&self.store);
            (
                store.tickets()?,
                store.events_from(since, usize::MAX, |e| matches!(e, Event::Landed))?,
                store.spent_since(since)?,
            )
        };
        let mine: Vec<Ticket> = tickets
            .into_iter()
            .filter(|t| t.project.0 == project)
            .collect();
        let title_of = |id: &str| {
            mine.iter()
                .find(|t| t.id.0 == id)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| id.to_string())
        };
        let mut lines = Vec::new();

        let asks: Vec<_> = lock(&self.asks)
            .values()
            .map(|a| a.view.clone())
            .filter(|a| mine.iter().any(|t| t.id.0 == a.ticket))
            .collect();
        let ready: Vec<&Ticket> = mine
            .iter()
            .filter(|t| t.kind == TicketKind::Work && t.state == TicketState::Ready)
            .collect();
        if !asks.is_empty() || !ready.is_empty() {
            lines.push("Waiting for the owner:".to_string());
            for ask in &asks {
                let what = if ask.title.trim().is_empty() {
                    format!("a {} request ({})", ask.kind, ask.tool)
                } else {
                    ask.title.trim().to_string()
                };
                lines.push(format!("- {what} (on \"{}\")", title_of(&ask.ticket)));
            }
            for task in &ready {
                lines.push(format!(
                    "- \"{}\" ({}) is ready for review",
                    task.title, task.id
                ));
            }
        }

        let goals = self.open_goals(&entry.path, &mine);
        if goals.is_empty() {
            lines.push("No goal is open.".to_string());
        }
        lines.extend(goals);
        let single: Vec<&Ticket> = mine
            .iter()
            .filter(|t| t.kind == TicketKind::Work && t.parent.is_none())
            .filter(|t| !matches!(t.state, TicketState::Landed | TicketState::Cancelled))
            .take(TASKS)
            .collect();
        if !single.is_empty() {
            lines.push("Tasks that belong to no goal:".to_string());
            for task in single {
                lines.push(format!(
                    "- \"{}\" ({}): {}",
                    task.title,
                    task.id,
                    state_words(&task.state)
                ));
            }
        }

        let (_, release, last) = self.release_fields(&entry);
        if let Some(release) = release {
            lines.push(format!("Release {}: {:?}.", release.version, release.state));
        }
        if let Some(last) = last {
            lines.push(format!("Last release published: {}.", last.version));
        }

        let results = self.results(&mine);
        let mut seen = BTreeSet::new();
        let done: Vec<String> = landed
            .iter()
            .filter_map(|e| mine.iter().find(|t| t.id == e.ticket_id))
            .filter(|t| t.kind == TicketKind::Work && seen.insert(t.id.0.clone()))
            .map(|t| match results.get(&t.id.0) {
                Some(result) => format!("- \"{}\": {result}", t.title),
                None => format!("- \"{}\"", t.title),
            })
            .collect();
        if !done.is_empty() {
            lines.push("Done in the last day:".to_string());
            lines.extend(done);
        }

        let cap = entry
            .daily_usd
            .unwrap_or_else(|| self.config().budget.daily_usd);
        lines.push(format!(
            "Spent today: ${:.2} of ${cap:.2}.",
            spent.by_project.get(project).copied().unwrap_or(0.0)
        ));
        Ok(lines.join("\n"))
    }

    /// The lines on the goals of `mine` that are not finished: still being
    /// planned, with tasks under way, or done but not shipped.
    fn open_goals(&self, repo: &std::path::Path, mine: &[Ticket]) -> Vec<String> {
        let mut lines = Vec::new();
        let repo = Repo::open(repo).ok();
        let tips = repo.as_ref().and_then(|r| r.branch_tips().ok());
        let goals = mine
            .iter()
            .filter(|t| t.kind == TicketKind::Plan && t.state != TicketState::Cancelled);
        let mut shown = 0;
        for goal in goals {
            let tasks: Vec<&Ticket> = mine
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&goal.id))
                .filter(|t| t.state != TicketState::Cancelled)
                .collect();
            let planned = goal.state == TicketState::Landed;
            let done = tasks
                .iter()
                .filter(|t| t.state == TicketState::Landed)
                .count();
            let shipped = planned
                && !tasks.is_empty()
                && done == tasks.len()
                && match (&repo, &tips) {
                    (Some(repo), Some(tips)) => {
                        let branch = Repo::goal_branch_name(&super::goal_of(tasks[0]));
                        match (tips.get(&branch), tips.get(&tasks[0].base_branch)) {
                            (Some(tip), Some(base)) => repo.is_merged(tip, base),
                            // No branch: nothing is left to ship.
                            (None, _) => true,
                            (Some(_), None) => false,
                        }
                    }
                    _ => false,
                };
            if planned && (tasks.is_empty() || shipped) {
                continue;
            }
            let version = goal
                .version
                .as_deref()
                .map(|v| format!(", for {v}"))
                .unwrap_or_default();
            let status = if !planned {
                "being planned".to_string()
            } else if done == tasks.len() {
                format!("all {done} tasks are done, ready to ship")
            } else {
                format!("{done} of {} tasks done", tasks.len())
            };
            lines.push(format!(
                "Goal \"{}\" ({}{version}): {status}.",
                super::goal_of(goal),
                goal.id
            ));
            for task in tasks
                .iter()
                .filter(|t| t.state != TicketState::Landed)
                .take(TASKS)
            {
                lines.push(format!(
                    "- \"{}\" ({}): {}",
                    task.title,
                    task.id,
                    state_words(&task.state)
                ));
            }
            shown += 1;
            if shown >= GOALS {
                break;
            }
        }
        lines
    }
}

/// What to call a hire in a message: the name in its file, else its id.
pub(super) fn named(hire: &Hire) -> String {
    let name = hire.name.trim();
    if name.is_empty() {
        hire.id.0.clone()
    } else {
        name.to_string()
    }
}

/// A message as one line of the conversation: its words, and what its card
/// shows.
fn line_of(message: &TalkMessage) -> String {
    let words = message.text.trim().replace('\n', " ");
    match &message.card {
        Some(TalkCard::Plan { title, tasks, .. }) => {
            format!("{words} [plan card for \"{title}\", {} tasks]", tasks.len())
        }
        Some(TalkCard::Acted { title, action, .. }) => {
            format!("{words} [{action}: \"{title}\"]")
        }
        Some(TalkCard::Setup { .. }) => format!("{words} [setup card]"),
        None => words,
    }
}

/// A task's state in words for the owner's side.
fn state_words(state: &TicketState) -> String {
    match state {
        TicketState::Queued => "waiting to start".to_string(),
        TicketState::Claimed | TicketState::Working => "being worked on".to_string(),
        TicketState::Checking => "running its checks".to_string(),
        TicketState::Reviewing => "in review".to_string(),
        TicketState::Ready => "ready for the owner's review".to_string(),
        TicketState::Landed => "done".to_string(),
        TicketState::Cancelled => "cancelled".to_string(),
        TicketState::Asking { .. } => "waiting for an answer".to_string(),
        TicketState::Held { reason } => format!("held ({reason:?})").to_lowercase(),
        TicketState::Failed { detail, .. } => {
            let detail: String = detail.chars().take(80).collect();
            format!("failed: {}", detail.replace('\n', " "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_rules_are_three_short_sentences_in_the_first_person() {
        let rules = reply_rules("what you found");
        for rule in [
            "in plain words, in the first person",
            "at most three short sentences of about 15 words each",
            "about what you found.",
            "One thought to a sentence: no semicolons and no lists.",
        ] {
            assert!(rules.contains(rule), "{rule}: {rules}");
        }
    }

    #[test]
    fn the_brief_asks_for_a_short_reply_in_the_first_person() {
        let brief = brief("shop");
        assert!(brief.starts_with("You lead shop for the owner. Reply in plain words, "));
        // The reply is worded by the rules that every lead's reply follows.
        assert!(
            brief.contains(&reply_rules("what the owner asked")),
            "{brief}"
        );
        for rule in [
            "in the third person",
            "Don't repeat figures the page already shows",
            "unless the owner asks for them",
            "say what you did in one line",
            "has a card of its own that carries the detail",
            "by their title, not their id",
        ] {
            assert!(brief.contains(rule), "{rule}: {brief}");
        }
    }
}
