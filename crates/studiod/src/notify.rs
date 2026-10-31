//! Desktop notifications for the moments that need the owner: an ask opens,
//! a ticket is ready, held or failed. Fire and forget; a missing notifier
//! program is ignored.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use studio_core::ticket::Ticket;

const TITLE_SHOWN: usize = 80;

pub struct Notifier {
    enabled: bool,
    /// The state kind each ticket was last seen in, so a state notifies once.
    seen: Mutex<HashMap<String, &'static str>>,
}

impl Notifier {
    pub fn new(enabled: bool) -> Notifier {
        Notifier {
            enabled,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Call after every event. Notifies when the ticket enters a state worth
    /// telling the owner about.
    pub fn observe(&self, ticket: &Ticket) {
        if !self.enabled {
            return;
        }
        let kind = ticket.state.kind();
        let id = ticket.id.to_string();
        let changed = {
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            seen.insert(id, kind) != Some(kind)
        };
        if changed && let Some(body) = message(ticket) {
            send(body);
        }
    }
}

/// What to say about a ticket, or `None` for states that don't need you.
fn message(ticket: &Ticket) -> Option<String> {
    let what = match ticket.state.kind() {
        "asking" => "is asking you something",
        "ready" => "is ready for review",
        "held" => "is held",
        "failed" => "failed",
        _ => return None,
    };
    let title: String = ticket.title.chars().take(TITLE_SHOWN).collect();
    Some(format!("{} {what}: {title}", ticket.id))
}

/// Runs the platform's notifier on a thread, so the daemon never waits on it.
fn send(body: String) {
    std::thread::spawn(move || {
        let _ = command(&body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

fn command(body: &str) -> Command {
    if cfg!(target_os = "macos") {
        // The text travels as an argument to `run`, never inside the script.
        let mut command = Command::new("osascript");
        command
            .args(["-e", "on run argv"])
            .args([
                "-e",
                "display notification (item 1 of argv) with title \"studio\"",
            ])
            .args(["-e", "end run"])
            .arg(body);
        command
    } else {
        let mut command = Command::new("notify-send");
        command.args(["studio", body]);
        command
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use studio_core::ids::{AskId, HireId, ProjectId, TicketId};
    use studio_core::ticket::{HeldReason, TicketKind, TicketState};

    fn ticket(state: TicketState) -> Ticket {
        Ticket {
            id: TicketId::new("GAME-1"),
            project: ProjectId::new("game"),
            title: "Add a pause menu".into(),
            goal: String::new(),
            hire: HireId::new("dev"),
            state,
            attempts: 0,
            base_branch: "master".into(),
            created_at: 0,
            held_from: None,
            feedback: None,
            kind: TicketKind::Work,
            parent: None,
            detail: String::new(),
            source: None,
        }
    }

    #[test]
    fn says_what_needs_the_owner() {
        let ready = message(&ticket(TicketState::Ready)).unwrap();
        assert_eq!(ready, "GAME-1 is ready for review: Add a pause menu");
        let held = ticket(TicketState::Held {
            reason: HeldReason::Budget,
        });
        assert!(message(&held).unwrap().contains("is held"));
        let ask = ticket(TicketState::Asking {
            ask: AskId::new("a1"),
        });
        assert!(message(&ask).unwrap().contains("asking"));
        assert_eq!(message(&ticket(TicketState::Working)), None);
    }
}
