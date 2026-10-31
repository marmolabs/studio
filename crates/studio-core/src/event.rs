use crate::ids::{AskId, HireId, ProjectId, RunId, TicketId};
use crate::ticket::{FailureClass, HeldReason, TicketKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Created {
        id: TicketId,
        project: ProjectId,
        title: String,
        goal: String,
        hire: HireId,
        base_branch: String,
        created_at: u64,
        #[serde(default)]
        kind: TicketKind,
        #[serde(default)]
        parent: Option<TicketId>,
        #[serde(default)]
        detail: String,
        #[serde(default)]
        source: Option<String>,
    },
    Claimed,
    RunStarted {
        run: RunId,
    },
    AskOpened {
        ask: AskId,
    },
    AskResolved {
        ask: AskId,
        approved: bool,
    },
    ChecksStarted,
    ChecksPassed,
    ChecksFailed {
        detail: String,
    },
    ReviewPassed,
    ReviewFailed {
        detail: String,
    },
    Held {
        reason: HeldReason,
    },
    Resumed,
    Landed,
    Cancelled,
    Failed {
        class: FailureClass,
        detail: String,
    },
    Retried,
    ChangesRequested {
        detail: String,
    },
    /// Landing found the goal branch had moved on and conflicts with the
    /// ticket in `files`; the branch is merged into the ticket's worktree and
    /// the ticket goes back to its agent to settle them.
    Conflicted {
        branch: String,
        files: Vec<String>,
    },
}

impl Event {
    /// The event's `type` as it is stored, such as `"checks_started"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Created { .. } => "created",
            Event::Claimed => "claimed",
            Event::RunStarted { .. } => "run_started",
            Event::AskOpened { .. } => "ask_opened",
            Event::AskResolved { .. } => "ask_resolved",
            Event::ChecksStarted => "checks_started",
            Event::ChecksPassed => "checks_passed",
            Event::ChecksFailed { .. } => "checks_failed",
            Event::ReviewPassed => "review_passed",
            Event::ReviewFailed { .. } => "review_failed",
            Event::Held { .. } => "held",
            Event::Resumed => "resumed",
            Event::Landed => "landed",
            Event::Cancelled => "cancelled",
            Event::Failed { .. } => "failed",
            Event::Retried => "retried",
            Event::ChangesRequested { .. } => "changes_requested",
            Event::Conflicted { .. } => "conflicted",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn kind_is_the_stored_type() {
        let events = [
            Event::Claimed,
            Event::AskResolved {
                ask: AskId::new("a1"),
                approved: false,
            },
            Event::ChecksStarted,
            Event::ReviewFailed { detail: "x".into() },
            Event::Held {
                reason: HeldReason::Budget,
            },
            Event::ChangesRequested { detail: "x".into() },
            Event::Conflicted {
                branch: "studio/goal/x".into(),
                files: vec!["a.rs".into()],
            },
        ];
        for event in events {
            let stored = serde_json::to_value(&event).unwrap();
            assert_eq!(stored["type"], event.kind());
        }
    }
}
