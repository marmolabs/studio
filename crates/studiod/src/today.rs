//! What each hire did in the last 24 hours: tickets that passed review or
//! landed, from the event store, and reviews, critiques, approvals and
//! routings, from the studio's notes in the run logs. A call or a routing
//! counts for the hire its note says decided it: a project's lead, or else the
//! studio's approver or router.

use crate::api::{ApproverCall, HireDay};
use crate::ruletest;
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

/// What happened in a project over a stretch of time, for its lead's standup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Day {
    /// The tasks that passed review, by ticket id, in the order they did.
    pub done: Vec<String>,
    /// Goals whose plan was made.
    pub planned: usize,
    /// Times work went back to its agent, from review or from the owner.
    pub sent_back: usize,
    /// Tasks that failed.
    pub failed: usize,
}

impl Day {
    /// Whether nothing happened.
    pub(crate) fn is_quiet(&self) -> bool {
        *self == Day::default()
    }

    /// What happened in a few plain words, such as "2 tasks done, 1 sent
    /// back": empty when nothing did.
    pub(crate) fn words(&self) -> String {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if !self.done.is_empty() {
            parts.push(plural(self.done.len(), "task done", "tasks done"));
        }
        if self.sent_back > 0 {
            parts.push(format!("{} sent back", self.sent_back));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        if self.planned > 0 {
            parts.push(plural(self.planned, "goal planned", "goals planned"));
        }
        parts.join(", ")
    }
}

/// What happened to the tickets of `project` in `events`: work that passed
/// review (a ticket counts once however often it got there), plans that
/// landed, work sent back and tickets that failed.
pub(crate) fn project_day(project: &str, tickets: &[Ticket], events: &[StoredEvent]) -> Day {
    let ours: HashMap<String, &Ticket> = tickets
        .iter()
        .filter(|t| t.project.0 == project)
        .map(|t| (t.id.to_string(), t))
        .collect();
    let mut day = Day::default();
    for stored in events {
        let id = stored.ticket_id.to_string();
        let Some(ticket) = ours.get(&id) else {
            continue;
        };
        let plan = ticket.kind == TicketKind::Plan;
        match &stored.event {
            Event::ReviewPassed if !plan && !day.done.contains(&id) => day.done.push(id),
            Event::Landed if plan => day.planned += 1,
            Event::ChangesRequested { .. } | Event::ReviewFailed { .. } => day.sent_back += 1,
            Event::Failed { .. } => day.failed += 1,
            _ => {}
        }
    }
    day
}

/// Each run log changed at or after `since` (epoch ms), as its ticket id and
/// its text.
fn fresh_logs(runs: &Path, since: u64) -> impl Iterator<Item = (String, String)> {
    let since = UNIX_EPOCH + Duration::from_millis(since);
    std::fs::read_dir(runs)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(move |path| {
            path.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|modified| modified >= since)
        })
        .filter_map(|path| {
            let ticket = path.file_stem()?.to_str()?.to_string();
            Some((ticket, std::fs::read_to_string(&path).ok()?))
        })
}

/// One note of the studio's, with the hire that decided it when the log says
/// (the line's `by` field: a lead or the studio's approver or router).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Note {
    pub text: String,
    pub by: Option<String>,
}

/// The studio's notes written at or after `since` (epoch ms), from every run
/// log changed since then.
pub(crate) fn notes_since(runs: &Path, since: u64) -> Vec<Note> {
    let mut notes = Vec::new();
    for (_, text) in fresh_logs(runs, since) {
        for line in text.lines().filter(|l| l.contains("\"note\"")) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let (Some(at), Some(note)) = (value["at"].as_u64(), value["note"].as_str())
                && at >= since
            {
                notes.push(Note {
                    text: note.to_string(),
                    by: value["by"].as_str().map(str::to_string),
                });
            }
        }
    }
    notes
}

/// The approver's calls at or after `since` (epoch ms), newest first, at
/// most `limit`. Each is a note in a run log, and the call it decided is the
/// run's permission request just before it.
pub(crate) fn approver_calls(runs: &Path, since: u64, limit: usize) -> Vec<ApproverCall> {
    let mut calls = Vec::new();
    for (ticket, text) in fresh_logs(runs, since) {
        let mut asked = Value::Null;
        for line in text.lines() {
            if !line.contains("\"permission\"") && !line.contains("\"note\":\"approver ") {
                continue;
            }
            let Ok(mut value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value["event"]["type"] == "permission" {
                asked = value["event"].take();
                continue;
            }
            let (Some(at), Some(note)) = (value["at"].as_u64(), value["note"].as_str()) else {
                continue;
            };
            let Some((verdict, tool, reason)) = approver_note(note).filter(|_| at >= since) else {
                continue;
            };
            let what = if asked["tool"] == tool {
                ruletest::what(&asked["input"])
            } else {
                String::new()
            };
            calls.push(ApproverCall {
                ticket: ticket.clone(),
                at,
                tool: tool.to_string(),
                what,
                verdict: verdict.to_string(),
                reason: reason.to_string(),
            });
        }
    }
    calls.sort_by_key(|call| std::cmp::Reverse(call.at));
    calls.truncate(limit);
    calls
}

/// The verdict, tool and reason in an approver's note, such as "approver
/// allowed Bash: runs the tests" or "approver left Bash to the owner: pushes".
fn approver_note(note: &str) -> Option<(&'static str, &str, &str)> {
    let rest = note.strip_prefix("approver ")?;
    let (verdict, call) = if let Some(call) = rest.strip_prefix("allowed ") {
        ("allowed", call.split_once(": "))
    } else if let Some(call) = rest.strip_prefix("denied ") {
        ("denied", call.split_once(": "))
    } else {
        let call = rest.strip_prefix("left ")?;
        ("to_owner", call.split_once(" to the owner: "))
    };
    let (tool, reason) = call?;
    Some((verdict, tool, reason))
}

/// Counts reviews and critiques by the hire the notes name. The approver's
/// and the router's notes say who decided (a project's lead decides for its
/// project); an older note that doesn't counts for `approver` and `router`,
/// the hires that do those jobs now.
pub(crate) fn from_notes(
    notes: &[Note],
    approver: Option<&str>,
    router: Option<&str>,
    days: &mut BTreeMap<String, HireDay>,
) {
    for Note { text: note, by } in notes {
        let by = by.as_deref();
        if let Some(hire) = reviewer(note) {
            days.entry(hire.to_string()).or_default().reviews += 1;
        } else if let Some(hire) = critic(note) {
            days.entry(hire.to_string()).or_default().critiques += 1;
        } else if let (Some(hire), Some((verdict, _, _))) = (by.or(approver), approver_note(note)) {
            let day = days.entry(hire.to_string()).or_default();
            match verdict {
                "allowed" => day.allowed += 1,
                "denied" => day.denied += 1,
                _ => day.to_owner += 1,
            }
        }
        // A router that failed leaves the ticket to the first engineer.
        if let (Some(hire), Some(rest)) = (by.or(router), note.strip_prefix("routed to "))
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
    fn a_projects_day_counts_what_happened_to_its_tickets() {
        let tickets = [
            ticket("GAME-1", "dev", "work"),
            ticket("GAME-2", "dev", "work"),
            ticket("GAME-3", "producer", "plan"),
        ];
        let other = Ticket {
            project: studio_core::ids::ProjectId::new("shop"),
            ..ticket("SHOP-1", "dev", "work")
        };
        let tickets = [&tickets[..], &[other]].concat();
        let events = [
            stored("GAME-1", Event::ReviewPassed),
            stored("GAME-1", Event::ReviewPassed),
            stored("GAME-2", Event::ReviewFailed { detail: "x".into() }),
            stored("GAME-2", Event::ChangesRequested { detail: "y".into() }),
            stored("GAME-3", Event::Landed),
            stored("GAME-3", Event::ReviewPassed),
            stored("SHOP-1", Event::ReviewPassed),
            stored("NONE-1", Event::ReviewPassed),
        ];
        let day = project_day("game", &tickets, &events);
        assert_eq!(day.done, ["GAME-1"]);
        assert_eq!((day.planned, day.sent_back, day.failed), (1, 2, 0));
        assert_eq!(day.words(), "1 task done, 2 sent back, 1 goal planned");
        assert!(!day.is_quiet());
        let quiet = project_day("game", &tickets, &[]);
        assert!(quiet.is_quiet());
        assert_eq!(quiet.words(), "");
        let busy = Day {
            done: vec!["A".into(), "B".into()],
            failed: 1,
            ..Day::default()
        };
        assert_eq!(busy.words(), "2 tasks done, 1 failed");
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
        let notes: Vec<Note> = [
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
        .map(plain)
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
        assert_eq!(notes_since(dir.path(), 200), [plain("review by new (x)")]);
        assert!(notes_since(&dir.path().join("missing"), 0).is_empty());
    }

    fn plain(text: &str) -> Note {
        Note {
            text: text.to_string(),
            by: None,
        }
    }

    fn by(text: &str, hire: &str) -> Note {
        Note {
            text: text.to_string(),
            by: Some(hire.to_string()),
        }
    }

    #[test]
    fn a_leads_goals_count_under_the_lead() {
        let tickets = [
            ticket("GAME-1", "nora", "plan"),
            ticket("GAME-2", "approver", "work"),
        ];
        let events = [
            stored("GAME-1", Event::Landed),
            stored("GAME-2", Event::ReviewPassed),
        ];
        let mut days = BTreeMap::new();
        from_events(&tickets, &events, &mut days);
        assert_eq!(days["nora"].planned, 1);
        assert_eq!(days["approver"].planned, 0);
    }

    #[test]
    fn a_leads_decisions_and_routings_count_under_the_lead() {
        let notes = [
            by("approver allowed Bash: runs the tests", "nora"),
            by("approver denied Bash: not needed", "nora"),
            by("approver left Bash to the owner: pushes code", "nora"),
            by("routed to dev: game logic", "nora"),
            by(
                "routed to dev: the router failed; the first engineer",
                "nora",
            ),
            // The studio's own approver and router, and notes from before
            // the log said who decided.
            by("approver allowed Read: reads", "approver"),
            by("routed to art: textures", "router"),
            plain("approver allowed Bash: old note"),
            plain("routed to dev: old note"),
        ];
        let mut days = BTreeMap::new();
        from_notes(&notes, Some("approver"), Some("router"), &mut days);
        let lead = &days["nora"];
        assert_eq!(
            (lead.allowed, lead.denied, lead.to_owner, lead.routed),
            (1, 1, 1, 1)
        );
        let approver = &days["approver"];
        assert_eq!(
            (approver.allowed, approver.denied, approver.to_owner),
            (2, 0, 0)
        );
        assert_eq!(days["router"].routed, 2);
        assert_eq!(days["approver"].routed, 0);
        assert_eq!(days["router"].allowed, 0);
    }

    #[test]
    fn the_run_log_keeps_who_decided_in_its_own_field() {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::runlog::RunLog::new(dir.path(), "GAME-1");
        log.note_by("approver allowed Bash: runs the tests", "nora");
        log.note("review by critic (x)");
        let text = std::fs::read_to_string(dir.path().join("runs/GAME-1.jsonl")).unwrap();
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        // The note's words are the same as ever, so the activity feed reads it.
        assert_eq!(first["note"], "approver allowed Bash: runs the tests");
        let notes = notes_since(&dir.path().join("runs"), 0);
        assert_eq!(notes.len(), 2);
        assert!(notes.contains(&by("approver allowed Bash: runs the tests", "nora")));
        assert!(notes.contains(&plain("review by critic (x)")));
    }

    #[test]
    fn approver_calls_pair_each_verdict_with_its_request() {
        let dir = tempfile::tempdir().unwrap();
        let asked = |at: u64, tool: &str, input: Value| json!({"at": at, "event": {"type": "permission", "request_id": "r", "tool": tool, "input": input}});
        let one = [
            asked(100, "Bash", json!({"command": "git push"})),
            json!({"at": 101, "note": "approver allowed Bash: too old"}),
            asked(300, "Bash", json!({"command": "cargo test"})),
            json!({"at": 301, "note": "approver allowed Bash: runs the tests"}),
            asked(400, "Edit", json!({"file_path": "src/main.rs"})),
            json!({"at": 401, "note": "allowed Edit: inside the worktree"}),
            asked(500, "Bash", json!({"command": "rm -rf target"})),
            json!({"at": 501, "note": "approver left Bash to the owner: deletes files"}),
        ];
        let two = [
            json!({"at": 450, "note": "approver denied WebFetch: not needed: here"}),
            asked(600, "Write", json!({"file_path": "notes.md"})),
            json!({"at": 601, "note": "approver denied Write: outside the ticket"}),
        ];
        let join = |lines: &[Value]| {
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        std::fs::write(dir.path().join("GAME-1.jsonl"), join(&one)).unwrap();
        std::fs::write(dir.path().join("GAME-2.jsonl"), join(&two)).unwrap();

        let calls = approver_calls(dir.path(), 200, 10);
        let seen: Vec<(&str, u64, &str, &str, &str, &str)> = calls
            .iter()
            .map(|c| {
                let c = (&c.ticket, c.at, &c.tool, &c.what, &c.verdict, &c.reason);
                (
                    c.0.as_str(),
                    c.1,
                    c.2.as_str(),
                    c.3.as_str(),
                    c.4.as_str(),
                    c.5.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (
                    "GAME-2",
                    601,
                    "Write",
                    "notes.md",
                    "denied",
                    "outside the ticket"
                ),
                (
                    "GAME-1",
                    501,
                    "Bash",
                    "rm -rf target",
                    "to_owner",
                    "deletes files"
                ),
                ("GAME-2", 450, "WebFetch", "", "denied", "not needed: here"),
                (
                    "GAME-1",
                    301,
                    "Bash",
                    "cargo test",
                    "allowed",
                    "runs the tests"
                ),
            ]
        );
        assert_eq!(approver_calls(dir.path(), 200, 1).len(), 1);
        assert!(approver_calls(&dir.path().join("missing"), 0, 10).is_empty());
    }
}
