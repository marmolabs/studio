//! The Log tab: what happened lately, newest first.

use studio_core::event::Event as TicketEvent;
use topcoat::view::{View, component, view};

use super::{quiet, tint};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::inspector::{self};
use crate::pages::office::{Office, ago};

/// The Log tab lists this many events.
const LOG_SHOWN: usize = 50;

#[component]
pub(super) async fn log_pane(office: Office) -> topcoat::Result<impl View> {
    let now = office.now;
    let rows: Vec<(String, String, Option<usize>, String, String)> = office
        .recent
        .iter()
        .rev()
        .filter(|e| !matches!(e.event, TicketEvent::RunStarted { .. }))
        .take(LOG_SHOWN)
        .map(|e| {
            let said = inspector::event_words(&e.event, &office.asks);
            let project = office
                .ticket(&e.ticket_id)
                .map(|t| tint(&office, &t.project.to_string()));
            (
                ago(now.saturating_sub(e.at_ms)),
                format!("dot {}", facts::dot(facts::event_state(&e.event))),
                project,
                e.ticket_id.clone(),
                said,
            )
        })
        .collect();
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "Nothing has happened lately.")
        }
        <ul class="m-0 flex list-none flex-col gap-2 p-0 text-[13px] text-fg-2">
            for (when, dot, project, id, said) in rows {
                <li class="grid grid-cols-[28px_8px_9px_minmax(0,1fr)] items-baseline gap-2">
                    <time class="text-[12px] tabular-nums text-fg-3">(when)</time>
                    <span class=(dot)></span>
                    if let Some(index) = project {
                        <i class=(dot_class(index))></i>
                    } else {
                        <i></i>
                    }
                    <span class="min-w-0">
                        <a href=(format!("/tickets/{id}")) class=(inspector::TICKET_LINK)>(id.clone())</a>
                        " " (said)
                    </span>
                </li>
            }
        </ul>
    })
}

#[cfg(test)]
mod tests {

    use crate::pages::office::panel::fixtures::{extra, html_of, office};

    #[tokio::test]
    async fn the_log_is_newest_first_with_project_dots() {
        let html = html_of(office(), extra(), "").await;
        let log = &html[html.find(r#"data-show="$tab == 'log'""#).unwrap()..];
        let api = log.find(">API-1<").unwrap();
        let shop = log.find(">SHOP-2<").unwrap();
        assert!(api < shop, "{log}");
        assert!(log.contains("failed"), "{log}");
        assert!(log.contains(">1h<") && log.contains(">2h<"), "{log}");
    }
}
