//! Everything waiting for the owner, newest first, each on its ticket.

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
    view::{Unescaped, View, component, view},
};

use super::activity::last_words;
use super::ask_card::{ALWAYS, DECISION, MESSAGE, always_prefix, always_rule, ask_card, ticket_of};
use super::ticket::log_tail;
use crate::{AppConfig, client, live, ui};

path_param!(id);

#[page("/asks")]
async fn asks(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let html = list_html(cx, config).await?;
    Ok(view! {
        <div data-init="@get('/asks/stream')" class="mx-auto flex w-full max-w-[880px] flex-col">
            (Unescaped::new_unchecked(html))
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
        let config = config.clone();
        let cx = cx.clone();
        async move { list_html(&cx, &config).await }
    }))
}

/// The asks, newest first, each with what its agent said just before.
async fn list_html(cx: &Cx, config: &AppConfig) -> topcoat::Result<String> {
    let mut pending: Vec<AskView> = client::call(&config.socket, Request::Asks).await?;
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    pending.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.id.cmp(&a.id)));
    let mut quotes = Vec::with_capacity(pending.len());
    for ask in &pending {
        let quote = if matches!(ask.kind.as_str(), "permission" | "question") {
            last_words(&log_tail(&config.data_dir, &ask.ticket).await)
        } else {
            None
        };
        quotes.push(quote);
    }
    ui::html(
        cx,
        view! { cx => list(pending: pending, tickets: tickets, quotes: quotes) },
    )
    .await
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

#[component]
async fn list(
    pending: Vec<AskView>,
    tickets: Vec<Ticket>,
    quotes: Vec<Option<String>>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="asks-list" class="flex flex-col gap-7">
            if pending.is_empty() {
                <div class=(format!("flex flex-col items-center gap-1.5 px-6 py-12 text-center {}", ui::CARD))>
                    <span class="mb-1 text-fg-3">ui::icon(name: "inbox")</span>
                    <p class="font-medium">"Nothing is waiting for you"</p>
                    <p class=(format!("text-[13px] {}", ui::MUTED))>"When an agent needs your say, its ask shows up here."</p>
                </div>
            }
            #[key(ask.id.clone())]
            for (ask, quote) in pending.into_iter().zip(quotes) {
                let (title, project) = ticket_of(&tickets, &ask.ticket);
                ask_card(ask: ask, title: title, project: project, quote: quote)
            }
        </div>
    })
}
