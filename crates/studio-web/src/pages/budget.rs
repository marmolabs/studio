use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use studio_core::ticket::Ticket;
use studiod::api::{DaySpend, FileKind, FileView, Request};
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
    #[serde(default)]
    reserved: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Limits {
    daily_usd: f64,
    ticket_usd: f64,
    run_usd: Option<f64>,
    #[serde(default)]
    projects: BTreeMap<String, f64>,
    #[serde(default)]
    hires: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Spent {
    total: f64,
    #[serde(default)]
    by_hire: BTreeMap<String, f64>,
    #[serde(default)]
    by_model: BTreeMap<String, f64>,
    #[serde(default)]
    by_ticket: BTreeMap<String, f64>,
    #[serde(default)]
    runs_by_hire: BTreeMap<String, u64>,
}

/// The limits besides the daily one, as one line: "$2 a ticket · dev $8".
fn limits_line(limits: &Limits) -> String {
    let mut parts = vec![format!("{} a ticket", ui::limit(limits.ticket_usd))];
    if let Some(run) = limits.run_usd {
        parts.push(format!("{} a run", ui::limit(run)));
    }
    for (name, usd) in limits.projects.iter().chain(&limits.hires) {
        parts.push(format!("{name} {}", ui::limit(*usd)));
    }
    parts.join(" · ")
}

/// `ui::meter`'s tone for a share of a limit: amber approaching it, red near it.
fn meter_tone(share: f64) -> &'static str {
    if share >= 0.9 {
        "bad"
    } else if share >= 0.7 {
        "attn"
    } else {
        ""
    }
}

/// One hire's runs and spend in the last 24 hours.
struct HireSpend {
    hire: String,
    runs: u64,
    usd: f64,
}

/// Every hire that ran or spent, biggest spender first.
fn by_hire(spent: &Spent) -> Vec<HireSpend> {
    let names: BTreeSet<&String> = spent
        .by_hire
        .keys()
        .chain(spent.runs_by_hire.keys())
        .collect();
    let mut rows: Vec<HireSpend> = names
        .into_iter()
        .map(|name| HireSpend {
            hire: name.clone(),
            runs: spent.runs_by_hire.get(name).copied().unwrap_or(0),
            usd: spent.by_hire.get(name).copied().unwrap_or(0.0),
        })
        .collect();
    rows.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    rows
}

/// The keys of `map`, biggest value first, at most `limit` of them.
fn biggest(map: &BTreeMap<String, f64>, limit: usize) -> Vec<(String, f64)> {
    let mut rows: Vec<(String, f64)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows.truncate(limit);
    rows
}

/// A round step for about four gridlines up to `max`, and the top of the
/// axis, a whole number of steps.
fn scale(max: f64) -> (f64, f64) {
    let max = if max > 0.0 { max } else { 1.0 };
    let rough = max / 4.0;
    let power = 10f64.powf(rough.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * power)
        .find(|step| *step >= rough - 1e-9)
        .unwrap_or(10.0 * power);
    (step, step * (max / step - 1e-9).ceil())
}

/// One label under the chart.
#[derive(Clone, Debug, PartialEq)]
struct Label {
    /// The month before the day, and whether a wide and a narrow screen
    /// show it.
    month: Option<(&'static str, bool, bool)>,
    text: String,
    /// Whether a narrow screen shows it: every `NARROW_EVERY`th, counting
    /// back from today.
    narrow: bool,
}

/// A narrow screen labels one bar in this many, so the labels don't touch.
const NARROW_EVERY: usize = 3;

/// The labels under the bars: "Today", else the day of the month, with the
/// month on the first label a screen shows and on its first in a new month.
/// An older studiod gives no dates, so its bars say how many days back.
fn labels(days: &[DaySpend]) -> Vec<Label> {
    let n = days.len();
    let dates: Vec<Option<(&str, u32)>> = days.iter().map(|d| month_day(&d.date)).collect();
    (0..n)
        .map(|i| {
            let back = n - 1 - i;
            let narrow = back.is_multiple_of(NARROW_EVERY);
            let (month, text) = match dates[i] {
                _ if back == 0 => (None, "Today".to_string()),
                Some((month, day)) => {
                    let wide = i == 0 || day == 1;
                    // A narrow screen's label before this one, if any.
                    let new =
                        i < NARROW_EVERY || dates[i - NARROW_EVERY].map(|(m, _)| m) != Some(month);
                    let shown = (wide || narrow && new).then_some((month, wide, narrow && new));
                    (shown, day.to_string())
                }
                None => (None, format!("{back}d")),
            };
            Label {
                month,
                text,
                narrow,
            }
        })
        .collect()
}

/// The month's short name and the day from a `YYYY-MM-DD` date.
fn month_day(date: &str) -> Option<(&'static str, u32)> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = date.split('-').skip(1);
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    Some((MONTHS.get(month.checked_sub(1)?)?, day))
}

#[page("/budget")]
async fn budget(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let b: Budget = client::call(&config.socket, Request::Budget).await?;
    // An older studiod can't say; the chart and the week are left out.
    let days: Vec<DaySpend> = client::call(&config.socket, Request::SpendByDay { days: 14 })
        .await
        .unwrap_or_default();
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets)
        .await
        .unwrap_or_default();
    let tickets: HashMap<String, Ticket> =
        tickets.into_iter().map(|t| (t.id.to_string(), t)).collect();
    let files: Vec<FileView> = client::call(
        &config.socket,
        Request::Files {
            kind: FileKind::Hires,
        },
    )
    .await
    .unwrap_or_default();
    let staff = super::hires::studio_hires(&files);

    let daily = b.limits.daily_usd;
    let spent = b.spent_24h.total;
    let held: f64 = b.reserved.values().sum();
    let held_line = match b.reserved.len() {
        0 => "No live runs".to_string(),
        1 => "1 run, released when it finishes".to_string(),
        n => format!("{n} runs, released as they finish"),
    };
    let week = (days.len() >= 7).then(|| days.iter().rev().take(7).map(|d| d.usd).sum::<f64>());
    let limits = limits_line(&b.limits);
    let hires = by_hire(&b.spent_24h);
    // Each with its title and state; both empty for a ticket no longer known.
    let expensive: Vec<(String, String, &'static str, f64)> = biggest(&b.spent_24h.by_ticket, 5)
        .into_iter()
        .map(|(id, usd)| match tickets.get(&id) {
            Some(t) => (id, t.title.clone(), ui::shown(&t.state), usd),
            None => (id, String::new(), "", usd),
        })
        .collect();
    let models = biggest(&b.spent_24h.by_model, usize::MAX);

    Ok(view! {
        ui::kpis(
            ui::kpi(label: "Spent, last 24h", value: ui::usd(spent), unit: format!("of {}", ui::limit(daily)),
                <div class="mt-1.5">ui::meter(value: spent, max: daily, tone: meter_tone(if daily > 0.0 { spent / daily } else { 1.0 }))</div>)
            ui::kpi(label: "Held for live runs", value: ui::usd(held),
                <span class=(ui::KPI_SUB)>(held_line)</span>)
            if let Some(week) = week {
                ui::kpi(label: "Last 7 days", value: ui::usd(week),
                    <span class=(ui::KPI_SUB)>(ui::usd(week / 7.0)) " a day on average"</span>)
            }
            ui::kpi(label: "Daily limit", value: ui::limit(daily),
                <span class=(ui::KPI_SUB) title=(limits.clone())>(limits)</span>)
        )
        if !days.is_empty() {
            chart(days: days, limit: daily)
        }
        if hires.is_empty() && models.is_empty() {
            <p class=(ui::MUTED)>"Nothing spent in the last 24 hours."</p>
        } else {
            <div class="grid grid-cols-2 gap-6 max-[1180px]:grid-cols-1">
                <section>
                    head(title: "By hire")
                    <div class=(ui::TABLE_BOX)>
                        <table class=(ui::TABLE)>
                            <thead>
                                <tr>
                                    <th class=(ui::TH)>"Hire"</th>
                                    <th class=(format!("text-right {}", ui::TH))>"Runs"</th>
                                    <th class=(format!("text-right {}", ui::TH))>"Spent"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for row in hires {
                                    <tr class=(ui::TR)>
                                        <td class=(ui::TD)>ui::who(staff: staff.contains(&row.hire), hire: row.hire)</td>
                                        <td class=(format!("text-right tabular-nums {}", ui::TD))>(row.runs)</td>
                                        <td class=(ui::TD)>spend_bar(usd: row.usd, total: spent)</td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    </div>
                </section>
                <div class="flex min-w-0 flex-col gap-6">
                    if !expensive.is_empty() {
                        <section>
                            head(title: "Most expensive tickets")
                            <div class=(ui::TABLE_BOX)>
                                <table class=(ui::TABLE)>
                                    <thead>
                                        <tr>
                                            <th class=(ui::TH)>"Ticket"</th>
                                            <th class=(ui::TH)>"Outcome"</th>
                                            <th class=(format!("text-right {}", ui::TH))>"Spent"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        for (id, title, state, usd) in expensive {
                                            <tr class=(ui::TR)>
                                                <td class=(format!("w-full max-w-0 {}", ui::TD))>
                                                    <a href=(format!("/tickets/{id}")) class="flex min-w-0 items-center gap-2 hover:underline decoration-line-3 underline-offset-3">
                                                        <span class="shrink-0 font-mono text-[12.5px] text-fg-3">(id)</span>
                                                        <span class="truncate">(title)</span>
                                                    </a>
                                                </td>
                                                <td class=(ui::TD)>
                                                    if !state.is_empty() {
                                                        outcome(state: state)
                                                    }
                                                </td>
                                                <td class=(format!("text-right tabular-nums {}", ui::TD))>(ui::usd(usd))</td>
                                            </tr>
                                        }
                                    </tbody>
                                </table>
                            </div>
                        </section>
                    }
                    if !models.is_empty() {
                        <section>
                            head(title: "By model")
                            <div class=(ui::TABLE_BOX)>
                                <table class=(ui::TABLE)>
                                    <tbody>
                                        for (model, usd) in models {
                                            <tr class=(ui::TR)>
                                                <td class=(ui::TD)>ui::model_tag(model: model)</td>
                                                <td class=(ui::TD)>spend_bar(usd: usd, total: spent)</td>
                                            </tr>
                                        }
                                    </tbody>
                                </table>
                            </div>
                        </section>
                    }
                </div>
            </div>
        }
    })
}

/// A section's title with a quiet "last 24h" after it.
#[component]
async fn head(title: &'static str) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="mb-2.5 flex items-baseline gap-2">
            <h2 class=(ui::SECTION_TITLE)>(title)</h2>
            <span class=(ui::MUTED)>"last 24h"</span>
        </div>
    })
}

/// An amount with a bar for its share of today's spend.
#[component]
async fn spend_bar(usd: f64, total: f64) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex items-center justify-end gap-2.5">
            <span class="w-[88px] max-[760px]:w-12">ui::meter(value: usd, max: total, tone: "faint")</span>
            <span class="tabular-nums">(ui::usd(usd))</span>
        </div>
    })
}

/// A ticket's state as a small pill, without its glyph.
#[component]
async fn outcome(state: &'static str) -> topcoat::Result<impl View> {
    let look = ui::look(state);
    Ok(view! {
        <span class="[&>span]:h-5 [&>span]:px-2 [&>span]:text-[11.5px]">
            ui::pill(tone: look.tone, label: look.label)
        </span>
    })
}

/// Spend per day as bars on a dollar axis, with the daily limit.
#[component]
async fn chart(days: Vec<DaySpend>, limit: f64) -> topcoat::Result<impl View> {
    let most = days.iter().map(|d| d.usd).fold(0.0, f64::max);
    let least = days.iter().map(|d| d.usd).fold(f64::INFINITY, f64::min);
    let (step, top) = scale(most.max(limit));
    let at = move |usd: f64| 100.0 - usd / top * 100.0;
    let count = (top / step).round() as usize;
    let ticks: Vec<(f64, bool)> = (0..=count)
        .rev()
        .map(|i| {
            let usd = i as f64 * step;
            (usd, limit > 0.0 && (usd - limit).abs() < 1e-9)
        })
        .collect();
    // A limit between gridlines gets a line of its own.
    let own_line = limit > 0.0 && !ticks.iter().any(|(_, is_limit)| *is_limit);
    let n = days.len();
    let columns = format!("grid-template-columns:repeat({n},minmax(0,1fr))");
    let label = format!(
        "Daily spend for the last {n} days, between {} and {}, against the {} daily limit",
        ui::usd(least),
        ui::usd(most),
        ui::limit(limit)
    );
    let bars: Vec<(String, String, bool)> = days
        .iter()
        .enumerate()
        .map(|(i, day)| {
            let back = n - 1 - i;
            let when = match (back, month_day(&day.date)) {
                (0, _) => "today so far".to_string(),
                (_, Some((month, d))) => format!("on {month} {d}"),
                (b, None) => format!("{b} days back"),
            };
            (
                format!("height:{:.1}%", (day.usd / top * 100.0).clamp(0.0, 100.0)),
                format!("{} {when}", ui::usd(day.usd)),
                back == 0,
            )
        })
        .collect();
    let labels = labels(&days);
    Ok(view! {
        <section class=(format!("px-5 pb-3.5 pt-[18px] max-[760px]:px-4 {}", ui::CARD))>
            <div class="mb-3.5 flex flex-wrap items-baseline justify-between gap-3">
                <h2 class=(ui::SECTION_TITLE)>"Daily spend"</h2>
                <span class="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12.5px] text-fg-3">
                    // The dashed line's legend, out of the bars' way.
                    if limit > 0.0 {
                        <span class="inline-flex items-center gap-1.5 text-attn-fg">
                            <i class="w-4 border-t border-dashed border-attn-line" aria-hidden="true"></i>
                            "Daily limit " (ui::limit(limit))
                        </span>
                    }
                    <span>"Last " (n) " days"</span>
                </span>
            </div>
            <div class="relative ml-10 h-[180px]" role="img" aria-label=(label)>
                for (usd, is_limit) in ticks {
                    <div
                        class=(if is_limit { "absolute inset-x-0 border-t border-dashed border-attn-line" } else { "absolute inset-x-0 border-t border-line" })
                        style=(format!("top:{:.2}%", at(usd)))
                    >
                        <span class="absolute -left-10 -top-2 w-8 text-right text-[11.5px] tabular-nums text-fg-3">(ui::limit(usd))</span>
                    </div>
                }
                if own_line {
                    <div class="absolute inset-x-0 border-t border-dashed border-attn-line" style=(format!("top:{:.2}%", at(limit)))></div>
                }
                <div class="absolute inset-0 grid items-end gap-2 max-[760px]:gap-1" style=(columns.clone())>
                    for (height, title, today) in bars {
                        <i
                            class=(if today { "block min-h-0.5 rounded-t-[3px] bg-fg" } else { "block min-h-0.5 rounded-t-[3px] bg-line-3" })
                            style=(height)
                            title=(title)
                        ></i>
                    }
                </div>
            </div>
            <div class="ml-10 mt-2 grid gap-2 text-center text-[11.5px] tabular-nums text-fg-3 max-[760px]:gap-1" style=(columns) aria-hidden="true">
                for (i, label) in labels.into_iter().enumerate() {
                    <span class=(if i + 1 == n { "text-fg max-[760px]:justify-self-end" } else if label.narrow { "whitespace-nowrap" } else { "whitespace-nowrap max-[760px]:invisible" })>
                        if let Some((month, wide, narrow)) = label.month {
                            <span class=(match (wide, narrow) { (true, true) => "", (true, false) => "max-[760px]:hidden", _ => "hidden max-[760px]:inline" })>(month) " "</span>
                        }
                        (label.text)
                    </span>
                }
            </div>
        </section>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_steps_are_round() {
        assert_eq!(scale(100.0), (25.0, 100.0));
        assert_eq!(scale(10.0), (2.5, 10.0));
        assert_eq!(scale(64.0), (20.0, 80.0));
        // Nothing spent and no limit still draws an axis.
        assert_eq!(scale(0.0), (0.25, 1.0));
    }

    #[test]
    fn hires_are_listed_by_spend_with_their_runs() {
        let spent = Spent {
            total: 3.0,
            by_hire: BTreeMap::from([("dev".to_string(), 1.0), ("artist".to_string(), 2.0)]),
            by_model: BTreeMap::new(),
            by_ticket: BTreeMap::new(),
            runs_by_hire: BTreeMap::from([
                ("dev".to_string(), 4),
                ("artist".to_string(), 1),
                ("approver".to_string(), 9),
            ]),
        };
        let rows: Vec<_> = by_hire(&spent)
            .into_iter()
            .map(|r| (r.hire, r.runs))
            .collect();
        assert_eq!(
            rows,
            [
                ("artist".to_string(), 1),
                ("dev".to_string(), 4),
                ("approver".to_string(), 9)
            ]
        );
    }

    #[test]
    fn limits_read_as_one_line() {
        let mut limits = Limits {
            daily_usd: 10.0,
            ticket_usd: 2.0,
            run_usd: None,
            projects: BTreeMap::new(),
            hires: BTreeMap::from([("reviewer".to_string(), 1.5)]),
        };
        assert_eq!(limits_line(&limits), "$2 a ticket · reviewer $1.50");
        limits.run_usd = Some(0.75);
        assert_eq!(
            limits_line(&limits),
            "$2 a ticket · $0.75 a run · reviewer $1.50"
        );
    }

    #[test]
    fn labels_name_the_month_on_each_screens_first_label_and_month() {
        let day = |date: &str| DaySpend {
            date: date.into(),
            start_ms: 0,
            usd: 0.0,
        };
        let label = |month, text: &str, narrow| Label {
            month,
            text: text.into(),
            narrow,
        };
        let days = [
            "2026-10-29",
            "2026-10-30",
            "2026-10-31",
            "2026-11-01",
            "2026-11-02",
        ];
        assert_eq!(
            labels(&days.map(day)),
            [
                label(Some(("Oct", true, false)), "29", false),
                label(Some(("Oct", false, true)), "30", true),
                label(None, "31", false),
                label(Some(("Nov", true, false)), "1", false),
                label(None, "Today", true),
            ]
        );
        // A narrow screen skips the first of November, so its next label
        // names the month.
        let days = [
            "2026-10-30",
            "2026-10-31",
            "2026-11-01",
            "2026-11-02",
            "2026-11-03",
            "2026-11-04",
            "2026-11-05",
        ];
        let months: Vec<_> = labels(&days.map(day))
            .into_iter()
            .map(|l| l.month)
            .collect();
        assert_eq!(
            months,
            [
                Some(("Oct", true, true)),
                None,
                Some(("Nov", true, false)),
                Some(("Nov", false, true)),
                None,
                None,
                None,
            ]
        );
        assert_eq!(labels(&[day(""), day("")])[0].text, "1d");
        assert_eq!(month_day("2026-13-01"), None);
    }
}
