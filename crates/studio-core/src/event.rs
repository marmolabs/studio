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
}
