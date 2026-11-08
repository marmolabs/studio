use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio::sync::mpsc;

/// What a run is asked to do and where.
#[derive(Clone, Debug, Default)]
pub struct RunSpec {
    /// The ticket's worktree.
    pub cwd: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    /// Reasoning effort, as the runtime names it.
    pub effort: Option<String>,
    /// Session id of an earlier run to continue.
    pub resume: Option<String>,
    pub append_system_prompt: Option<String>,
    /// Replaces the runtime's own system prompt.
    pub system_prompt: Option<String>,
    /// A one-turn question: no tools, none of the owner's settings, notes or
    /// commands, nothing saved. Only the owner's login is kept.
    pub lean: bool,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    pub mcp_config: Option<PathBuf>,
    /// Use only the servers in `mcp_config`, none of the owner's own.
    pub strict_mcp: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// Extra environment for the agent and the commands it runs.
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub skills: bool,
    pub mcp: bool,
    pub resume: bool,
    pub approvals: bool,
    pub reports_cost: bool,
}

/// The program that logs a runtime in, which opens the sign-in page in the
/// owner's browser and keeps the credentials itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvReport {
    pub installed: bool,
    pub version: Option<String>,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<String>,
    pub multi_select: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Cost {
    pub usd: f64,
    /// True when the runtime reported the figure itself; false when estimated.
    pub reported: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// True when `usd` is the session's running total rather than this run's.
    pub cumulative: bool,
}

/// Why a run stopped without finishing its work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Auth,
    Quota,
    Crash,
    Timeout,
    Tooling,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Success,
    MaxTurns,
    Budget,
    Failed { kind: FailureKind, message: String },
}

/// Everything a run reports, in the order it happens.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    Started {
        session: String,
        model: Option<String>,
    },
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
    },
    /// The run waits until this is allowed or denied.
    Permission {
        request_id: String,
        tool: String,
        input: Value,
    },
    /// The run waits until this is answered (or denied).
    Question {
        request_id: String,
        questions: Vec<Question>,
    },
    RateLimit {
        status: String,
        resets_at: Option<u64>,
    },
    /// Token usage of one model response, for live spend estimates. The same
    /// `message_id` may be reported more than once; the latest counts.
    Usage {
        message_id: String,
        model: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    },
    /// Always the last event of a run.
    Finished {
        outcome: Outcome,
        session: Option<String>,
        cost: Cost,
        summary: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Allow {
        request_id: String,
    },
    Deny {
        request_id: String,
        message: String,
    },
    /// Answers keyed by question text; values are option labels.
    Answer {
        request_id: String,
        answers: BTreeMap<String, String>,
    },
    Interrupt,
    /// Says something to the agent in the middle of its work, as a new message
    /// from the user.
    Say {
        text: String,
    },
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("could not start the runtime: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the run has already ended")]
    Closed,
}

/// A live run: read its events, send it decisions.
pub struct Run {
    events: mpsc::Receiver<RunEvent>,
    control: mpsc::Sender<Control>,
}

impl Run {
    pub(crate) fn new(events: mpsc::Receiver<RunEvent>, control: mpsc::Sender<Control>) -> Run {
        Run { events, control }
    }

    /// The next event, or `None` after `Finished` has been read.
    pub async fn next(&mut self) -> Option<RunEvent> {
        self.events.recv().await
    }

    pub async fn send(&self, control: Control) -> Result<(), RuntimeError> {
        self.control
            .send(control)
            .await
            .map_err(|_| RuntimeError::Closed)
    }

    pub async fn allow(&self, request_id: &str) -> Result<(), RuntimeError> {
        self.send(Control::Allow {
            request_id: request_id.to_string(),
        })
        .await
    }

    pub async fn deny(&self, request_id: &str, message: &str) -> Result<(), RuntimeError> {
        self.send(Control::Deny {
            request_id: request_id.to_string(),
            message: message.to_string(),
        })
        .await
    }

    pub async fn interrupt(&self) -> Result<(), RuntimeError> {
        self.send(Control::Interrupt).await
    }

    /// Says `text` to the agent while it works.
    pub async fn say(&self, text: &str) -> Result<(), RuntimeError> {
        self.send(Control::Say {
            text: text.to_string(),
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[tokio::test]
    async fn saying_something_sends_a_message_to_the_agent() {
        let (_events, events_in) = mpsc::channel(1);
        let (control, mut controls) = mpsc::channel(4);
        let run = Run::new(events_in, control);
        run.say("Use three colours.").await.unwrap();
        run.say("And keep it short.").await.unwrap();
        let said = |text: &str| Some(Control::Say { text: text.into() });
        assert_eq!(controls.recv().await, said("Use three colours."));
        assert_eq!(controls.recv().await, said("And keep it short."));
    }

    #[tokio::test]
    async fn saying_something_to_a_run_that_has_ended_is_an_error() {
        let (_events, events_in) = mpsc::channel(1);
        let (control, controls) = mpsc::channel(1);
        let run = Run::new(events_in, control);
        drop(controls);
        assert!(matches!(
            run.say("Too late.").await,
            Err(RuntimeError::Closed)
        ));
    }
}
