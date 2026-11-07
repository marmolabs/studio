use crate::ids::{AskId, HireId, ProjectId, RunId, TicketId};
use crate::ticket::{DEFAULT_PRIORITY, FailureClass, HeldReason, TicketKind};
use serde::{Deserialize, Serialize};

fn default_created_priority() -> u32 {
    DEFAULT_PRIORITY
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// `Created` is one event per ticket and the rest are small and many; boxing
// its fields would only complicate every place that reads them.
#[allow(clippy::large_enum_variant)]
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
        /// On a goal's plan ticket: the version the goal is a milestone
        /// towards.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default = "default_created_priority")]
        priority: u32,
        #[serde(default)]
        depends_on: Vec<TicketId>,
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
    /// Changes scheduling without interrupting an active run.
    PriorityChanged {
        priority: u32,
    },
    /// Replaces the tickets that must land before this one starts.
    DependenciesChanged {
        depends_on: Vec<TicketId>,
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
            Event::PriorityChanged { .. } => "priority_changed",
            Event::DependenciesChanged { .. } => "dependencies_changed",
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
            Event::PriorityChanged { priority: 100 },
            Event::DependenciesChanged {
                depends_on: vec![TicketId::new("T-1")],
            },
        ];
        for event in events {
            let stored = serde_json::to_value(&event).unwrap();
            assert_eq!(stored["type"], event.kind());
        }
    }
}
