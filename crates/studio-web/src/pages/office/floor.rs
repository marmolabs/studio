//! Who sits where on the floor, and the rooms that show it.

use serde_json::Value;
use std::collections::HashMap;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{HeldReason, Ticket, TicketKind, TicketState};

use super::Office;
use crate::pages::board::finished;
use crate::{client, ui};

/// Where a ticket is on the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Room {
    /// At a desk: its agent is at work, or it waits on the owner.
    Desk,
    Checks,
    Review,
    /// A producer at the planning table.
    Planning,
    Ready,
    UpNext,
    /// Landed, failed or cancelled.
    Done,
}

/// Where a ticket is; one asking or held stays where it stopped, so a
/// critic's question waits in review.
pub fn room(ticket: &Ticket) -> Room {
    let plan = ticket.kind == TicketKind::Plan;
    let stopped = matches!(ticket.state.kind(), "asking" | "held");
    let kind = match &ticket.held_from {
        Some(from) if stopped => from.kind(),
        _ => ticket.state.kind(),
    };
    match kind {
        "queued" => Room::UpNext,
        "checking" => Room::Checks,
        "reviewing" => Room::Review,
        "ready" => Room::Ready,
        _ if stopped && plan => Room::Planning,
        _ if stopped => Room::Desk,
        "claimed" | "working" if plan => Room::Planning,
        "claimed" | "working" => Room::Desk,
        _ => Room::Done,
    }
}

/// Roles that sit at the front desk rather than in the lounge.
pub const FRONT: [&str; 2] = ["approver", "router"];
/// A plan ticket and the tickets it filed.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    pub plan: Ticket,
    /// Filed tickets, cancelled ones left out.
    pub filed: Vec<Ticket>,
    pub merged: bool,
}

impl Goal {
    pub fn landed(&self) -> usize {
        self.filed
            .iter()
            .filter(|t| t.state.kind() == "landed")
            .count()
    }

    /// The goal's first line.
    pub fn name(&self) -> String {
        self.plan
            .goal
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// As `ui::look` reads a goal.
    pub fn state(&self) -> &'static str {
        ui::goal_state(&self.plan, &self.filed, self.merged)
    }
}

/// Every goal still open: not failed or cancelled, and not merged.
pub fn goals(office: &Office) -> Vec<Goal> {
    office
        .tickets
        .iter()
        .filter(|t| t.kind == TicketKind::Plan)
        .filter(|t| !matches!(t.state.kind(), "failed" | "cancelled"))
        .map(|plan| {
            let family: Vec<&Ticket> = office
                .tickets
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&plan.id))
                .collect();
            Goal {
                plan: plan.clone(),
                filed: family
                    .iter()
                    .filter(|t| t.state.kind() != "cancelled")
                    .map(|t| (*t).clone())
                    .collect(),
                merged: family
                    .iter()
                    .any(|t| office.merged.contains(&t.id.to_string())),
            }
        })
        .filter(|g| g.state() != "merged")
        .collect()
}

/// Why a ticket is held, short enough for a desk.
pub(super) fn held_short(state: &TicketState) -> &'static str {
    match state {
        TicketState::Held { reason } => match reason {
            HeldReason::Budget => "budget",
            HeldReason::Quota => "quota",
            HeldReason::Auth => "login",
            HeldReason::Conflict => "conflict",
            HeldReason::Paused => "paused",
        },
        _ => "",
    }
}

/// The project checks a run log names since `since` (epoch ms): each
/// command and when it started, oldest first.
pub fn checks_since(log: &str, since: u64) -> Vec<(String, u64)> {
    log.lines()
        .filter_map(|raw| {
            let entry: Value = serde_json::from_str(raw).ok()?;
            let at = entry["at"].as_u64().unwrap_or(0);
            let command = entry["note"].as_str()?.strip_prefix("check: ")?;
            (at >= since).then(|| (command.to_string(), at))
        })
        .collect()
}

/// When a hire's last run of its own ended, if the window shows it: its
/// latest ticket going to checks, being held, failing or being cancelled,
/// or its plan landing.
pub fn idle_since(office: &Office, hire: &str) -> Option<u64> {
    // Whether each of the hire's tickets is a plan.
    let own: HashMap<String, bool> = office
        .tickets
        .iter()
        .filter(|t| t.hire.to_string() == hire)
        .map(|t| (t.id.to_string(), t.kind == TicketKind::Plan))
        .collect();
    office
        .recent
        .iter()
        .rev()
        .find(|e| {
            own.get(&e.ticket_id).is_some_and(|plan| match e.event {
                TicketEvent::ChecksStarted
                | TicketEvent::Held { .. }
                | TicketEvent::Failed { .. }
                | TicketEvent::Cancelled => true,
                TicketEvent::Landed => *plan,
                _ => false,
            })
        })
        .map(|e| e.at_ms)
}

/// Tickets that finished in the window, newest first, with when. A landed
/// plan only filed its goal's tickets, so it doesn't count as shipped.
pub fn shipped(office: &Office) -> Vec<(Ticket, u64)> {
    finished(&office.recent, &office.tickets)
        .into_iter()
        .filter(|(t, _)| !(t.kind == TicketKind::Plan && t.state.kind() == "landed"))
        .collect()
}

/// Hires with nothing running: everyone but the front desk who is not at a
/// desk, the planning table or the review table.
pub fn resting(office: &Office) -> Vec<client::Member> {
    let busy: Vec<String> = office
        .tickets
        .iter()
        .filter(|t| matches!(room(t), Room::Desk | Room::Planning))
        .map(|t| t.hire.to_string())
        .chain(
            office
                .tickets
                .iter()
                .filter(|t| room(t) == Room::Review)
                .filter_map(|t| Some(office.judges.get(&t.id.to_string())?.hire.clone())),
        )
        .collect();
    office
        .status
        .team
        .iter()
        .filter(|m| !FRONT.contains(&m.role.as_str()) && !busy.contains(&m.id))
        .cloned()
        .collect()
}

pub fn front_desk(office: &Office) -> Vec<client::Member> {
    FRONT
        .iter()
        .filter_map(|role| office.status.team.iter().find(|m| m.role == *role))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn ticket(id: &str, state: Value, extra: Value) -> Ticket {
        let mut t = json!({
            "id": id, "project": "game", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
            t.extend(extra.clone());
        }
        serde_json::from_value(t).unwrap()
    }

    #[test]
    fn each_state_has_its_room() {
        let work = |state: Value| room(&ticket("G-1", state, json!({})));
        assert_eq!(work(json!("queued")), Room::UpNext);
        assert_eq!(work(json!("claimed")), Room::Desk);
        assert_eq!(work(json!("working")), Room::Desk);
        assert_eq!(work(json!({"asking": {"ask": "a1"}})), Room::Desk);
        assert_eq!(work(json!({"held": {"reason": "budget"}})), Room::Desk);
        assert_eq!(work(json!("checking")), Room::Checks);
        assert_eq!(work(json!("reviewing")), Room::Review);
        assert_eq!(work(json!("ready")), Room::Ready);
        assert_eq!(work(json!("landed")), Room::Done);
        assert_eq!(work(json!("cancelled")), Room::Done);
        assert_eq!(
            work(json!({"failed": {"class": "checks", "detail": "x"}})),
            Room::Done
        );
        // A producer plans at the table, not at a desk.
        let plan = |state: Value| room(&ticket("G-2", state, json!({"kind": "plan"})));
        assert_eq!(plan(json!("working")), Room::Planning);
        assert_eq!(plan(json!({"asking": {"ask": "a2"}})), Room::Planning);
        assert_eq!(plan(json!("queued")), Room::UpNext);
        // Asking or held, a ticket stays where it stopped.
        let stopped =
            |state: Value, from: &str| room(&ticket("G-3", state, json!({"held_from": from})));
        assert_eq!(
            stopped(json!({"asking": {"ask": "a3"}}), "reviewing"),
            Room::Review
        );
        assert_eq!(
            stopped(json!({"held": {"reason": "budget"}}), "checking"),
            Room::Checks
        );
        assert_eq!(
            stopped(json!({"held": {"reason": "paused"}}), "working"),
            Room::Desk
        );
    }

    #[test]
    fn checks_are_read_from_the_current_run() {
        let log = [
            r#"{"at":10,"note":"check: cargo fmt --check"}"#,
            r#"{"at":50,"note":"check: cargo fmt --check"}"#,
            r#"{"at":54,"note":"check: cargo test"}"#,
        ]
        .join("\n");
        assert_eq!(
            checks_since(&log, 40),
            [
                ("cargo fmt --check".to_string(), 50),
                ("cargo test".to_string(), 54)
            ]
        );
    }
}
