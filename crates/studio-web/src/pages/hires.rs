//! The team as a table: who each hire is, what it is on now, what it did in
//! the last 24 hours and what that cost. The approver and the router work for
//! the studio rather than on tickets, so they get a table of their own.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use studio_core::ticket::Ticket;
use studiod::api::{FileView, HireDay, OfficeView, Request};
use topcoat::view::{View, component, view};

use crate::{client, ui};

/// Roles that work for the studio itself.
const STUDIO_ROLES: [&str; 2] = ["approver", "router"];
/// How much of a prompt's first line the table shows.
const ABOUT_CHARS: usize = 60;

/// The hire fields the table shows.
#[derive(Clone, Debug, Deserialize)]
struct Info {
    role: String,
    model: String,
    #[serde(default)]
    escalate: Vec<String>,
    #[serde(default)]
    effort: Option<String>,
}

/// The fields, and the prompt's first line.
fn parse(text: &str) -> Result<(Info, String), String> {
    let (yaml, prompt) = studio_config::frontmatter::split(text)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "missing frontmatter".to_string())?;
    let info = serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())?;
    let first = prompt.lines().map(str::trim).find(|l| !l.is_empty());
    let about = crate::summary::truncate(first.unwrap_or_default(), ABOUT_CHARS);
    Ok((info, about))
}

/// One hire's row.
#[derive(Clone, Debug)]
pub struct Hire {
    /// The file name, which is also how tickets name the hire.
    name: String,
    /// The fields and the prompt's first line, or why the file doesn't load.
    info: Result<(Info, String), String>,
    /// The tickets it is on now, as (id, state).
    now: Vec<(String, &'static str)>,
    /// What it did in the last 24 hours; `None` when studiod can't say.
    today: Option<HireDay>,
    /// What it spent in the last 24 hours; `None` when studiod can't say.
    spent_today: Option<f64>,
}

impl Hire {
    fn works_for_studio(&self) -> bool {
        self.info
            .as_ref()
            .is_ok_and(|(info, _)| STUDIO_ROLES.contains(&info.role.as_str()))
    }
}

/// The last 24 hours' spend by hire; `None` if studiod can't say.
async fn spent_by_hire(socket: &Path) -> Option<BTreeMap<String, f64>> {
    #[derive(Default, Deserialize)]
    struct Budget {
        spent_24h: Spent,
    }
    #[derive(Default, Deserialize)]
    struct Spent {
        #[serde(default)]
        by_hire: BTreeMap<String, f64>,
    }
    client::call::<Budget>(socket, Request::Budget)
        .await
        .map(|b| b.spent_24h.by_hire)
        .ok()
}

/// The rows for `files`, with the daemon's tickets, spend and today's work.
pub async fn hires_for(socket: &Path, files: &[FileView]) -> Vec<Hire> {
    // The tickets, and who judges each one in review, as the office has them.
    let office: Option<OfficeView> = client::call(socket, Request::Office).await.ok();
    let tickets = office
        .as_ref()
        .map(|o| o.tickets.clone())
        .unwrap_or_default();
    let spent = spent_by_hire(socket).await;
    let today: Option<BTreeMap<String, HireDay>> =
        client::call(socket, Request::HiresToday).await.ok();
    let reviewers: HashMap<String, String> = office
        .into_iter()
        .flat_map(|o| o.judges)
        .map(|(id, judge)| (id, judge.hire))
        .collect();
    hires_of(files, &tickets, spent.as_ref(), today.as_ref(), &reviewers)
}

fn hires_of(
    files: &[FileView],
    tickets: &[Ticket],
    spent: Option<&BTreeMap<String, f64>>,
    today: Option<&BTreeMap<String, HireDay>>,
    reviewers: &HashMap<String, String>,
) -> Vec<Hire> {
    // A ticket in review is the reviewer's for now.
    let on = |ticket: &Ticket| {
        let id = ticket.id.to_string();
        reviewers
            .get(&id)
            .cloned()
            .unwrap_or_else(|| ticket.hire.to_string())
    };
    files
        .iter()
        .map(|file| Hire {
            name: file.name.clone(),
            info: match &file.problem {
                Some(problem) => Err(problem.clone()),
                None => parse(&file.text),
            },
            now: tickets
                .iter()
                .filter(|t| ui::run_under_way(t.state.kind()))
                .filter(|t| on(t) == file.name)
                .map(|t| (t.id.to_string(), t.state.kind()))
                .collect(),
            today: today.map(|days| days.get(&file.name).cloned().unwrap_or_default()),
            spent_today: spent.map(|s| s.get(&file.name).copied().unwrap_or(0.0)),
        })
        .collect()
}

/// "1 review", "3 reviews".
fn count(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a hire did, in a few words; empty when it did nothing.
pub fn did(day: &HireDay) -> String {
    let mut parts = Vec::new();
    if day.done > 0 {
        parts.push(format!("{} done", day.done));
    }
    if day.planned > 0 {
        parts.push(count(day.planned, "goal planned", "goals planned"));
    }
    if day.reviews > 0 {
        parts.push(count(day.reviews, "review", "reviews"));
    }
    if day.critiques > 0 {
        parts.push(count(day.critiques, "critique", "critiques"));
    }
    let calls = day.allowed + day.denied + day.to_owner;
    if calls > 0 {
        parts.push(format!(
            "{}: {} allowed, {} denied, {} sent to you",
            count(calls, "call", "calls"),
            day.allowed,
            day.denied,
            day.to_owner
        ));
    }
    if day.routed > 0 {
        parts.push(count(day.routed, "ticket routed", "tickets routed"));
    }
    parts.join(" · ")
}

/// "Medium" for `medium`.
fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Columns left out on a phone; the hire's second line has the model and
/// spend.
const PHONE_HIDDEN: &str = "max-[760px]:hidden";
/// Columns left out below a wide screen, so the table fits a laptop.
const NARROW_HIDDEN: &str = "max-[1180px]:hidden";
/// Columns left out below a laptop's width.
const MID_HIDDEN: &str = "max-[900px]:hidden";

/// The team's table, then the studio's own hires.
#[component]
pub async fn tables(list: Vec<Hire>) -> topcoat::Result<impl View> {
    let (studio, team): (Vec<Hire>, Vec<Hire>) = list.into_iter().partition(Hire::works_for_studio);
    let (team_count, studio_count) = (team.len() as u64, studio.len() as u64);
    Ok(view! {
        <section>
            ui::section_head(title: "Team", count: Some(team_count), href: String::new(), label: String::new())
            if team.is_empty() {
                <p class=(ui::MUTED)>"No hires yet. Add one with New hire."</p>
            } else {
                <div class=(ui::TABLE_BOX)>
                    <table class=(ui::TABLE)>
                        <thead>
                            <tr>
                                <th class=(ui::TH)>"Hire"</th>
                                <th class=(format!("{} {PHONE_HIDDEN}", ui::TH))>"Model"</th>
                                <th class=(format!("{} {NARROW_HIDDEN}", ui::TH))>"Effort"</th>
                                <th class=(ui::TH)>"Now"</th>
                                <th class=(format!("{} {MID_HIDDEN}", ui::TH))>"Last 24h"</th>
                                <th class=(format!("{} text-right {PHONE_HIDDEN}", ui::TH))>"Spent" <span class=(NARROW_HIDDEN)>", last 24h"</span></th>
                                <th class=(format!("{} {PHONE_HIDDEN}", ui::TH))></th>
                            </tr>
                        </thead>
                        <tbody>
                            for hire in team { hire_row(hire: hire, staff: false) }
                        </tbody>
                    </table>
                </div>
            }
        </section>
        if !studio.is_empty() {
            <section>
                ui::section_head(title: "Working for the studio", count: Some(studio_count), href: String::new(), label: String::new())
                <div class=(ui::TABLE_BOX)>
                    <table class=(ui::TABLE)>
                        <thead>
                            <tr>
                                <th class=(ui::TH)>"Hire"</th>
                                <th class=(format!("{} {PHONE_HIDDEN}", ui::TH))>"Model"</th>
                                <th class=(ui::TH)>"Last 24h"</th>
                                <th class=(format!("{} text-right {PHONE_HIDDEN}", ui::TH))>"Spent, last 24h"</th>
                                <th class=(format!("{} {PHONE_HIDDEN}", ui::TH))></th>
                            </tr>
                        </thead>
                        <tbody>
                            for hire in studio { hire_row(hire: hire, staff: true) }
                        </tbody>
                    </table>
                </div>
            </section>
        }
    })
}

/// One hire. A studio hire (`staff`) has no effort or tickets to show.
#[component]
async fn hire_row(hire: Hire, staff: bool) -> topcoat::Result<impl View> {
    let edit = format!("/hires?file={}", hire.name);
    let today = hire.today.as_ref().map(did);
    let more = hire.now.len().saturating_sub(1);
    let first = hire.now.first().cloned();
    let (info, about, problem) = match hire.info {
        Ok((info, about)) => (Some(info), about, None),
        Err(problem) => (None, String::new(), Some(problem)),
    };
    let effort = info
        .as_ref()
        .and_then(|i| i.effort.as_deref().map(capitalised));
    // The model and spend, for the second line on a phone.
    let short = [
        info.as_ref().map(|i| i.model.clone()),
        hire.spent_today.map(ui::usd),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Ok(view! {
        <tr class=(ui::TR)>
            <td class=(ui::TD)>
                <span class="flex items-center gap-[7px]">
                    ui::avatar(hire: hire.name.clone(), staff: staff)
                    <span class="flex min-w-0 flex-col">
                        <a href=(edit.clone()) class="font-medium leading-[18px] decoration-line-3 underline-offset-3 hover:underline">(hire.name.clone())</a>
                        if let Some(problem) = problem {
                            <span class="max-w-[220px] truncate text-[12px] leading-4 text-bad-fg max-[760px]:max-w-[160px]" title=(problem.clone())>"Doesn't load: " (problem)</span>
                        } else {
                            if !about.is_empty() {
                                <span class="max-w-[220px] truncate text-[12px] leading-4 text-fg-3 max-[1180px]:max-w-[150px] max-[760px]:hidden" title=(about.clone())>(about)</span>
                            }
                            <span class="hidden text-[12px] leading-4 text-fg-3 max-[760px]:block">(short)</span>
                        }
                    </span>
                </span>
            </td>
            <td class=(format!("{} {PHONE_HIDDEN}", ui::TD))>
                if let Some(info) = &info {
                    <span class="flex items-center gap-2">
                        ui::model_tag(model: info.model.clone())
                        // What it escalates to, where there is room.
                        for model in info.escalate.clone() {
                            <span class="contents max-[1180px]:hidden">
                                <span class="text-fg-3">ui::icon(name: "arrow-right", small: true)</span>
                                ui::model_tag(model: model)
                            </span>
                        }
                    </span>
                }
            </td>
            if !staff {
                <td class=(format!("{} text-fg-2 {NARROW_HIDDEN}", ui::TD))>
                    if let Some(effort) = effort {
                        (effort)
                    } else if info.is_some() {
                        <span class="text-fg-3">"Default"</span>
                    }
                </td>
                <td class=(ui::TD)>
                    if let Some((id, state)) = first {
                        <span class="flex items-center gap-2">
                            ui::state_icon(state: state, small: true)
                            <a href=(format!("/tickets/{id}")) class="hover:underline">(id)</a>
                            if more > 0 {
                                <span class="text-fg-3 max-[1180px]:hidden">"and " (more) " more"</span>
                                <span class="hidden text-fg-3 max-[1180px]:inline">"+" (more)</span>
                            }
                        </span>
                    } else {
                        <span class="text-fg-3">"Idle"</span>
                    }
                </td>
            }
            <td class=(format!("{} text-fg-2 {}", ui::TD, if staff { "max-[760px]:whitespace-normal" } else { MID_HIDDEN }))>
                if let Some(text) = today {
                    if text.is_empty() {
                        <span class="text-fg-3">"Nothing yet"</span>
                    } else {
                        (text)
                    }
                }
            </td>
            <td class=(format!("{} text-right tabular-nums {PHONE_HIDDEN}", ui::TD))>
                if let Some(spent) = hire.spent_today { (ui::usd(spent)) }
            </td>
            // The name links to the editor too, so this goes on a phone.
            <td class=(format!("{} w-px text-right {PHONE_HIDDEN}", ui::TD))>
                <a href=(edit) class=(ui::GHOST_SM) aria-label=(format!("Edit {}", hire.name)) title="Edit">
                    <span class="hidden max-[1180px]:inline-flex">ui::icon(name: "square-pen", small: true)</span>
                    <span class=(NARROW_HIDDEN)>"Edit"</span>
                </a>
            </td>
        </tr>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use topcoat::context::Cx;

    async fn render(list: Vec<Hire>) -> String {
        let cx = Cx::default();
        ui::html(&cx, view! { cx => tables(list: list) })
            .await
            .unwrap()
    }

    fn file(name: &str, text: &str) -> FileView {
        FileView {
            name: name.into(),
            text: text.into(),
            problem: None,
        }
    }

    fn ticket(id: &str, hire: &str, state: &str) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": "t", "goal": "", "hire": hire,
            "state": state, "attempts": 1, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    #[test]
    fn reads_a_hire_file_and_its_prompts_first_line() {
        let text = "---\nname: Engineer\nrole: engineer\nruntime: claude-code\nmodel: sonnet\nescalate: [opus]\neffort: high\n---\n\nWrites code.\nCarefully.\n";
        let (info, about) = parse(text).unwrap();
        assert_eq!(
            (info.model.as_str(), info.escalate.as_slice()),
            ("sonnet", &["opus".to_string()][..])
        );
        assert_eq!(info.effort.as_deref(), Some("high"));
        assert_eq!(about, "Writes code.");
        assert!(parse("oops").is_err());
    }

    #[test]
    fn a_ticket_in_review_is_on_its_reviewer() {
        let files = [
            file("dev", "---\nrole: engineer\nmodel: sonnet\n---\n"),
            file("critic", "---\nrole: reviewer\nmodel: opus\n---\n"),
        ];
        let tickets = [
            ticket("GAME-1", "dev", "working"),
            ticket("GAME-2", "dev", "reviewing"),
            ticket("GAME-3", "dev", "landed"),
        ];
        let reviewers = HashMap::from([("GAME-2".to_string(), "critic".to_string())]);
        let hires = hires_of(&files, &tickets, None, None, &reviewers);
        assert_eq!(hires[0].now, [("GAME-1".to_string(), "working")]);
        assert_eq!(hires[1].now, [("GAME-2".to_string(), "reviewing")]);
        assert!(hires[0].today.is_none());
    }

    #[test]
    fn what_a_hire_did_reads_as_a_short_line() {
        let day = HireDay {
            done: 6,
            planned: 1,
            reviews: 1,
            ..HireDay::default()
        };
        assert_eq!(did(&day), "6 done · 1 goal planned · 1 review");
        let approver = HireDay {
            allowed: 81,
            denied: 11,
            to_owner: 4,
            ..HireDay::default()
        };
        assert_eq!(
            did(&approver),
            "96 calls: 81 allowed, 11 denied, 4 sent to you"
        );
        assert_eq!(did(&HireDay::default()), "");
    }

    #[tokio::test]
    async fn the_studios_hires_get_their_own_table() {
        let files = [
            file(
                "dev",
                "---\nrole: engineer\nmodel: sonnet\neffort: medium\n---\nWrites code.\n",
            ),
            file(
                "approver",
                "---\nrole: approver\nmodel: haiku\n---\nDecides tool calls.\n",
            ),
            FileView {
                name: "broken".into(),
                text: String::new(),
                problem: Some("line 3: <bad>".into()),
            },
        ];
        let today = BTreeMap::from([(
            "approver".to_string(),
            HireDay {
                denied: 2,
                ..HireDay::default()
            },
        )]);
        let spent = BTreeMap::from([("dev".to_string(), 1.2)]);
        let tickets = [
            ticket("GAME-1", "dev", "working"),
            ticket("GAME-2", "dev", "checking"),
        ];
        let html = render(hires_of(
            &files,
            &tickets,
            Some(&spent),
            Some(&today),
            &HashMap::new(),
        ))
        .await;
        let (team, studio) = html.split_once("Working for the studio").unwrap();
        assert!(
            team.contains("Writes code.") && team.contains("Medium"),
            "{team}"
        );
        assert!(
            team.contains("/tickets/GAME-1") && team.contains("and 1 more"),
            "{team}"
        );
        assert!(
            team.contains("$1.20") && team.contains("Nothing yet"),
            "{team}"
        );
        assert!(team.contains("load: line 3: &lt;bad&gt;"), "{team}");
        assert!(
            studio.contains("2 calls: 0 allowed, 2 denied, 0 sent to you"),
            "{studio}"
        );
        assert!(
            studio.contains("/hires?file=approver") && !studio.contains("Idle"),
            "{studio}"
        );
    }
}
