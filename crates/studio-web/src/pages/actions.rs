//! The office's actions that change something: filing a ticket, starting a
//! goal and answering an ask.

use serde::Deserialize;
use std::collections::BTreeMap;
use studio_config::draft::{Draft, from_denial};
use studio_core::ticket::Ticket;
use studiod::api::{AskView, FileKind, FileView, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{content::Form, path_param, route},
};

use super::ask_card::{ALWAYS, DECISION, MESSAGE, always_prefix, always_rule, ticket_of};
use crate::{AppConfig, client, ui};

path_param!(id);

#[derive(Deserialize)]
struct NewTicket {
    project: String,
    /// Empty means the router picks.
    #[serde(default)]
    hire: String,
    title: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    detail: String,
}

#[route(POST "/tickets")]
async fn create_ticket(cx: &Cx, Form(form): Form<NewTicket>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::CreateTicket {
        project: form.project,
        title: form.title,
        goal: form.goal,
        hire: form.hire,
        base: None,
        detail: form.detail.replace("\r\n", "\n"),
    };
    match client::call::<Ticket>(&config.socket, request).await {
        Ok(ticket) => ui::notice(cx, format!("Queued {}.", ticket.id), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct NewGoal {
    project: String,
    text: String,
    /// The brief for the producer.
    #[serde(default)]
    detail: String,
    #[serde(default)]
    hire: String,
}

#[route(POST "/goals")]
async fn create_goal(cx: &Cx, Form(form): Form<NewGoal>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::CreateGoal {
        project: form.project,
        text: form.text,
        hire: Some(form.hire).filter(|h| !h.is_empty()),
        detail: form.detail.replace("\r\n", "\n"),
    };
    match client::call::<Ticket>(&config.socket, request).await {
        Ok(ticket) => ui::notice(cx, format!("Started {}.", ticket.id), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// Allow or deny an ask. The clicked button is the `decision` field;
/// every other field except `message` answers a question by its text.
/// "Always allow" allows the ask and saves a rule for its command.
#[route(POST "/asks/{id}")]
async fn decide(
    cx: &Cx,
    Form(mut form): Form<BTreeMap<String, String>>,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let ask = path_param::<Id>(cx).to_owned();
    let decision = form.remove(DECISION);
    let always = decision.as_deref() == Some(ALWAYS);
    let allow = always || decision.as_deref() == Some("approve");
    // A note goes only with a denial: an allow can't carry one to the agent.
    let note = form.remove(MESSAGE).filter(|m| !m.trim().is_empty());
    let unsent = allow && note.is_some();
    let message = note.filter(|_| !allow);
    let answers = (allow && !form.is_empty()).then_some(form);
    // The rule comes from the ask as the daemon has it, not from the form.
    let rule = if always {
        always_rule_for(&config.socket, &ask).await
    } else {
        None
    };
    // Decided asks leave the list, so a denial's rule is drafted first.
    let draft = if allow {
        None
    } else {
        denial_draft(&config.socket, &ask, message.as_deref()).await
    };
    let request = Request::Decide {
        ask: ask.clone(),
        allow,
        message,
        answers,
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            if let Some(draft) = draft {
                let label = "Make this a rule".to_string();
                return ui::rule_notice(cx, format!("Denied {ask}."), label, &draft).await;
            }
            let done = if allow { "Allowed" } else { "Denied" };
            let mut text = format!("{done} {ask}.");
            if unsent {
                text.push_str(" Your note was not sent: a note goes only with Deny.");
            }
            if let Some((project, prefix)) = rule {
                match save_rule(&config.socket, &project, &prefix).await {
                    Ok(name) => text.push_str(&format!(" Saved rule {name}.")),
                    Err(err) => text.push_str(&format!(" The rule was not saved: {err}")),
                }
            }
            ui::notice(cx, text, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The project and command prefix an "always allow" on `ask` is for.
async fn always_rule_for(socket: &std::path::Path, ask: &str) -> Option<(String, String)> {
    let pending: Vec<AskView> = client::call(socket, Request::Asks).await.ok()?;
    let ask = pending.into_iter().find(|a| a.id == ask)?;
    let prefix = always_prefix(&ask)?;
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await.ok()?;
    let (_, project) = ticket_of(&tickets, &ask.ticket);
    (!project.is_empty()).then_some((project, prefix))
}

/// The rule a denial of `ask` suggests, from the ask as the daemon has it.
async fn denial_draft(socket: &std::path::Path, ask: &str, message: Option<&str>) -> Option<Draft> {
    let pending: Vec<AskView> = client::call(socket, Request::Asks).await.ok()?;
    let ask = pending.into_iter().find(|a| a.id == ask)?;
    if ask.kind != "permission" {
        return None;
    }
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await.ok()?;
    let (_, project) = ticket_of(&tickets, &ask.ticket);
    (!project.is_empty())
        .then(|| from_denial(&project, &ask.tool, &ask.input, message.unwrap_or_default()))
}

/// Saves the allow rule unless one with that text is already there; a
/// different file under the same name is never replaced.
async fn save_rule(
    socket: &std::path::Path,
    project: &str,
    prefix: &str,
) -> topcoat::Result<String> {
    let (base, text) = always_rule(project, prefix);
    let files: Vec<FileView> = client::call(
        socket,
        Request::Files {
            kind: FileKind::Rules,
        },
    )
    .await?;
    let mut name = base.clone();
    for n in 2..100 {
        match files.iter().find(|f| f.name == name) {
            Some(file) if file.text == text => return Ok(name),
            Some(_) => name = format!("{base}-{n}"),
            None => break,
        }
    }
    let request = Request::SaveFile {
        kind: FileKind::Rules,
        name: name.clone(),
        text,
    };
    client::call::<serde_json::Value>(socket, request).await?;
    Ok(name)
}
