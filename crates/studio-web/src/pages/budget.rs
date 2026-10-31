use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use studio_core::ticket::Ticket;
use studiod::api::Request;
use topcoat::{
    context::{Cx, app_context},
    router::page,
    view::{View, component, view},
};

use crate::{AppConfig, client, ui};

#[derive(Deserialize)]
struct Budget {
    limits: Limits,
    spent_24h: Spent,
    runs_24h: u64,
    #[serde(default)]
    reserved: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Limits {
    daily_usd: f64,
    ticket_usd: f64,
    run_usd: Option<f64>,
    min_run_usd: f64,
    #[serde(default)]
    projects: BTreeMap<String, f64>,
    #[serde(default)]
    hires: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Spent {
    total: f64,
    #[serde(default)]
    by_project: BTreeMap<String, f64>,
    #[serde(default)]
    by_hire: BTreeMap<String, f64>,
}

/// One "spent of limit" row.
struct Row {
    name: String,
    spent: f64,
    limit: Option<f64>,
}

/// Everything that has a limit or spent money, biggest spender first.
fn rows(spent: &BTreeMap<String, f64>, limits: &BTreeMap<String, f64>) -> Vec<Row> {
    let names: BTreeSet<&String> = spent.keys().chain(limits.keys()).collect();
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|name| Row {
            name: name.clone(),
            spent: spent.get(name).copied().unwrap_or(0.0),
            limit: limits.get(name).copied(),
        })
        .collect();
    rows.sort_by(|a, b| b.spent.total_cmp(&a.spent));
    rows
}

/// Today's money against the daily limit: what is spent, what live runs hold
/// back, and what is left for new runs.
#[derive(Debug, PartialEq)]
struct Headline {
    spent: f64,
    reserved: f64,
    left: f64,
    /// Spent as a share of the limit, from 0 to 1.
    spent_share: f64,
    /// Spent plus reserved as a share of the limit, from 0 to 1.
    used_share: f64,
}

fn headline(spent: f64, reserved: &BTreeMap<String, f64>, daily: f64) -> Headline {
    let reserved: f64 = reserved.values().sum();
    let share = |usd: f64| {
        if daily > 0.0 {
            (usd / daily).clamp(0.0, 1.0)
        } else {
            1.0
        }
    };
    Headline {
        spent,
        reserved,
        left: (daily - spent - reserved).max(0.0),
        spent_share: share(spent),
        used_share: share(spent + reserved),
    }
}

/// The limits as one line; an unset one is worded, not shown as "none".
fn limits_line(limits: &Limits, runs: u64) -> String {
    let run = limits.run_usd.map_or_else(|| "no cap".to_string(), ui::usd);
    format!(
        "Per ticket {} · Per run: {run} · Smallest run {} · {runs} {} today",
        ui::usd(limits.ticket_usd),
        ui::usd(limits.min_run_usd),
        if runs == 1 { "run" } else { "runs" },
    )
}

#[page("/budget")]
async fn budget(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let b: Budget = client::call(&config.socket, Request::Budget).await?;
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets)
        .await
        .unwrap_or_default();
    let titles: HashMap<String, String> = tickets
        .into_iter()
        .map(|t| (t.id.to_string(), t.title))
        .collect();
    let head = headline(b.spent_24h.total, &b.reserved, b.limits.daily_usd);
    let projects = rows(&b.spent_24h.by_project, &b.limits.projects);
    let hires = rows(&b.spent_24h.by_hire, &b.limits.hires);
    let line = limits_line(&b.limits, b.runs_24h);
    let bar = bar_tone(head.used_share);
    let reserved: Vec<(String, String, f64)> = b
        .reserved
        .into_iter()
        .map(|(id, usd)| (titles.get(&id).cloned().unwrap_or_default(), id, usd))
        .collect();

    Ok(view! {
        <h1 class=(format!("mb-3 {}", ui::PAGE_TITLE))>"Budget"</h1>
        <section class=(format!("p-3 {}", ui::CARD))>
            <p class="flex flex-wrap items-baseline gap-x-2">
                <span class="text-2xl font-semibold tabular-nums">(ui::usd(head.spent + head.reserved))</span>
                <span class=(ui::SECONDARY)>"of " (ui::usd(b.limits.daily_usd)) " daily limit, spent or held"</span>
            </p>
            <div class="mt-2 flex h-2 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                <div class=(format!("h-full {bar}")) style=(format!("width: {:.1}%", head.spent_share * 100.0))></div>
                <div class="h-full bg-indigo-300 dark:bg-indigo-800" style=(format!("width: {:.1}%", (head.used_share - head.spent_share) * 100.0))></div>
            </div>
            <p class=(format!("mt-2 text-[12px] {}", ui::MUTED))>
                <span class=(ui::SECONDARY)>(ui::usd(head.spent))</span> " spent by finished runs · "
                <span class=(ui::SECONDARY)>(ui::usd(head.reserved))</span> " held for live runs, an upper bound until they end · "
                <span class=(ui::SECONDARY)>(ui::usd(head.left))</span> " left for new runs"
            </p>
        </section>
        <p class=(format!("mb-1 mt-3 text-[12px] {}", ui::MUTED))>(line)</p>
        <p class=(format!("mb-2 text-[12px] {}", ui::MUTED))>
            "Spend is the last 24 hours. A run only starts if its allowance fits under every limit."
        </p>
        table(title: "By project", rows: projects)
        table(title: "By hire", rows: hires)
        if !reserved.is_empty() {
            <h2 class=(format!("mb-1 mt-4 text-[12px] font-medium {}", ui::MUTED))>"Held for live runs"</h2>
            <ul class=(format!("divide-y divide-zinc-200 dark:divide-zinc-800 {}", ui::CARD))>
                for (title, id, usd) in reserved {
                    <li class="flex items-baseline gap-2 px-3 py-1.5">
                        <a href=(format!("/tickets/{id}")) class="flex min-w-0 flex-1 items-baseline gap-2 hover:text-zinc-900 dark:hover:text-zinc-100">
                            <span class="shrink-0 font-mono text-[12px]">(id.clone())</span>
                            <span class=(format!("min-w-0 truncate {}", ui::SECONDARY)) title=(title.clone())>(title)</span>
                        </a>
                        <span class="shrink-0 tabular-nums">(ui::usd(usd))</span>
                    </li>
                }
            </ul>
        }
    })
}

/// Red near the limit, amber approaching it. Literal classes so Tailwind finds them.
fn bar_tone(share: f64) -> &'static str {
    if share >= 0.9 {
        "bg-rose-500"
    } else if share >= 0.7 {
        "bg-amber-500"
    } else {
        "bg-indigo-500"
    }
}

#[component]
async fn table(title: &'static str, rows: Vec<Row>) -> topcoat::Result<impl View> {
    Ok(view! {
        if !rows.is_empty() {
            <h2 class=(format!("mb-1 mt-4 text-[12px] font-medium {}", ui::MUTED))>(title)</h2>
            <div class=(format!("divide-y divide-zinc-200 dark:divide-zinc-800 {}", ui::CARD))>
                for row in rows {
                    let share = row.limit.filter(|l| *l > 0.0).map(|l| (row.spent / l).clamp(0.0, 1.0));
                    <div class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 px-3 py-1.5 sm:grid-cols-[12rem_minmax(0,1fr)_9rem]">
                        <span class="truncate font-medium" title=(row.name.clone())>(row.name)</span>
                        <span class="text-right tabular-nums sm:order-last">
                            (ui::usd(row.spent))
                            <span class=(ui::MUTED)>
                                if let Some(limit) = row.limit {
                                    " of " (ui::usd(limit))
                                } else {
                                    " · no limit"
                                }
                            </span>
                        </span>
                        if let Some(share) = share {
                            <div class="col-span-2 h-1.5 overflow-hidden rounded-full bg-zinc-100 sm:col-span-1 dark:bg-zinc-800">
                                <div class=(format!("h-full rounded-full {}", bar_tone(share))) style=(format!("width: {:.1}%", share * 100.0))></div>
                            </div>
                        } else {
                            <div class="hidden sm:block"></div>
                        }
                    </div>
                }
            </div>
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headline_counts_held_money_as_used() {
        let reserved = BTreeMap::from([("GAME-1".to_string(), 0.5), ("GAME-2".to_string(), 0.25)]);
        let h = headline(6.0, &reserved, 10.0);
        assert_eq!((h.spent, h.reserved, h.left), (6.0, 0.75, 3.25));
        assert_eq!((h.spent_share, h.used_share), (0.6, 0.675));
        // Nothing spent yet, but a live run holds an allowance.
        let h = headline(0.0, &reserved, 10.0);
        assert_eq!((h.spent, h.reserved), (0.0, 0.75));
        // Over the limit: nothing left and the bar stops at full.
        let h = headline(12.0, &reserved, 10.0);
        assert_eq!((h.left, h.used_share), (0.0, 1.0));
    }

    #[test]
    fn rows_put_the_biggest_spender_first() {
        let spent = BTreeMap::from([("a".to_string(), 1.0), ("b".to_string(), 3.0)]);
        let limits = BTreeMap::from([("c".to_string(), 2.0)]);
        let names: Vec<_> = rows(&spent, &limits).into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["b", "a", "c"]);
    }

    #[test]
    fn unset_limits_are_worded_plainly() {
        let mut limits = Limits {
            daily_usd: 10.0,
            ticket_usd: 2.0,
            run_usd: None,
            min_run_usd: 0.05,
            projects: BTreeMap::new(),
            hires: BTreeMap::new(),
        };
        assert_eq!(
            limits_line(&limits, 17),
            "Per ticket $2.00 · Per run: no cap · Smallest run $0.05 · 17 runs today"
        );
        limits.run_usd = Some(0.75);
        assert!(
            limits_line(&limits, 1).ends_with("Per run: $0.75 · Smallest run $0.05 · 1 run today")
        );
    }
}
