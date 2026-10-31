//! A ticket's history, one line per event.

use studio_core::event::Event;
use studio_core::ids::AskId;
use studio_core::ticket::TicketState;
use topcoat::view::{View, component, view};

use crate::client::Stamped;
use crate::ui;

/// One event, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    when: String,
    exact: String,
    label: String,
    /// Muted text after the label.
    note: Option<String>,
    detail: Option<String>,
}

pub fn rows(events: &[Stamped], now_ms: u64) -> Vec<Row> {
    // An ask and its answer share a row, at the time it was asked.
    let answer = |ask: &AskId| {
        events.iter().find_map(|stamped| match &stamped.event {
            Event::AskResolved {
                ask: resolved,
                approved,
            } if resolved == ask => Some(*approved),
            _ => None,
        })
    };
    let asked = |ask: &AskId| {
        events
            .iter()
            .any(|s| matches!(&s.event, Event::AskOpened { ask: opened } if opened == ask))
    };
    events
        .iter()
        .filter_map(|stamped| {
            let (label, detail) = describe(&stamped.event);
            let note = match &stamped.event {
                Event::RunStarted { run } => Some(run.to_string()),
                Event::AskOpened { ask } => Some(
                    match answer(ask) {
                        Some(true) => "approved",
                        Some(false) => "denied",
                        None => "waiting for you",
                    }
                    .to_string(),
                ),
                // Already shown on the row that asked.
                Event::AskResolved { ask, .. } if asked(ask) => return None,
                _ => None,
            };
            Some(Row {
                when: relative(now_ms, stamped.at_ms),
                exact: exact(stamped.at_ms),
                label,
                note,
                detail: detail.filter(|d| !d.trim().is_empty()),
            })
        })
        .collect()
}

/// A human label, and the long text to fold away, for an event.
fn describe(event: &Event) -> (String, Option<String>) {
    let plain = |label: &str| (label.to_string(), None);
    match event {
        Event::Created { .. } => plain("Created"),
        Event::Claimed => plain("Claimed"),
        Event::RunStarted { .. } => plain("Run started"),
        Event::AskOpened { .. } => plain("Asked you"),
        Event::AskResolved { approved, .. } => plain(if *approved {
            "Ask approved"
        } else {
            "Ask denied"
        }),
        Event::ChecksStarted => plain("Checks started"),
        Event::ChecksPassed => plain("Checks passed"),
        Event::ChecksFailed { detail } => ("Checks failed".into(), Some(detail.clone())),
        Event::ReviewPassed => plain("Review passed"),
        Event::ReviewFailed { detail } => ("Review failed".into(), Some(detail.clone())),
        Event::Held { reason } => {
            let state = TicketState::Held {
                reason: reason.clone(),
            };
            let why = ui::state_detail(&state).unwrap_or_default();
            plain(&format!("Held: {why}"))
        }
        Event::Resumed => plain("Resumed"),
        Event::Landed => plain("Landed"),
        Event::Cancelled => plain("Cancelled"),
        Event::Failed { class, detail } => {
            let state = TicketState::Failed {
                class: class.clone(),
                detail: String::new(),
            };
            let why = ui::state_detail(&state).unwrap_or_default();
            (format!("Failed: {why}"), Some(detail.clone()))
        }
        Event::Retried => plain("Retried"),
        Event::ChangesRequested { detail } => ("Changes requested".into(), Some(detail.clone())),
    }
}

/// "just now", "3m ago", "2h ago", "4d ago".
fn relative(now_ms: u64, at_ms: u64) -> String {
    let secs = now_ms.saturating_sub(at_ms) / 1000;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// "2026-10-30 14:05:09 UTC".
fn exact(at_ms: u64) -> String {
    let secs = at_ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[component]
pub async fn timeline(rows: Vec<Row>) -> topcoat::Result<impl View> {
    Ok(view! {
        <section id="timeline" class="mt-6">
            <h2 class=(format!("mb-2 text-[12px] font-medium {}", ui::MUTED))>"Timeline"</h2>
            <ol class="divide-y divide-zinc-200 rounded-lg border border-zinc-200 bg-white dark:divide-zinc-800 dark:border-zinc-800 dark:bg-zinc-900">
                if rows.is_empty() {
                    <li class=(format!("px-3 py-2 {}", ui::MUTED))>"Nothing yet."</li>
                }
                for row in rows {
                    <li class="px-3 py-1.5">
                        <div class="flex items-baseline gap-3">
                            <span title=(row.exact) class=(format!("w-16 shrink-0 text-[12px] tabular-nums {}", ui::MUTED))>(row.when)</span>
                            <span>
                                (row.label)
                                if let Some(note) = row.note {
                                    <span class=(format!("break-all text-[12px] {}", ui::MUTED))>" · " (note)</span>
                                }
                            </span>
                        </div>
                        if let Some(detail) = row.detail {
                            <details class="ml-[4.75rem] mt-1">
                                <summary class=(format!("cursor-pointer select-none text-[12px] {}", ui::MUTED))>"Details"</summary>
                                <pre class="mt-1 max-h-60 overflow-auto whitespace-pre-wrap font-mono text-[12px]">(detail)</pre>
                            </details>
                        }
                    </li>
                }
            </ol>
        </section>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_naturally() {
        assert_eq!(relative(1_000_000, 1_000_000 - 30_000), "just now");
        assert_eq!(relative(1_000_000, 1_000_000 - 180_000), "3m ago");
        assert_eq!(relative(10_000_000, 10_000_000 - 7_300_000), "2h ago");
        assert_eq!(exact(0), "1970-01-01 00:00:00 UTC");
        // One second after a leap day.
        assert_eq!(exact(1_709_251_201_000), "2024-03-01 00:00:01 UTC");
    }

    fn stamped(at_ms: u64, event: Event) -> Stamped {
        Stamped { at_ms, event }
    }

    #[test]
    fn an_ask_and_its_answer_share_a_row() {
        let ask = AskId::new("ASK-1");
        let events = [
            stamped(1, Event::Claimed),
            stamped(2, Event::AskOpened { ask: ask.clone() }),
            stamped(
                3,
                Event::AskResolved {
                    ask,
                    approved: true,
                },
            ),
            stamped(
                4,
                Event::AskOpened {
                    ask: AskId::new("ASK-2"),
                },
            ),
        ];
        let rows = rows(&events, 10);
        let labels: Vec<_> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.note.as_deref()))
            .collect();
        assert_eq!(
            labels,
            [
                ("Claimed", None),
                ("Asked you", Some("approved")),
                ("Asked you", Some("waiting for you")),
            ]
        );
    }

    #[test]
    fn failed_checks_fold_their_detail() {
        let (label, detail) = describe(&Event::ChecksFailed {
            detail: "boom".into(),
        });
        assert_eq!(label, "Checks failed");
        assert_eq!(detail.as_deref(), Some("boom"));
    }
}
