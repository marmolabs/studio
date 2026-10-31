//! One card per hire: what it is doing now, what it has done, and who it is.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use studio_core::ticket::Ticket;
use studiod::api::{FileView, Request};
use topcoat::view::{View, component, view};

use super::ticket::log_tail;
use crate::summary::latest_activity;
use crate::{client, ui};

const ACTIVITY_SHOWN: usize = 80;

/// The hire fields the cards show; the rest of the file is the prompt.
#[derive(Clone, Debug, Deserialize)]
struct Info {
    name: String,
    role: String,
    model: String,
    #[serde(default)]
    escalate: Vec<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    budget_per_ticket_usd: Option<f64>,
    #[serde(default)]
    daily_usd: Option<f64>,
    #[serde(default)]
    tools: Vec<String>,
}

fn parse(text: &str) -> Result<Info, String> {
    let (yaml, _) = studio_config::frontmatter::split(text)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "missing frontmatter".to_string())?;
    serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())
}

/// A ticket a hire has in hand.
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    id: String,
    title: String,
    state: &'static str,
    /// What the agent did last, for a ticket that is running.
    activity: Option<String>,
}

/// What a hire has done, from the tickets list and today's spend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// Every ticket the hire has had.
    tickets: usize,
    /// Landed, or ready to land.
    done: usize,
    failed: usize,
    spent_today: f64,
}

impl Record {
    /// No tickets and no spend today. Reviewers and approvers spend without owning tickets.
    fn is_new(&self) -> bool {
        self.tickets == 0 && self.spent_today == 0.0
    }

    /// Done as a whole percent of done plus failed; `None` before any has finished.
    fn pass_rate(&self) -> Option<u32> {
        let finished = self.done + self.failed;
        (finished > 0).then(|| (self.done as f64 * 100.0 / finished as f64).round() as u32)
    }
}

/// Counts the tickets of `hire` by outcome and attaches `spent_today`.
fn record_of(hire: &str, tickets: &[Ticket], spent_today: f64) -> Record {
    let mut record = Record {
        spent_today,
        ..Record::default()
    };
    for ticket in tickets.iter().filter(|t| t.hire.to_string() == hire) {
        record.tickets += 1;
        match ticket.state.kind() {
            "landed" | "ready" => record.done += 1,
            "failed" => record.failed += 1,
            _ => {}
        }
    }
    record
}

#[derive(Clone, Debug)]
pub struct HireCard {
    /// The file name, which is also how tickets name the hire.
    file: String,
    info: Result<Info, String>,
    work: Vec<Work>,
    record: Record,
}

/// Today's spend by hire; empty if studiod can't say.
async fn spent_by_hire(socket: &Path) -> BTreeMap<String, f64> {
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
        .unwrap_or_default()
}

/// The cards for `files`, with the daemon's tickets, spend and run logs.
pub async fn cards_for(socket: &Path, data_dir: &Path, files: &[FileView]) -> Vec<HireCard> {
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets)
        .await
        .unwrap_or_default();
    let spent = spent_by_hire(socket).await;
    let mut activity = HashMap::new();
    for ticket in tickets.iter().filter(|t| t.state.kind() == "working") {
        let id = ticket.id.to_string();
        let tail = log_tail(data_dir, &id).await;
        if let Some(line) = latest_activity(&tail, ACTIVITY_SHOWN) {
            activity.insert(id, line);
        }
    }
    cards_of(files, &tickets, &spent, &activity)
}

fn cards_of(
    files: &[FileView],
    tickets: &[Ticket],
    spent: &BTreeMap<String, f64>,
    activity: &HashMap<String, String>,
) -> Vec<HireCard> {
    files
        .iter()
        .map(|file| HireCard {
            file: file.name.clone(),
            info: match &file.problem {
                Some(problem) => Err(problem.clone()),
                None => parse(&file.text),
            },
            record: record_of(
                &file.name,
                tickets,
                spent.get(&file.name).copied().unwrap_or(0.0),
            ),
            work: tickets
                .iter()
                .filter(|t| t.hire.to_string() == file.name)
                .filter(|t| {
                    matches!(
                        t.state.kind(),
                        "claimed" | "working" | "checking" | "reviewing" | "asking"
                    )
                })
                .map(|t| Work {
                    id: t.id.to_string(),
                    title: t.title.clone(),
                    state: t.state.kind(),
                    activity: activity.get(&t.id.to_string()).cloned(),
                })
                .collect(),
        })
        .collect()
}

#[component]
pub async fn cards(list: Vec<HireCard>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="hire-cards" class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
            if list.is_empty() {
                <div class="rounded-lg border border-dashed border-zinc-200 px-3 py-8 text-center text-zinc-600 sm:col-span-2 lg:col-span-3 dark:border-zinc-800 dark:text-zinc-400">
                    "No hires yet. Add one with New."
                </div>
            }
            for card in list { hire_card(card: card) }
        </div>
    })
}

#[component]
async fn hire_card(card: HireCard) -> topcoat::Result<impl View> {
    let edit = format!("/hires?file={}", card.file);
    let (info, problem) = match card.info {
        Ok(info) => (Some(info), None),
        Err(problem) => (None, Some(problem)),
    };
    let budgets = info.as_ref().map(|i| {
        let mut parts = Vec::new();
        if let Some(usd) = i.budget_per_ticket_usd {
            parts.push(format!("{} per ticket", ui::usd(usd)));
        }
        if let Some(usd) = i.daily_usd {
            parts.push(format!("{} a day", ui::usd(usd)));
        }
        parts.join(" · ")
    });
    Ok(view! {
        <article class=(format!("flex min-w-0 flex-col p-3 {}", ui::CARD))>
            if let Some(info) = info {
                <div class="flex items-center gap-2">
                    <h2 class=(format!("min-w-0 truncate {}", ui::SECTION_TITLE))>(info.name)</h2>
                    <span class="shrink-0 rounded bg-indigo-50 px-1.5 py-px text-[11px] font-medium text-indigo-700 dark:bg-indigo-950 dark:text-indigo-300">(info.role)</span>
                    <a href=(edit.clone()) class="ml-auto shrink-0 text-[12px] text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100">"Edit"</a>
                </div>
                <dl class="mt-2 grid grid-cols-[4.5rem_minmax(0,1fr)] gap-x-2 gap-y-1 text-[12px]">
                    <dt class="text-zinc-500 dark:text-zinc-400">"Model"</dt>
                    <dd class="font-mono">
                        (info.model)
                        if !info.escalate.is_empty() {
                            <span class="text-zinc-500 dark:text-zinc-400">" → " (info.escalate.join(" → "))</span>
                        }
                    </dd>
                    if let Some(effort) = info.effort {
                        <dt class="text-zinc-500 dark:text-zinc-400">"Effort"</dt>
                        <dd>(effort)</dd>
                    }
                    if let Some(budgets) = budgets.filter(|b| !b.is_empty()) {
                        <dt class="text-zinc-500 dark:text-zinc-400">"Budget"</dt>
                        <dd>(budgets)</dd>
                    }
                    if !info.tools.is_empty() {
                        <dt class="text-zinc-500 dark:text-zinc-400">"Tools"</dt>
                        <dd class="font-mono">(info.tools.join(", "))</dd>
                    }
                </dl>
            }
            if let Some(problem) = problem {
                <div class="flex items-center gap-2">
                    <h2 class="min-w-0 truncate font-mono text-[14px] font-semibold">(card.file.clone())</h2>
                    <span class="shrink-0 rounded bg-rose-100 px-1 text-[11px] font-semibold text-rose-800 dark:bg-rose-950 dark:text-rose-300">"broken"</span>
                    <a href=(edit.clone()) class="ml-auto shrink-0 text-[12px] text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-100">"Edit"</a>
                </div>
                <p class="mt-2 break-words text-[12px] text-rose-700 dark:text-rose-400">(problem)</p>
            }
            <div class="mt-3 border-t border-zinc-100 pt-2 text-[12px] dark:border-zinc-800">
                if card.work.is_empty() && !card.record.is_new() {
                    <span class=(ui::MUTED)>"Idle"</span>
                }
                for work in card.work {
                    <a href=(format!("/tickets/{}", work.id)) class="block py-0.5 hover:text-zinc-900 dark:hover:text-zinc-100">
                        <span class="flex items-center gap-2">
                            <span class="shrink-0 font-mono text-zinc-600 dark:text-zinc-400">(work.id.clone())</span>
                            <span class="min-w-0 flex-1 truncate" title=(work.title.clone())>(work.title)</span>
                            ui::state_pill(state: work.state)
                        </span>
                        if let Some(line) = work.activity {
                            <span class=(format!("block truncate {}", ui::MUTED))>(line)</span>
                        }
                    </a>
                }
                <p class=(ui::MUTED)>
                    if card.record.is_new() {
                        "No tickets yet"
                    } else {
                        <span class=(ui::SECONDARY)>(card.record.done)</span> " done · "
                        <span class=(ui::SECONDARY)>(ui::usd(card.record.spent_today))</span> " today"
                        if let Some(rate) = card.record.pass_rate() {
                            " · " <span class=(ui::SECONDARY)>(rate) "%"</span> " pass rate"
                        }
                    }
                </p>
            </div>
        </article>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use topcoat::context::Cx;

    async fn render(card: HireCard) -> String {
        let cx = Cx::default();
        ui::html(&cx, view! { cx => hire_card(card: card) })
            .await
            .unwrap()
    }

    fn card(info: Result<Info, String>, work: Vec<Work>, record: Record) -> HireCard {
        HireCard {
            file: "dev".into(),
            info,
            work,
            record,
        }
    }

    fn info() -> Info {
        parse("---\nname: Rev\nrole: reviewer\nmodel: sonnet\n---\n").unwrap()
    }

    #[test]
    fn reads_a_hire_file() {
        let text = "---\nname: Engineer\nrole: engineer\nruntime: claude-code\nmodel: sonnet\nescalate: [opus]\neffort: high\nbudget_per_ticket_usd: 0.5\ntools: [file_ticket]\n---\nPrompt.\n";
        let info = parse(text).unwrap();
        assert_eq!(info.name, "Engineer");
        assert_eq!(info.escalate, ["opus"]);
        assert_eq!(info.budget_per_ticket_usd, Some(0.5));
        assert!(parse("oops").is_err());
    }

    fn ticket(id: &str, hire: &str, state: serde_json::Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "game", "title": "t", "goal": "", "hire": hire,
            "state": state, "attempts": 1, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    #[test]
    fn a_record_counts_outcomes_and_the_pass_rate() {
        let failed = json!({"failed": {"class": "checks", "detail": ""}});
        let tickets = [
            ticket("GAME-1", "dev", json!("landed")),
            ticket("GAME-2", "dev", json!("ready")),
            ticket("GAME-3", "dev", json!("landed")),
            ticket("GAME-4", "dev", failed),
            ticket("GAME-5", "dev", json!("working")),
            ticket("GAME-6", "artist", json!("landed")),
        ];
        let dev = record_of("dev", &tickets, 1.5);
        assert_eq!((dev.tickets, dev.done, dev.failed), (5, 3, 1));
        assert_eq!(dev.spent_today, 1.5);
        assert_eq!(dev.pass_rate(), Some(75));
        assert_eq!(record_of("artist", &tickets, 0.0).pass_rate(), Some(100));
        // Nothing finished yet, or no tickets at all: no rate.
        assert_eq!(record_of("dev", &tickets[4..5], 0.0).pass_rate(), None);
        assert_eq!(record_of("nobody", &tickets, 0.0), Record::default());
    }

    #[tokio::test]
    async fn a_broken_hire_keeps_its_work_and_spend() {
        let work = Work {
            id: "GAME-1".into(),
            title: "Add a menu".into(),
            state: "working",
            activity: Some("Running tests".into()),
        };
        let record = Record {
            tickets: 1,
            spent_today: 1.2,
            ..Record::default()
        };
        let html = render(card(Err("bad yaml".into()), vec![work], record)).await;
        assert!(html.contains("broken"), "{html}");
        assert!(html.contains(r#"href="/tickets/GAME-1""#), "{html}");
        assert!(html.contains("Running tests"), "{html}");
        assert!(html.contains("$1.20"), "{html}");
    }

    #[tokio::test]
    async fn spend_without_tickets_is_shown() {
        let record = Record {
            spent_today: 1.2,
            ..Record::default()
        };
        let html = render(card(Ok(info()), Vec::new(), record)).await;
        assert!(html.contains("Idle"), "{html}");
        assert!(html.contains("$1.20"), "{html}");
        assert!(!html.contains("No tickets yet"), "{html}");
    }

    #[tokio::test]
    async fn a_new_hire_gets_one_quiet_line() {
        let html = render(card(Ok(info()), Vec::new(), Record::default())).await;
        assert!(html.contains("No tickets yet"), "{html}");
        assert!(!html.contains("Idle"), "{html}");
    }
}
