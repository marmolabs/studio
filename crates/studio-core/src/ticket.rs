use crate::ids::{AskId, HireId, ProjectId, TicketId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: TicketId,
    pub project: ProjectId,
    pub title: String,
    pub goal: String,
    pub hire: HireId,
    pub state: TicketState,
    pub attempts: u32,
    pub base_branch: String,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub held_from: Option<TicketState>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub feedback: Option<String>,
    #[serde(default)]
    pub kind: TicketKind,
    /// The ticket whose agent filed this one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent: Option<TicketId>,
    /// What the ticket asks for, beyond its title.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub detail: String,
    /// What made the studio file this ticket itself, e.g. `ci:<run id>`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
}

/// A plan ticket's run files tickets instead of changing code, so it skips
/// checks and review and lands without a merge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketKind {
    #[default]
    Work,
    Plan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    Queued,
    Claimed,
    Working,
    Checking,
    Reviewing,
    Ready,
    Landed,
    Cancelled,
    Asking { ask: AskId },
    Held { reason: HeldReason },
    Failed { class: FailureClass, detail: String },
}

impl TicketState {
    pub fn kind(&self) -> &'static str {
        match self {
            TicketState::Queued => "queued",
            TicketState::Claimed => "claimed",
            TicketState::Working => "working",
            TicketState::Checking => "checking",
            TicketState::Reviewing => "reviewing",
            TicketState::Ready => "ready",
            TicketState::Landed => "landed",
            TicketState::Cancelled => "cancelled",
            TicketState::Asking { .. } => "asking",
            TicketState::Held { .. } => "held",
            TicketState::Failed { .. } => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldReason {
    Budget,
    Quota,
    Auth,
    Conflict,
    Paused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Crash,
    Timeout,
    Checks,
    Review,
    Tooling,
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AskId;

    #[test]
    fn state_kind_snake_case() {
        assert_eq!(TicketState::Queued.kind(), "queued");
        assert_eq!(TicketState::Claimed.kind(), "claimed");
        assert_eq!(TicketState::Working.kind(), "working");
        assert_eq!(TicketState::Checking.kind(), "checking");
        assert_eq!(TicketState::Reviewing.kind(), "reviewing");
        assert_eq!(TicketState::Ready.kind(), "ready");
        assert_eq!(TicketState::Landed.kind(), "landed");
        assert_eq!(TicketState::Cancelled.kind(), "cancelled");
        assert_eq!(
            TicketState::Asking {
                ask: AskId::new("a")
            }
            .kind(),
            "asking"
        );
        assert_eq!(
            TicketState::Held {
                reason: HeldReason::Budget
            }
            .kind(),
            "held"
        );
        assert_eq!(
            TicketState::Failed {
                class: FailureClass::Crash,
                detail: "x".into()
            }
            .kind(),
            "failed"
        );
    }
}
