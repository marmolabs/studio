//! What each hire did in the last 24 hours: tickets that passed review or
//! landed, from the event store, and reviews, critiques, approvals and
//! routings, from the studio's notes in the run logs.

use crate::api::HireDay;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};
use studio_core::event::Event;
use studio_core::ticket::{Ticket, TicketKind};
use studio_store::StoredEvent;

/// Counts, for each ticket's hire, the work that passed review and the plans
/// that landed. A ticket counts once however often it got there.
pub(crate) fn from_events(
    tickets: &[Ticket],
    events: &[StoredEvent],
    days: &mut BTreeMap<String, HireDay>,
) {
    let by_id: HashMap<String, &Ticket> = tickets.iter().map(|t| (t.id.to_string(), t)).collect();
    let mut counted = BTreeSet::new();
    for stored in events {
        let Some(ticket) = by_id.get(&stored.ticket_id.to_string()) else {
            continue;
        };
        let plan = ticket.kind == TicketKind::Plan;
        let done = match &stored.event {
            Event::ReviewPassed if !plan => true,
            Event::Landed if plan => false,
            _ => continue,
        };
        if !counted.insert(ticket.id.to_string()) {
            continue;
        }
        let day = days.entry(ticket.hire.to_string()).or_default();
        if done {
            day.done += 1;
        } else {
            day.planned += 1;
        }
    }
}

/// The studio's notes written at or after `since` (epoch ms), from every run
/// log changed since then.
pub(crate) fn notes_since(runs: &Path, since: u64) -> Vec<String> {
    let mut notes = Vec::new();
    let Ok(entries) = std::fs::read_dir(runs) else {
        return notes;
    };
    let since_time = UNIX_EPOCH + Duration::from_millis(since);
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified >= since_time);
        if !fresh {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().filter(|l| l.contains("\"note\"")) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let (Some(at), Some(note)) = (value["at"].as_u64(), value["note"].as_str())
                && at >= since
            {
                notes.push(note.to_string());
            }
        }
    }
    notes
}

/// Counts reviews and critiques by the hire the notes name. The approver's
/// and the router's notes don't name one, so they count for `approver` and
/// `router`, the hires that do those jobs now.
pub(crate) fn from_notes(
    notes: &[String],
    approver: Option<&str>,
    router: Option<&str>,
    days: &mut BTreeMap<String, HireDay>,
) {
    for note in notes {
        if let Some(hire) = reviewer(note) {
            days.entry(hire.to_string()).or_default().reviews += 1;
        } else if let Some(hire) = critic(note) {
            days.entry(hire.to_string()).or_default().critiques += 1;
        } else if let (Some(hire), Some(call)) = (approver, note.strip_prefix("approver ")) {
            let day = days.entry(hire.to_string()).or_default();
            match call.split_once(' ').map(|(word, _)| word) {
                Some("allowed") => day.allowed += 1,
                Some("denied") => day.denied += 1,
                Some("left") => day.to_owner += 1,
                _ => {}
            }
        }
        // A router that failed leaves the ticket to the first engineer.
        if let (Some(hire), Some(rest)) = (router, note.strip_prefix("routed to "))
            && !rest.ends_with("the first engineer")
        {
            days.entry(hire.to_string()).or_default().routed += 1;
        }
    }
}

/// The hire in "review by <hire> (<model>)".
fn reviewer(note: &str) -> Option<&str> {
    let rest = note.strip_prefix("review by ")?;
    rest.split_once(" (").map(|(hire, _)| hire)
}

/// The hire in "gate <name> by <hire> (<model>)".
fn critic(note: &str) -> Option<&str> {
    let rest = note.strip_prefix("gate ")?.strip_suffix(')')?;
    let (_, by) = rest.rsplit_once(" by ")?;
    by.split_once(" (").map(|(hire, _)| hire)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;
    use studio_core::ids::TicketId;

    fn ticket(id: &str, hire: &str, kind: &str) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": "t", "goal": "", "hire": hire,
            "state": "landed", "attempts": 1, "base_branch": "main", "created_at": 0,
            "kind": kind,
        }))
        .unwrap()
    }

    fn stored(id: &str, event: Event) -> StoredEvent {
        StoredEvent {
            seq: 0,
            ticket_id: TicketId::new(id),
            at_ms: 0,
            event,
        }
    }

    #[test]
    fn passed_work_and_landed_plans_count_once_per_ticket() {
        let tickets = [
            ticket("GAME-1", "dev", "work"),
            ticket("GAME-2", "dev", "work"),
            ticket("GAME-3", "producer", "plan"),
        ];
        let events = [
            stored("GAME-1", Event::ReviewPassed),
            stored("GAME-1", Event::ReviewPassed),
            stored("GAME-1", Event::Landed),
            stored("GAME-2", Event::Claimed),
            stored("GAME-3", Event::Landed),
            stored("GAME-9", Event::ReviewPassed),
        ];
        let mut days = BTreeMap::new();
        from_events(&tickets, &events, &mut days);
        assert_eq!(days["dev"].done, 1);
        assert_eq!((days["producer"].done, days["producer"].planned), (0, 1));
        assert_eq!(days.len(), 2);
    }

    #[test]
    fn notes_count_reviews_critiques_approvals_and_routings() {
        let notes: Vec<String> = [
            "review by critic (opus)",
            "review passed",
            "gate art by designer (sonnet)",
            "gate art passed",
            "gate art passed by the owner",
            "approver allowed Bash: runs the tests",
            "approver allowed Read: reads",
            "approver denied Bash: not needed",
            "approver left Bash to the owner: pushes code",
            "allowed Edit: inside the worktree",
            "routed to dev: game logic",
            "routed to dev: no router; the first engineer",
        ]
        .map(String::from)
        .to_vec();
        let mut days = BTreeMap::new();
        from_notes(&notes, Some("approver"), Some("router"), &mut days);
        assert_eq!(days["critic"].reviews, 1);
        assert_eq!(days["designer"].critiques, 1);
        let approver = &days["approver"];
        assert_eq!(
            (approver.allowed, approver.denied, approver.to_owner),
            (2, 1, 1)
        );
        assert_eq!(days["router"].routed, 1);
        assert!(!days.contains_key("the owner"));

        // Without an approver or router, their notes count for no one.
        let mut days = BTreeMap::new();
        from_notes(&notes, None, None, &mut days);
        assert_eq!(days.len(), 2);
    }

    #[test]
    fn only_recent_notes_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let lines = [
            json!({"at": 100, "note": "review by old (x)"}),
            json!({"at": 300, "note": "review by new (x)"}),
            json!({"at": 300, "event": {"type": "text", "text": "\"note\""}}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        std::fs::write(dir.path().join("GAME-1.jsonl"), lines).unwrap();
        std::fs::write(dir.path().join("GAME-1.mcp.json"), "{\"note\": 1}").unwrap();
        assert_eq!(notes_since(dir.path(), 200), ["review by new (x)"]);
        assert!(notes_since(&dir.path().join("missing"), 0).is_empty());
    }
}
