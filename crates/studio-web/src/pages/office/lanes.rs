//! The "Last hour" drawer under the floor: one lane per ticket that did
//! something in the hour, drawn from its whole history.

use std::collections::BTreeMap;
use std::path::Path;
use studio_core::event::Event as TicketEvent;
use studio_core::machine;
use studio_core::ticket::{Ticket, TicketState};
use studiod::api::Request;
use topcoat::view::{View, component, view};

use crate::client::{self, Happened};
use crate::ui;

/// The stretch of time the lanes cover.
const HOUR_MS: u64 = 3_600_000;
const LANES_SHOWN: usize = 12;

/// A stretch of a lane: `kind` is a `.seg` class (`work`, `check`,
/// `review`, `wait`, `hold` or `ready`).
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    kind: &'static str,
    from: u64,
    to: u64,
}

/// A ticket's row in "Last hour".
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    id: String,
    title: String,
    hire: String,
    segments: Vec<Segment>,
    /// When it was sent back or failed.
    marks: Vec<u64>,
}

/// The lanes for the hour before `now`; none when studiod can't say.
pub async fn load(socket: &Path, tickets: &[Ticket], now: u64) -> Vec<Lane> {
    let since_ms = now.saturating_sub(HOUR_MS);
    let history: Vec<Happened> = client::call(socket, Request::Activity { since_ms })
        .await
        .unwrap_or_default();
    let mut by_ticket: BTreeMap<&str, Vec<&Happened>> = BTreeMap::new();
    for logged in &history {
        by_ticket.entry(&logged.ticket_id).or_default().push(logged);
    }
    lanes(tickets, &by_ticket, now)
}

/// Each of a ticket's events with the state it left the ticket in, replayed
/// from the start.
fn replay<'a>(ticket: &Ticket, events: &[&'a Happened]) -> Vec<(&'a Happened, TicketState)> {
    let mut current = Ticket {
        state: TicketState::Queued,
        attempts: 0,
        held_from: None,
        feedback: None,
        ..ticket.clone()
    };
    events
        .iter()
        .filter(|e| !matches!(e.event, TicketEvent::Created { .. }))
        .map(|e| {
            // An event the machine refuses leaves the state as it was.
            if let Ok(next) = machine::apply(&current, &e.event) {
                current = next;
            }
            (*e, current.state.clone())
        })
        .collect()
}

/// How a state is drawn on a lane, if at all.
fn segment_kind(state: &TicketState) -> Option<&'static str> {
    match state.kind() {
        "claimed" | "working" => Some("work"),
        "checking" => Some("check"),
        "reviewing" => Some("review"),
        "asking" => Some("wait"),
        // Budget and quota holds end on their own; the rest need the owner.
        "held" if ui::waits_on_its_own(state) => Some("hold"),
        "held" => Some("wait"),
        "ready" => Some("ready"),
        _ => None,
    }
}

/// Whether an event sent the work back or stopped it.
fn sent_back(event: &TicketEvent, after: &TicketState) -> bool {
    matches!(
        event,
        TicketEvent::ChecksFailed { .. }
            | TicketEvent::ReviewFailed { .. }
            | TicketEvent::ChangesRequested { .. }
            | TicketEvent::Conflicted { .. }
    ) || matches!(after, TicketState::Failed { .. })
}

/// A ticket's lane over the hour before `now`; `None` when nothing in it
/// is drawn.
fn lane(ticket: &Ticket, events: &[&Happened], now: u64) -> Option<Lane> {
    let start = now.saturating_sub(HOUR_MS);
    let states = replay(ticket, events);
    let mut segments: Vec<Segment> = Vec::new();
    let mut marks = Vec::new();
    for (i, (logged, state)) in states.iter().enumerate() {
        let at = logged.at_ms;
        if at >= start && sent_back(&logged.event, state) {
            marks.push(at);
        }
        let end = states
            .get(i + 1)
            .map_or(now, |(next, _)| next.at_ms)
            .min(now);
        let from = at.max(start);
        let Some(kind) = segment_kind(state) else {
            continue;
        };
        if end <= from {
            continue;
        }
        match segments.last_mut() {
            Some(last) if last.kind == kind && last.to == from => last.to = end,
            _ => segments.push(Segment {
                kind,
                from,
                to: end,
            }),
        }
    }
    (!segments.is_empty() || !marks.is_empty()).then(|| Lane {
        id: ticket.id.to_string(),
        title: ticket.title.clone(),
        hire: ticket.hire.to_string(),
        segments,
        marks,
    })
}

/// The lanes of the tickets that did something in the hour, the most
/// recently active kept when there are too many, oldest ticket first.
fn lanes(tickets: &[Ticket], by_ticket: &BTreeMap<&str, Vec<&Happened>>, now: u64) -> Vec<Lane> {
    let mut drawn: Vec<(u64, &Ticket, Lane)> = tickets
        .iter()
        .filter_map(|ticket| {
            let events = by_ticket.get(ticket.id.to_string().as_str())?;
            let last = events.last().map_or(0, |e| e.at_ms);
            Some((last, ticket, lane(ticket, events, now)?))
        })
        .collect();
    drawn.sort_by_key(|(last, ..)| std::cmp::Reverse(*last));
    drawn.truncate(LANES_SHOWN);
    drawn.sort_by_key(|(_, ticket, _)| (ticket.created_at, ticket.id.to_string()));
    drawn.into_iter().map(|(_, _, lane)| lane).collect()
}

/// Minutes before `now`, for a lane's description.
fn minutes_ago(now: u64, at: u64) -> String {
    match now.saturating_sub(at) / 60_000 {
        0 => "now".into(),
        n => format!("{n}m ago"),
    }
}

/// A lane in words, for screen readers.
fn lane_label(lane: &Lane, now: u64) -> String {
    let name = |kind| match kind {
        "work" => "working",
        "check" => "checks",
        "review" => "critic or review",
        "wait" => "waiting for you",
        "hold" => "on hold",
        _ => "ready to land",
    };
    let mut parts: Vec<String> = lane
        .segments
        .iter()
        .map(|s| {
            format!(
                "{} from {} to {}",
                name(s.kind),
                minutes_ago(now, s.from),
                minutes_ago(now, s.to)
            )
        })
        .collect();
    for at in &lane.marks {
        parts.push(format!("sent back {}", minutes_ago(now, *at)));
    }
    format!("{}: {}", lane.id, parts.join(", "))
}

/// Where a moment falls on a lane, in percent from the left.
fn position(now: u64, at: u64) -> f64 {
    let start = now.saturating_sub(HOUR_MS);
    at.clamp(start, now).saturating_sub(start) as f64 * 100.0 / HOUR_MS as f64
}

/// The legend: (`.sw` class, label).
const LEGEND: [(&str, &str); 7] = [
    ("work", "Working"),
    ("check", "Checks"),
    ("review", "Critic or review"),
    ("wait", "Waiting for you"),
    ("hold", "On hold"),
    ("fail", "Sent back"),
    ("ready", "Ready to land"),
];

/// The time axis: (left in percent, label).
const AXIS: [(u8, &str); 5] = [(0, ""), (25, "45"), (50, "30"), (75, "15"), (100, "Now")];

/// The drawer's body: the legend, the axis and one lane per ticket.
#[component]
pub async fn hour(rows: Vec<Lane>, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="office-lanes" class="px-[18px] pb-[18px] max-[760px]:px-4">
            <div class="mb-3 flex flex-wrap gap-x-4 gap-y-1.5 text-[12.5px] text-fg-2">
                for (class, label) in LEGEND {
                    <span class="inline-flex items-center gap-[7px]">
                        <i class=(format!("sw {class} inline-block size-2.5 shrink-0 rounded-[3px]"))></i>
                        (label)
                    </span>
                }
            </div>
            if rows.is_empty() {
                <p class=(format!("text-[13px] {}", ui::MUTED))>"No ticket has done anything in the last hour."</p>
            } else {
                <div class="grid grid-cols-[260px_minmax(0,1fr)] items-center gap-x-4 gap-y-1.5 max-[760px]:grid-cols-[72px_minmax(0,1fr)] max-[760px]:gap-x-2.5">
                    <div></div>
                    <div class="relative h-[18px] text-[11.5px] text-fg-3 tabular-nums" aria-hidden="true">
                        for (left, label) in AXIS {
                            if left == 0 {
                                <span class="absolute top-0 whitespace-nowrap" style="left:0">
                                    <span class="max-[760px]:hidden">"60 min ago"</span>
                                    <span class="hidden max-[760px]:inline">"60m"</span>
                                </span>
                            } else if left == 100 {
                                <span class="absolute top-0 -translate-x-full whitespace-nowrap text-fg" style="left:100%">(label)</span>
                            } else {
                                <span class="absolute top-0 -translate-x-1/2 whitespace-nowrap" style=(format!("left:{left}%"))>(label)</span>
                            }
                        }
                    </div>
                    #[key(row.id.clone())]
                    for row in rows {
                        lane_row(row: row, now: now)
                    }
                </div>
            }
        </div>
    })
}

#[component]
async fn lane_row(row: Lane, now: u64) -> topcoat::Result<impl View> {
    let label = lane_label(&row, now);
    let href = format!("/tickets/{}", row.id);
    Ok(view! {
        <a href=(href) class="flex min-w-0 items-center gap-2 text-[13px] hover:text-fg">
            <span class="contents max-[760px]:hidden">ui::avatar(hire: row.hire.clone())</span>
            <span class="shrink-0 font-mono text-[12px] text-fg-3">(row.id.clone())</span>
            <span class="truncate max-[760px]:hidden">(row.title.clone())</span>
        </a>
        <div class="relative h-7 rounded-md bg-bg-2 bg-[linear-gradient(90deg,var(--line)_1px,transparent_1px)] bg-[length:25%_100%]" role="img" aria-label=(label)>
            for s in row.segments {
                let left = position(now, s.from);
                let width = position(now, s.to) - left;
                let shape = if s.kind == "ready" { "top-[13px] h-0.5 rounded-[1px]" } else { "top-2 h-3 rounded-[3px]" };
                <i class=(format!("seg {} absolute {shape}", s.kind)) style=(format!("left:{left:.2}%;width:max(2px,{width:.2}%)"))></i>
            }
            for at in row.marks {
                <i class="seg fail absolute top-2 h-3 w-1 rounded-[2px]" style=(format!("left:calc({:.2}% - 2px)", position(now, at)))></i>
            }
            <span class="absolute -bottom-[3px] -right-px -top-[3px] w-0.5 rounded-[1px] bg-fg"></span>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 10 * HOUR_MS;

    fn ticket(state: serde_json::Value) -> Ticket {
        serde_json::from_value(json!({
            "id": "GAME-1", "project": "game", "title": "Waves", "goal": "", "hire": "dev",
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    fn history(events: &[(u64, serde_json::Value)]) -> Vec<Happened> {
        let created = json!({"type": "created", "id": "GAME-1", "project": "game",
            "title": "Waves", "goal": "", "hire": "dev", "base_branch": "main", "created_at": 0});
        std::iter::once((200, created))
            .chain(events.iter().cloned())
            .map(|(minutes_ago, event)| {
                serde_json::from_value(json!({
                    "ticket_id": "GAME-1", "at_ms": NOW - minutes_ago * 60_000, "event": event,
                }))
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn lanes_follow_the_tickets_states_within_the_hour() {
        let events = history(&[
            (90, json!({"type": "claimed"})),
            (89, json!({"type": "run_started", "run": "r1"})),
            (30, json!({"type": "checks_started"})),
            (25, json!({"type": "checks_failed", "detail": "1 failed"})),
            (10, json!({"type": "ask_opened", "ask": "ASK-1"})),
        ]);
        let refs: Vec<&Happened> = events.iter().collect();
        let lane = lane(&ticket(json!({"asking": {"ask": "ASK-1"}})), &refs, NOW).unwrap();
        let at = |minutes_ago: u64| NOW - minutes_ago * 60_000;
        let segment = |kind, from, to| Segment { kind, from, to };
        // Working since before the hour began, clipped to its start.
        assert_eq!(
            lane.segments,
            [
                segment("work", at(60), at(30)),
                segment("check", at(30), at(25)),
                segment("work", at(25), at(10)),
                segment("wait", at(10), NOW),
            ]
        );
        assert_eq!(lane.marks, [at(25)]);
        assert_eq!(position(NOW, at(30)), 50.0);
        assert!(
            lane_label(&lane, NOW)
                .ends_with("waiting for you from 10m ago to now, sent back 25m ago"),
            "{}",
            lane_label(&lane, NOW)
        );
    }

    #[test]
    fn tickets_quiet_all_hour_get_no_lane() {
        let events = history(&[(120, json!({"type": "cancelled"}))]);
        let refs: Vec<&Happened> = events.iter().collect();
        assert_eq!(lane(&ticket(json!("cancelled")), &refs, NOW), None);
    }
}
