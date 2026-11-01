//! The Team tab: every hire with its level and the day's numbers.

use studiod::api::HireDay;
use topcoat::view::{View, component, view};

use super::{Extra, capital, portrait_class, quiet, section, share};
use crate::pages::office::floor::{self, Room, room};
use crate::pages::office::{Office, ago, key, select};
use crate::ui;

/// Landed tickets that make a hire reach level 2, 3 and so on up to 10.
const LEVELS: [usize; 9] = [2, 5, 10, 20, 35, 55, 80, 110, 150];

/// The level for `landed` tickets, and how far through it, as (landed since
/// this level began, landed this level takes); `None` at the top level.
fn level(landed: usize) -> (usize, Option<(usize, usize)>) {
    let level = 1 + LEVELS.iter().filter(|&&need| landed >= need).count();
    let from = if level == 1 { 0 } else { LEVELS[level - 2] };
    let next = LEVELS.get(level - 1).map(|&to| (landed - from, to - from));
    (level, next)
}

/// One hire in the team list.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Member {
    pub(super) id: String,
    pub(super) look: String,
    /// Level and the share of the way to the next, for those who level up.
    pub(super) level: Option<(usize, f64)>,
    pub(super) line: String,
    /// Today's figures: the number and what it counts.
    pub(super) figures: Vec<(String, &'static str)>,
}

/// What a hire is doing: the ticket at its desk or table, or the one it is
/// judging in review.
fn doing_now(office: &Office, hire: &str) -> Option<String> {
    let own = office
        .tickets
        .iter()
        .filter(|t| t.hire.to_string() == hire)
        .find(|t| matches!(room(t), Room::Desk | Room::Planning));
    if let Some(t) = own {
        let id = t.id.to_string();
        return Some(match t.state.kind() {
            "asking" => format!("waiting for you on {id}"),
            "held" => format!("{id} on hold"),
            _ if room(t) == Room::Planning => format!("planning {id}"),
            _ => format!("on {id}"),
        });
    }
    office
        .tickets
        .iter()
        .filter(|t| room(t) == Room::Review)
        .find_map(|t| {
            let judge = office.judges.get(&t.id.to_string())?;
            (judge.hire == hire).then(|| {
                let verb = if judge.gate.is_some() {
                    "judging"
                } else {
                    "reading"
                };
                format!("{verb} {}", t.id)
            })
        })
}

/// Today's numbers for a hire, by what its role does.
fn figures(role: &str, day: &HireDay, spent: Option<f64>) -> Vec<(String, &'static str)> {
    let money = |spent: Option<f64>| spent.map(|usd| (ui::usd(usd), "spent"));
    let decided = day.allowed + day.denied + day.to_owner;
    let mut out: Vec<(String, &'static str)> = match role {
        "reviewer" => vec![(day.reviews.to_string(), "reviewed")],
        "critic" => vec![(day.critiques.to_string(), "judged")],
        "producer" => vec![(day.planned.to_string(), "planned")],
        "approver" => vec![
            (decided.to_string(), "decided"),
            (day.to_owner.to_string(), "to you"),
        ],
        "router" => vec![(day.routed.to_string(), "routed")],
        _ => vec![(day.done.to_string(), "done")],
    };
    if role != "approver" {
        out.extend(money(spent));
    }
    out
}

/// The team in three groups: at work, resting and at the front desk.
pub(super) fn members(office: &Office, extra: &Extra) -> [Vec<Member>; 3] {
    let mut groups: [Vec<Member>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for m in &office.status.team {
        let front = floor::FRONT.contains(&m.role.as_str());
        // Reviewers and critics judge work rather than land it, so a level
        // from landed tickets would say nothing about them.
        let levelled = !front && !matches!(m.role.as_str(), "reviewer" | "critic");
        let landed = office
            .tickets
            .iter()
            .filter(|t| t.hire.to_string() == m.id && t.state.kind() == "landed")
            .count();
        let (lv, next) = level(landed);
        let now = doing_now(office, &m.id);
        let doing = match (&now, front) {
            (Some(now), _) => now.clone(),
            (None, true) => String::new(),
            (None, false) => match floor::idle_since(office, &m.id) {
                Some(at) => format!("resting {}", ago(office.now.saturating_sub(at))),
                None => "resting".into(),
            },
        };
        let about = [capital(&m.role), m.model.clone(), doing]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let spent = extra
            .budget
            .as_ref()
            .map(|b| b.spent_24h.by_hire.get(&m.id).copied().unwrap_or(0.0));
        let day = office.today.get(&m.id).cloned().unwrap_or_default();
        let member = Member {
            id: m.id.clone(),
            look: portrait_class(&m.id, &m.role),
            level: levelled.then(|| {
                (
                    lv,
                    next.map_or(100.0, |(have, need)| share(have as f64, need as f64)),
                )
            }),
            line: about,
            figures: figures(&m.role, &day, spent),
        };
        let group = if front {
            2
        } else if now.is_some() {
            0
        } else {
            1
        };
        groups[group].push(member);
    }
    groups
}

#[component]
pub(super) async fn team_pane(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let [work, resting, front] = members(&office, &extra);
    let none = work.is_empty() && resting.is_empty() && front.is_empty();
    Ok(view! {
        if none {
            quiet(text: "No one is hired yet.")
        }
        if !work.is_empty() {
            section(label: "At work", note: "today",
                <div class="card">
                    for member in work { member_row(member: member) }
                </div>
            )
        }
        if !resting.is_empty() {
            section(label: "Resting",
                <div class="card">
                    for member in resting { member_row(member: member) }
                </div>
            )
        }
        if !front.is_empty() {
            section(label: "Front desk",
                <div class="card">
                    for member in front { member_row(member: member) }
                </div>
            )
        }
        if !none {
            <p class="why m-0">"Levels come from the tickets a hire has landed."</p>
        }
        hire_link()
    })
}

#[component]
pub(super) async fn hire_link() -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <a href="/hires?new" class=(ui::BUTTON)>
                ui::icon(name: "plus")
                "Hire someone"
            </a>
        </div>
    })
}

#[component]
async fn member_row(member: Member) -> topcoat::Result<impl View> {
    let pick = select(&key("hires", &member.id));
    Ok(view! {
        <button type="button" class="member w-full border-0 bg-transparent text-left" data-on:click=(pick)>
            <span class=(format!("portrait {}", member.look))>
                if let Some((lv, _)) = member.level {
                    <span class="lv">"Lv " (lv)</span>
                }
            </span>
            <div class="who min-w-0">
                <b>(member.id.clone())</b>
                <span>(member.line.clone())</span>
                if let Some((_, percent)) = member.level {
                    <div class="xp"><i style=(format!("width: {percent:.0}%"))></i></div>
                }
            </div>
            <div class="nums">
                for (number, what) in member.figures {
                    <div><b>(number)</b><small>(what)</small></div>
                }
            </div>
        </button>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::panel::fixtures::{extra, html_of, office};
    use crate::probe::{find_containing, text_of, with};

    #[test]
    fn levels_come_from_landed_tickets() {
        assert_eq!(level(0), (1, Some((0, 2))));
        assert_eq!(level(1), (1, Some((1, 2))));
        assert_eq!(level(2), (2, Some((0, 3))));
        assert_eq!(level(4), (2, Some((2, 3))));
        assert_eq!(level(5), (3, Some((0, 5))));
        assert_eq!(level(149).0, 9);
        assert_eq!(level(150), (10, None));
        assert_eq!(level(900), (10, None));
    }

    #[tokio::test]
    async fn the_team_shows_levels_numbers_and_the_front_desk() {
        let html = html_of(office(), extra(), "").await;
        let row = |hire: &str| {
            find_containing(&html, "button", "data-on:click", &format!("hires/{hire}")).text()
        };
        // dev landed two tickets: level 2.
        let dev = row("dev");
        assert!(dev.contains("Lv 2"), "{dev}");
        assert!(dev.contains("Engineer · sonnet · on SHOP-3"), "{dev}");
        assert!(dev.contains("$4.40"), "{dev}");
        assert!(
            text_of(&html).contains("Levels come from the tickets a hire has landed."),
            "{html}"
        );
        assert!(!with(&html, "href", "/hires?new").is_empty(), "{html}");
        // The approver: ten decided today, one sent to the owner.
        let approver = row("approver");
        assert!(approver.contains("10 decided"), "{approver}");
        assert!(approver.contains("1 to you"), "{approver}");
        // The front desk has no levels.
        assert!(!approver.contains("Lv "), "{approver}");
    }
}
