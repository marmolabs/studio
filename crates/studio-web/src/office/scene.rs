//! Who is where, and what they are doing.

use std::collections::HashSet;
use studio_core::ticket::Ticket;

use super::layout::{Kind, Layout, WALL_ROWS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    /// At a desk; the tile is the desk's top-left.
    Seated,
    Standing,
    /// On a couch tile.
    Couch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Typing,
    Reading,
    /// Has a ticket in the queue, not started.
    Waiting,
    /// Blocked on you: asking or held.
    Asking,
    Idle,
}

/// The ticket a hire is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub id: String,
    pub title: String,
    pub state: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub hire: String,
    pub tile: (u32, u32),
    pub pose: Pose,
    pub mood: Mood,
    pub focus: Option<Focus>,
}

/// Lower is more pressing: blocked on you, then in progress, then queued.
fn urgency(state: &str) -> Option<u8> {
    match state {
        "asking" | "held" => Some(0),
        "claimed" | "working" | "checking" | "reviewing" => Some(1),
        "queued" => Some(2),
        _ => None,
    }
}

/// The most relevant open ticket for a hire.
pub fn focus(hire: &str, tickets: &[Ticket]) -> Option<Focus> {
    tickets
        .iter()
        .filter(|t| t.hire.to_string() == hire)
        .filter_map(|t| Some((urgency(t.state.kind())?, t)))
        .min_by_key(|(rank, t)| (*rank, t.created_at))
        .map(|(_, t)| Focus {
            id: t.id.to_string(),
            title: t.title.clone(),
            state: t.state.kind(),
        })
}

pub fn mood_of(focus: Option<&Focus>) -> Mood {
    match focus.map(|f| f.state) {
        Some("asking" | "held") => Mood::Asking,
        Some("reviewing") => Mood::Reading,
        Some("queued") => Mood::Waiting,
        Some(_) => Mood::Typing,
        None => Mood::Idle,
    }
}

/// Tiles with something standing on them (a rug doesn't count).
fn blocked(layout: &Layout) -> HashSet<(u32, u32)> {
    let mut tiles = HashSet::new();
    for item in layout.items.iter().filter(|i| i.kind != Kind::Rug) {
        let (w, h) = item.kind.size();
        for x in item.x..item.x + w {
            for y in item.y..item.y + h {
                tiles.insert((x, y));
            }
        }
    }
    tiles
}

/// Places to wait around, best first: by the coffee machine, on the couch,
/// then any free floor, from the bottom up.
fn lounge(layout: &Layout) -> Vec<((u32, u32), Pose)> {
    let blocked = blocked(layout);
    let mut spots = Vec::new();
    for coffee in layout.items.iter().filter(|i| i.kind == Kind::Coffee) {
        let tile = (coffee.x, coffee.y + 1);
        if coffee.y + 1 < layout.height && !blocked.contains(&tile) {
            spots.push((tile, Pose::Standing));
        }
    }
    for couch in layout.items.iter().filter(|i| i.kind == Kind::Couch) {
        // Two seats per couch, so names don't run together.
        for x in [couch.x, couch.x + 2] {
            spots.push(((x, couch.y), Pose::Couch));
        }
    }
    for y in (WALL_ROWS..layout.height).rev() {
        for x in (0..layout.width).rev() {
            let tile = (x, y);
            let crowded = spots.iter().any(|(t, _)| t.1 == y && t.0.abs_diff(x) < 2);
            if !blocked.contains(&tile) && !crowded {
                spots.push((tile, Pose::Standing));
            }
        }
    }
    spots
}

/// Seats each hire. Hire number `i` gets desk number `i` when they have
/// work; idle hires, and hires past the last desk, go to the lounge.
pub fn assign(layout: &Layout, hires: &[String], tickets: &[Ticket]) -> Vec<Placement> {
    let desks = layout.desks();
    let mut spots = lounge(layout).into_iter();
    hires
        .iter()
        .enumerate()
        .map(|(index, hire)| {
            let focus = focus(hire, tickets);
            let mood = mood_of(focus.as_ref());
            let seat = desks.get(index).filter(|_| focus.is_some());
            let (tile, pose) = match seat {
                Some(desk) => ((desk.x, desk.y), Pose::Seated),
                None => spots
                    .next()
                    .unwrap_or(((0, layout.height.saturating_sub(1)), Pose::Standing)),
            };
            Placement {
                hire: hire.clone(),
                tile,
                pose,
                mood,
                focus,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::layout::presets;
    use serde_json::json;

    fn ticket(id: &str, hire: &str, state: serde_json::Value, created: u64) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": format!("title {id}"), "goal": "",
            "hire": hire, "state": state, "attempts": 0, "base_branch": "master",
            "created_at": created,
        }))
        .unwrap()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_most_pressing_open_ticket_wins() {
        let tickets = vec![
            ticket("G-1", "dev", json!("queued"), 1),
            ticket("G-2", "dev", json!("working"), 2),
            ticket("G-3", "dev", json!({"held": {"reason": "budget"}}), 3),
            ticket("G-4", "dev", json!("landed"), 0),
            ticket("G-5", "qa", json!("landed"), 0),
        ];
        assert_eq!(focus("dev", &tickets).unwrap().id, "G-3");
        assert_eq!(focus("qa", &tickets), None);
        assert_eq!(mood_of(focus("dev", &tickets).as_ref()), Mood::Asking);
        let without_held = &tickets[..2];
        assert_eq!(focus("dev", without_held).unwrap().id, "G-2");
    }

    #[test]
    fn workers_sit_and_idle_hires_wander_off() {
        let layout = presets::open_plan();
        let tickets = vec![ticket("G-1", "dev", json!("working"), 1)];
        let placed = assign(&layout, &names(&["dev", "qa"]), &tickets);
        assert_eq!(placed[0].pose, Pose::Seated);
        assert_eq!(placed[0].mood, Mood::Typing);
        assert_eq!(placed[0].tile, (layout.desks()[0].x, layout.desks()[0].y));
        assert_eq!(placed[1].mood, Mood::Idle);
        assert_ne!(placed[1].pose, Pose::Seated);
        // Coffee machine first.
        assert_eq!(placed[1].tile, (18, 4));
    }

    #[test]
    fn extra_hires_stand_in_the_lounge_without_sharing_a_tile() {
        let layout = Layout {
            width: 20,
            height: 12,
            items: presets::open_plan()
                .items
                .into_iter()
                .filter(|i| i.kind != Kind::Desk || i.x == 2 && i.y == 3)
                .collect(),
        };
        assert_eq!(layout.desks().len(), 1);
        let hires = names(&["a", "b", "c", "d"]);
        let tickets: Vec<Ticket> = hires
            .iter()
            .enumerate()
            .map(|(i, h)| ticket(&format!("G-{i}"), h, json!("working"), i as u64))
            .collect();
        let placed = assign(&layout, &hires, &tickets);
        assert_eq!(placed.iter().filter(|p| p.pose == Pose::Seated).count(), 1);
        let tiles: HashSet<_> = placed.iter().map(|p| p.tile).collect();
        assert_eq!(tiles.len(), 4);
        let blocked = blocked(&layout);
        for p in placed.iter().filter(|p| p.pose == Pose::Standing) {
            assert!(!blocked.contains(&p.tile), "{p:?}");
        }
        // Still shown working, even standing.
        assert!(placed.iter().all(|p| p.mood == Mood::Typing));
    }
}
