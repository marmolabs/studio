//! One-turn model calls for the studio's own hires (the approver and the
//! router): cheap, tool-less, capped, and charged to a ticket.

use crate::budget::Meter;
use crate::studio::Studio;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use studio_core::ids::TicketId;
use studio_core::rule::Hire;
use studio_runtime::{Outcome, RunEvent, RunSpec};
use studio_store::Spend;

/// The most one quick call may cost.
pub(crate) const QUICK_USD: f64 = 0.05;

/// How long one quick call may take.
const TIMEOUT: Duration = Duration::from_secs(90);

/// What a quick call said, or why it gave no answer, and what it cost either
/// way. The caller records the spend once it knows which ticket pays.
pub(crate) struct Reply {
    pub answer: Result<String, String>,
    spend: Spend,
}

impl Reply {
    /// The spend, charged to `ticket`.
    pub(crate) fn charge(&self, ticket: &TicketId) -> Spend {
        Spend {
            ticket_id: ticket.clone(),
            ..self.spend.clone()
        }
    }
}

/// Asks the hire's model one question with no tools. An error means the call
/// never started and cost nothing. Nothing is recorded; see `ask_model` for the
/// common case.
pub(crate) async fn ask(
    studio: &Arc<Studio>,
    for_ticket: Option<&TicketId>,
    project: &str,
    hire: &Hire,
    cwd: &Path,
    prompt: &str,
) -> Result<Reply, String> {
    let role = &hire.role;
    let for_ticket = for_ticket.map(TicketId::to_string).unwrap_or_default();
    if !studio.has_headroom(project, hire, QUICK_USD, &for_ticket) {
        return Err(format!("no budget left for the {role}"));
    }
    let runtime = studio.runtime(&hire.runtime).map_err(|e| e.to_string())?;
    let spec = RunSpec {
        cwd: cwd.to_path_buf(),
        prompt: prompt.to_string(),
        model: Some(hire.model.clone()).filter(|m| !m.is_empty()),
        effort: hire.effort.clone(),
        append_system_prompt: Some(hire.prompt.clone()).filter(|p| !p.trim().is_empty()),
        max_turns: Some(1),
        max_budget_usd: Some(QUICK_USD),
        strict_mcp: true,
        mcp_config: None,
        disallowed_tools: [
            "Bash",
            "Edit",
            "Write",
            "NotebookEdit",
            "WebFetch",
            "WebSearch",
            "Task",
        ]
        .map(String::from)
        .to_vec(),
        ..RunSpec::default()
    };
    let mut run = runtime
        .start(spec)
        .map_err(|e| format!("the {role} could not start: {e}"))?;
    let mut meter = Meter::new(studio.price(&hire.model));
    let mut text = String::new();
    let mut model = hire.model.clone();
    let mut session = None;
    let read = async {
        while let Some(event) = run.next().await {
            match event {
                RunEvent::Started { model: Some(m), .. } => model = m,
                RunEvent::Text { text: t } => {
                    text.push_str(&t);
                    text.push('\n');
                }
                RunEvent::Usage {
                    message_id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    ..
                } => meter.update(
                    &message_id,
                    (
                        input_tokens,
                        output_tokens,
                        cache_read_tokens,
                        cache_write_tokens,
                    ),
                ),
                RunEvent::Permission { request_id, .. } | RunEvent::Question { request_id, .. } => {
                    let _ = run.deny(&request_id, "No tools here: just answer.").await;
                }
                RunEvent::Finished {
                    outcome,
                    session: s,
                    cost,
                    summary,
                } => {
                    session = s;
                    if let Some(summary) = summary {
                        text.push_str(&summary);
                    }
                    let (usd, reported) = if cost.reported {
                        (cost.usd, true)
                    } else {
                        (meter.usd(), false)
                    };
                    return match outcome {
                        Outcome::Failed { message, .. } => {
                            Err((format!("the {role} failed: {message}"), usd, reported))
                        }
                        _ => Ok((usd, reported)),
                    };
                }
                _ => {}
            }
        }
        Err((format!("the {role} ended without an answer"), 0.0, false))
    };
    let result = match tokio::time::timeout(TIMEOUT, read).await {
        Ok(result) => result,
        Err(_) => {
            let _ = run.interrupt().await;
            Err((format!("the {role} took too long"), 0.0, false))
        }
    };
    let (input_tokens, output_tokens) = meter.tokens();
    let (outcome, usd, reported) = match result {
        Ok((usd, reported)) => (Ok(()), usd, reported),
        Err((message, usd, reported)) => (Err(message), usd, reported),
    };
    let spend = Spend {
        at_ms: crate::now_ms(),
        ticket_id: TicketId::new(""),
        project: project.to_string(),
        hire: hire.id.0.clone(),
        runtime: runtime.id().to_string(),
        model,
        session,
        usd,
        reported,
        input_tokens,
        output_tokens,
    };
    Ok(Reply {
        answer: outcome.map(|()| text),
        spend,
    })
}

/// `ask`, with the spend recorded against `ticket`, answered or not.
pub(crate) async fn ask_model(
    studio: &Arc<Studio>,
    ticket: &TicketId,
    project: &str,
    hire: &Hire,
    cwd: &Path,
    prompt: &str,
) -> Result<String, String> {
    let reply = ask(studio, Some(ticket), project, hire, cwd, prompt).await?;
    studio
        .record_spend(&reply.charge(ticket))
        .map_err(|e| e.to_string())?;
    reply.answer
}
