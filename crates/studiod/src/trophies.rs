//! What the studio has achieved, worked out from its own history: when
//! tickets landed, and when the first goal was merged. Nothing is guessed: a
//! trophy only appears when the history shows it.

use crate::api::Trophy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use studio_store::Landing;

/// Landed totals worth a trophy.
const MILESTONES: [u64; 4] = [10, 25, 50, 100];

/// The first goal merged through the studio, remembered in the data folder
/// because git keeps no record of who merged what, or when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FirstMerge {
    pub at_ms: u64,
    pub goal: String,
    pub into: String,
}

/// A goal's text cut to a few words for a sentence about it.
fn short(goal: &str) -> String {
    const MOST: usize = 48;
    let goal = goal.trim();
    if goal.chars().count() <= MOST {
        return goal.to_string();
    }
    let cut: String = goal.chars().take(MOST).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ':', ';', '.', ' ']))
}

/// The trophies the history earns, and the next landed milestone with how
/// far along it is. `landings` is oldest first (as `Store::landings` gives
/// it) and `today` is the local day, `YYYY-MM-DD`.
pub(crate) fn build(
    landings: &[Landing],
    today: &str,
    first_merge: Option<&FirstMerge>,
) -> Vec<Trophy> {
    let mut trophies = Vec::new();
    trophies.extend(streak(landings, today));
    trophies.extend(best_day(landings));
    if let Some(merge) = first_merge {
        trophies.push(Trophy {
            id: "first-goal-merged".into(),
            title: "First goal merged".into(),
            detail: format!("“{}” was merged into {}.", short(&merge.goal), merge.into),
            earned_at: Some(merge.at_ms),
            progress: None,
        });
    }
    let total = landings.len() as u64;
    for milestone in MILESTONES {
        let title = format!("{milestone} tickets landed");
        if total >= milestone {
            trophies.push(Trophy {
                id: format!("landed-{milestone}"),
                title,
                detail: format!("The studio has landed {milestone} tickets."),
                earned_at: landings.get(milestone as usize - 1).map(|l| l.at_ms),
                progress: None,
            });
        } else {
            trophies.push(Trophy {
                id: format!("landed-{milestone}"),
                title,
                detail: format!("{} more to land.", milestone - total),
                earned_at: None,
                progress: Some((total, milestone)),
            });
            break;
        }
    }
    trophies
}

/// Days in a row with a landing, counting back from today, or from yesterday
/// when nothing has landed yet today. Two days or more make a streak.
fn streak(landings: &[Landing], today: &str) -> Option<Trophy> {
    let days: BTreeSet<i64> = landings
        .iter()
        .filter_map(|l| day_number(&l.date))
        .collect();
    let today = day_number(today)?;
    let end = [today, today - 1].into_iter().find(|d| days.contains(d))?;
    let length = (0..)
        .take_while(|back| days.contains(&(end - back)))
        .count() as u64;
    if length < 2 {
        return None;
    }
    let last = landings
        .iter()
        .rev()
        .find(|l| day_number(&l.date) == Some(end))
        .map(|l| l.at_ms);
    let detail = if end == today {
        format!("Tickets landed on {length} days in a row.")
    } else {
        format!("Tickets landed on {length} days in a row. Land one today to keep it going.")
    };
    Some(Trophy {
        id: "streak".into(),
        title: format!("{length}-day streak"),
        detail,
        earned_at: last,
        progress: None,
    })
}

/// The day with the most landings; the first to reach that many if several
/// days did.
fn best_day(landings: &[Landing]) -> Option<Trophy> {
    let mut by_day: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for landing in landings {
        by_day.entry(&landing.date).or_default().push(landing.at_ms);
    }
    let mut best: Option<(&str, &Vec<u64>)> = None;
    for (date, times) in &by_day {
        if best.is_none_or(|(_, most)| times.len() > most.len()) {
            best = Some((date, times));
        }
    }
    let (date, times) = best?;
    let count = times.len();
    let noun = if count == 1 { "ticket" } else { "tickets" };
    Some(Trophy {
        id: "best-day".into(),
        title: "Best day".into(),
        detail: format!("Landed {count} {noun} on {}.", plain_date(date)),
        // The moment the day reached its count.
        earned_at: times.last().copied(),
        progress: None,
    })
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date.
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Civil calendar to day count, with March as the first month of the year.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// `2026-10-30` as `30 Oct 2026`; the text as it came when it isn't a date.
fn plain_date(date: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return date.to_string();
    };
    let name = month
        .parse::<usize>()
        .ok()
        .and_then(|m| MONTHS.get(m.checked_sub(1)?));
    match (name, day.parse::<u32>()) {
        (Some(name), Ok(day)) => format!("{day} {name} {year}"),
        _ => date.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use studio_core::ids::TicketId;

    fn landing(n: u32, date: &str, at_ms: u64) -> Landing {
        Landing {
            ticket_id: TicketId::new(format!("T-{n}")),
            at_ms,
            date: date.to_string(),
        }
    }

    fn on(dates: &[&str]) -> Vec<Landing> {
        dates
            .iter()
            .enumerate()
            .map(|(i, d)| landing(i as u32, d, 1_000 + i as u64))
            .collect()
    }

    fn find<'a>(trophies: &'a [Trophy], id: &str) -> Option<&'a Trophy> {
        trophies.iter().find(|t| t.id == id)
    }

    #[test]
    fn day_numbers_count_days_across_months_and_leap_years() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2000-03-01"), Some(11_017));
        let jump = |a, b| day_number(b).unwrap() - day_number(a).unwrap();
        assert_eq!(jump("2024-02-28", "2024-03-01"), 2);
        assert_eq!(jump("2023-02-28", "2023-03-01"), 1);
        assert_eq!(jump("2025-12-31", "2026-01-01"), 1);
        assert_eq!(day_number("nope"), None);
        assert_eq!(day_number("2026-13-01"), None);
    }

    #[test]
    fn no_history_earns_only_the_first_milestone_in_progress() {
        let trophies = build(&[], "2026-10-30", None);
        assert_eq!(trophies.len(), 1);
        assert_eq!(trophies[0].id, "landed-10");
        assert_eq!(trophies[0].progress, Some((0, 10)));
        assert_eq!(trophies[0].earned_at, None);
    }

    #[test]
    fn a_streak_runs_to_today_or_yesterday_and_breaks_on_a_gap() {
        let landings = on(&["2026-10-27", "2026-10-29", "2026-10-30", "2026-10-31"]);
        let ending_today = build(&landings, "2026-10-31", None);
        let streak = find(&ending_today, "streak").unwrap();
        assert_eq!(streak.title, "3-day streak");
        assert_eq!(streak.earned_at, Some(1_003));
        assert!(!streak.detail.contains("today to keep"));

        let ending_yesterday = build(&landings, "2026-11-01", None);
        let streak = find(&ending_yesterday, "streak").unwrap();
        assert_eq!(streak.title, "3-day streak");
        assert!(streak.detail.contains("Land one today"));

        // Two days without a landing: the streak is over.
        assert!(find(&build(&landings, "2026-11-02", None), "streak").is_none());
        // A single day is not a streak.
        assert!(find(&build(&on(&["2026-10-31"]), "2026-10-31", None), "streak").is_none());
    }

    #[test]
    fn a_streak_crosses_month_and_year_ends() {
        let landings = on(&["2025-12-30", "2025-12-31", "2026-01-01"]);
        let trophies = build(&landings, "2026-01-01", None);
        assert_eq!(find(&trophies, "streak").unwrap().title, "3-day streak");
    }

    #[test]
    fn the_best_day_is_the_busiest_and_the_first_to_get_there() {
        let landings = on(&[
            "2026-10-01",
            "2026-10-02",
            "2026-10-02",
            "2026-10-03",
            "2026-10-03",
            "2026-10-04",
        ]);
        let trophies = build(&landings, "2026-10-20", None);
        let best = find(&trophies, "best-day").unwrap();
        assert_eq!(best.detail, "Landed 2 tickets on 2 Oct 2026.");
        // The second landing of that day.
        assert_eq!(best.earned_at, Some(1_002));
        let one = build(&on(&["2026-10-01"]), "2026-10-20", None);
        assert_eq!(
            find(&one, "best-day").unwrap().detail,
            "Landed 1 ticket on 1 Oct 2026."
        );
    }

    #[test]
    fn milestones_are_earned_when_reached_and_the_next_shows_progress() {
        let dates = vec!["2026-09-01"; 30];
        let landings = on(&dates);
        let trophies = build(&landings, "2026-10-20", None);
        let ten = find(&trophies, "landed-10").unwrap();
        assert_eq!(ten.earned_at, Some(1_009));
        assert_eq!(ten.progress, None);
        assert_eq!(find(&trophies, "landed-25").unwrap().earned_at, Some(1_024));
        let fifty = find(&trophies, "landed-50").unwrap();
        assert_eq!(fifty.earned_at, None);
        assert_eq!(fifty.progress, Some((30, 50)));
        assert_eq!(fifty.detail, "20 more to land.");
        assert!(find(&trophies, "landed-100").is_none());

        let many = on(&vec!["2026-09-01"; 100]);
        let trophies = build(&many, "2026-10-20", None);
        let hundred = find(&trophies, "landed-100").unwrap();
        assert_eq!(hundred.earned_at, Some(1_099));
        assert!(trophies.iter().all(|t| t.progress.is_none()));
    }

    #[test]
    fn the_first_merge_is_shown_only_when_it_was_recorded() {
        assert!(find(&build(&[], "2026-10-20", None), "first-goal-merged").is_none());
        let merge = FirstMerge {
            at_ms: 42,
            goal: "First level".into(),
            into: "master".into(),
        };
        let trophies = build(&[], "2026-10-20", Some(&merge));
        let trophy = find(&trophies, "first-goal-merged").unwrap();
        assert_eq!(trophy.earned_at, Some(42));
        assert_eq!(trophy.detail, "“First level” was merged into master.");
        let long = FirstMerge {
            goal: "Let notes have tags: a note added with #words in its text keeps them".into(),
            ..merge
        };
        let trophies = build(&[], "2026-10-20", Some(&long));
        assert_eq!(
            find(&trophies, "first-goal-merged").unwrap().detail,
            "“Let notes have tags: a note added with #words…” was merged into master."
        );
    }
}
