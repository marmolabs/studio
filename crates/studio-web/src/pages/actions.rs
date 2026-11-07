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
    /// The version this work is a milestone towards; "v1.2" and "1.2" are
    /// the same.
    #[serde(default)]
    version: String,
}

#[route(POST "/goals")]
async fn create_goal(cx: &Cx, Form(form): Form<NewGoal>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::CreateGoal {
        project: form.project,
        text: form.text,
        hire: Some(form.hire).filter(|h| !h.is_empty()),
        detail: form.detail.replace("\r\n", "\n"),
        version: Some(
            form.version
                .trim()
                .trim_start_matches(['v', 'V'])
                .trim()
                .to_string(),
        )
        .filter(|v| !v.is_empty()),
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
                return ui::rule_notice(cx, "Denied.".to_string(), label, &draft).await;
            }
            let done = if allow { "Allowed" } else { "Denied" };
            let mut text = format!("{done}.");
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

#[derive(Deserialize)]
struct ProjectForm {
    #[serde(default)]
    project: String,
    /// The version to release.
    #[serde(default)]
    version: String,
    /// The hire to lead the project; empty clears the lead.
    #[serde(default)]
    hire: String,
}

/// The project named in a form, or what to tell the owner when none is.
fn project_of(form: &ProjectForm) -> Result<String, &'static str> {
    let project = form.project.trim();
    if project.is_empty() {
        Err("Pick a project.")
    } else {
        Ok(project.to_string())
    }
}

/// Starts releasing a project as the version typed: the final checks run,
/// then the release waits for the owner's go-ahead.
#[route(POST "/office/releases/start")]
async fn start_release(cx: &Cx, Form(form): Form<ProjectForm>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = match project_of(&form) {
        Ok(project) => project,
        Err(words) => return ui::notice(cx, words.into(), true).await,
    };
    // "v1.2" and "1.2" are the same version.
    let version = form.version.trim().trim_start_matches(['v', 'V']).trim();
    if version.is_empty() {
        let words = "Enter the version to release, such as 1.2.";
        return ui::notice(cx, words.into(), true).await;
    }
    let request = Request::StartRelease {
        project: project.clone(),
        version: version.to_string(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            let words = format!(
                "Started releasing {project} v{version}. The final checks run first, then it waits for your go-ahead."
            );
            ui::notice(cx, words, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The owner's go-ahead for the release that waits in a project.
#[route(POST "/office/releases/approve")]
async fn approve_release(cx: &Cx, Form(form): Form<ProjectForm>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = match project_of(&form) {
        Ok(project) => project,
        Err(words) => return ui::notice(cx, words.into(), true).await,
    };
    let request = Request::ApproveRelease {
        project: project.clone(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            ui::notice(
                cx,
                format!("Approved. {project} is being published."),
                false,
            )
            .await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// Stops the release under way in a project, before it publishes.
#[route(POST "/office/releases/cancel")]
async fn cancel_release(cx: &Cx, Form(form): Form<ProjectForm>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = match project_of(&form) {
        Ok(project) => project,
        Err(words) => return ui::notice(cx, words.into(), true).await,
    };
    let request = Request::CancelRelease {
        project: project.clone(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, format!("Stopped the release of {project}."), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// What to call a hire in a notice: the name its file gives, else its id.
async fn called(socket: &std::path::Path, hire: &str) -> String {
    let status = client::call::<client::Status>(socket, Request::Status).await;
    status
        .ok()
        .and_then(|s| s.team.into_iter().find(|m| m.id == hire))
        .map(|m| m.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| hire.to_string())
}

/// Makes a hire the lead of a project; none clears the lead.
#[route(POST "/office/projects/lead")]
async fn set_lead(cx: &Cx, Form(form): Form<ProjectForm>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = match project_of(&form) {
        Ok(project) => project,
        Err(words) => return ui::notice(cx, words.into(), true).await,
    };
    let hire = Some(form.hire.trim().to_string()).filter(|h| !h.is_empty());
    let request = Request::SetProjectLead {
        project: project.clone(),
        hire: hire.clone(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            let words = match hire {
                Some(hire) => {
                    let name = called(&config.socket, &hire).await;
                    format!("{name} now leads {project}.")
                }
                None => format!("{project} has no lead now."),
            };
            ui::notice(cx, words, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}
