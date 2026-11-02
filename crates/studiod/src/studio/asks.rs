//! Asks: a worker waiting on the owner, and the owner's answer.

use super::{Studio, StudioError, lock};
use crate::api::AskView;
use crate::now_ms;
use serde_json::json;
use std::collections::BTreeMap;
use studio_core::event::Event;
use studio_core::ids::TicketId;
use tokio::sync::oneshot;

/// The owner's answer to an ask.
pub(crate) enum Decision {
    Allow,
    /// `stop` is false when the owner denied a permission with a note: the
    /// agent is told and carries on.
    Deny {
        message: String,
        stop: bool,
    },
    Answer(BTreeMap<String, String>),
}

pub(super) struct PendingAsk {
    pub(super) view: AskView,
    pub(super) reply: oneshot::Sender<Decision>,
}

impl Studio {
    /// Opens an ask and waits for the owner. The ticket shows as asking until then.
    pub(crate) async fn ask(
        &self,
        ticket: &TicketId,
        hire: &str,
        kind: &str,
        tool: &str,
        input: serde_json::Value,
        reasons: Vec<String>,
    ) -> Result<Decision, StudioError> {
        let id = {
            let mut next = lock(&self.next_ask);
            *next += 1;
            format!("a{}", *next)
        };
        let (reply, answer) = oneshot::channel();
        self.append(
            ticket,
            Event::AskOpened {
                ask: studio_core::ids::AskId::new(&id),
            },
        )?;
        let view = AskView {
            id: id.clone(),
            ticket: ticket.to_string(),
            hire: hire.to_string(),
            kind: kind.to_string(),
            tool: tool.to_string(),
            input,
            reasons,
            at_ms: now_ms(),
        };
        lock(&self.asks).insert(id.clone(), PendingAsk { view, reply });
        let decision = answer.await.unwrap_or_else(|_| Decision::Deny {
            message: "the ask was withdrawn".to_string(),
            stop: true,
        });
        self.append(
            ticket,
            Event::AskResolved {
                ask: studio_core::ids::AskId::new(&id),
                approved: !matches!(decision, Decision::Deny { stop: true, .. }),
            },
        )?;
        Ok(decision)
    }

    pub(super) async fn ask_owner(
        &self,
        ticket: &str,
        question: &str,
        options: Vec<String>,
    ) -> Result<String, StudioError> {
        let ticket = self.ticket(&TicketId::new(ticket))?;
        let input = json!([{
            "question": question,
            "header": "Question",
            "options": options,
            "multi_select": false,
        }]);
        let decision = self
            .ask(
                &ticket.id,
                &ticket.hire.0,
                "question",
                "ask_owner",
                input,
                Vec::new(),
            )
            .await?;
        Ok(match decision {
            Decision::Allow => "The owner approved without an answer.".to_string(),
            Decision::Answer(answers) => answers.into_values().collect::<Vec<_>>().join("; "),
            Decision::Deny { message, .. } => format!("The owner declined to answer: {message}"),
        })
    }
}
