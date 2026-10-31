use futures_util::Stream;
use std::collections::BTreeMap;
use studio_config::draft::{Draft, from_denial};
use studio_core::ticket::Ticket;
use studiod::api::{AskView, FileKind, FileView, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{
        content::{
            Form,
            sse::{Event, Sse},
        },
        page, path_param, route,
    },
    view::{View, component, view},
};

use super::ask_card::{ALWAYS, DECISION, MESSAGE, always_prefix, always_rule, ask_card, ticket_of};
use crate::{AppConfig, client, live, ui};

path_param!(id);

#[page("/asks")]
async fn asks(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let pending: Vec<AskView> = client::call(&config.socket, Request::Asks).await?;
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    Ok(view! {
        <h1 class=(format!("mb-3 {}", ui::PAGE_TITLE))>"Asks"</h1>
        <div data-init="@get('/asks/stream')">
            list(pending: pending, tickets: tickets)
        </div>
    })
}

#[route(GET "/asks/stream")]
async fn asks_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    Ok(live::watch(move || {
        let socket = config.socket.clone();
        let cx = cx.clone();
        async move {
            let pending: Vec<AskView> = client::call(&socket, Request::Asks).await?;
            let tickets: Vec<Ticket> = client::call(&socket, Request::Tickets).await?;
            ui::html(
                &cx,
                view! { cx => list(pending: pending, tickets: tickets) },
            )
            .await
        }
    }))
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
    let message = form.remove(MESSAGE).filter(|m| !m.trim().is_empty());
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

#[component]
async fn list(pending: Vec<AskView>, tickets: Vec<Ticket>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="asks-list" class="space-y-3">
            if pending.is_empty() {
                <p class=(format!("px-1 py-2 {}", ui::MUTED))>"Nothing is waiting for you."</p>
            }
            #[key(ask.id.clone())]
            for ask in pending {
                let (title, project) = ticket_of(&tickets, &ask.ticket);
                ask_card(ask: ask, title: title, project: project, compact: false)
            }
        </div>
    })
}
