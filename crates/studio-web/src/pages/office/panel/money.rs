//! The Money tab: what was spent, what is held, and the limits.

use std::collections::BTreeMap;
use studiod::api::DaySpend;
use topcoat::view::{View, component, view};

use super::{Budget, Extra, section, share, tint};
use crate::pages::office::Office;
use crate::pages::office::facts::dot_class;
use crate::pages::office::model::Role;
use crate::ui;

/// "Oct 19" for "2026-10-19".
fn short_date(date: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, rest) = date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    let month = MONTHS.get(month.parse::<usize>().ok()?.checked_sub(1)?)?;
    Some(format!("{month} {}", day.parse::<u32>().ok()?))
}

/// One day's bar.
#[derive(Clone, Debug, PartialEq)]
struct Bar {
    label: String,
    usd: f64,
    /// Its height in percent of the chart.
    height: f64,
}

/// The days as bars, and where the daily limit sits in the chart. The chart
/// is as tall as the biggest day or the limit.
fn bars(days: &[DaySpend], daily: f64) -> (Vec<Bar>, Option<f64>) {
    let top = days.iter().map(|d| d.usd).fold(daily.max(0.0), f64::max);
    let last = days.len().saturating_sub(1);
    let bars = days
        .iter()
        .enumerate()
        .map(|(i, d)| Bar {
            label: if i == last {
                "Today".into()
            } else {
                short_date(&d.date).unwrap_or_default()
            },
            usd: d.usd,
            height: share(d.usd, top),
        })
        .collect();
    (bars, (daily > 0.0).then(|| share(daily, top)))
}

/// The spend by project, biggest first, from what each ticket cost.
fn by_project(office: &Office, budget: &Budget) -> Vec<(String, usize, f64)> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for (id, usd) in &budget.spent_24h.by_ticket {
        if let Some(ticket) = office.ticket(id) {
            *sums.entry(ticket.project.to_string()).or_default() += usd;
        }
    }
    let mut rows: Vec<(String, usize, f64)> = sums
        .into_iter()
        .filter(|(_, usd)| *usd > 0.0)
        .map(|(name, usd)| (tint(office, &name), name, usd))
        .map(|(i, name, usd)| (name, i, usd))
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    rows
}

/// The spend by hire, biggest first.
fn by_hire(budget: &Budget) -> Vec<(String, f64)> {
    let mut rows: Vec<(String, f64)> = budget
        .spent_24h
        .by_hire
        .iter()
        .filter(|(_, usd)| **usd > 0.0)
        .map(|(hire, usd)| (hire.clone(), *usd))
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows
}

/// What was spent over the last 24 hours against the daily limit, with what
/// live runs hold and what is left when the budget says.
#[component]
pub(super) async fn spend_summary(
    spent: f64,
    daily: f64,
    held: Option<f64>,
) -> topcoat::Result<impl View> {
    let left = held.map(|held| (daily - spent - held).max(0.0));
    let tone = if daily > 0.0 && spent / daily >= 0.9 {
        "bad"
    } else if daily > 0.0 && spent / daily >= 0.7 {
        "attn"
    } else {
        ""
    };
    Ok(view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                <b class="text-[28px] font-bold leading-none tabular-nums">(ui::usd(spent))</b>
                <span class="text-[12.5px] text-fg-3">
                    "spent of " (ui::limit(daily))
                    if let Some(held) = held {
                        " · " (ui::usd(held)) " held by live runs"
                    }
                    if let Some(left) = left {
                        " · " (ui::usd(left)) " left"
                    }
                </span>
            </div>
            ui::meter(value: spent, max: daily, tone: tone)
        </div>
    })
}

/// A row of a split: a name, its share as a bar and the amount.
#[component]
async fn split_row(
    #[into] name: String,
    #[into] fill: String,
    percent: f64,
    #[into] amount: String,
    #[default] tint: Option<usize>,
    #[into]
    #[default]
    shirt: String,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class=(format!("grid grid-cols-[104px_minmax(0,1fr)_56px] items-center gap-2.5 text-[13px] {shirt}"))>
            <span class="flex min-w-0 items-center gap-1.5">
                if let Some(tint) = tint {
                    <i class=(dot_class(tint))></i>
                }
                <span class="truncate">(name)</span>
            </span>
            <span class="h-1.5 overflow-hidden rounded-full bg-bg-4">
                <i class="block h-full rounded-full" style=(format!("width: {percent:.1}%; background: {fill}"))></i>
            </span>
            <b class="text-right font-semibold tabular-nums">(amount)</b>
        </div>
    })
}

/// The limits, to change: the daily one and what one ticket may spend.
#[component]
pub(super) async fn limits_form(daily: f64, ticket: Option<f64>) -> topcoat::Result<impl View> {
    let ticket = ticket.map(|usd| usd.to_string()).unwrap_or_default();
    Ok(view! {
        <form class="flex flex-col gap-3" data-on:submit="@post('/office/limits', {contentType: 'form'})">
            <div class="grid grid-cols-2 gap-3">
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Daily limit, USD"</span>
                    <input name="daily_usd" type="number" step="0.01" min="0.01" required="" value=(daily.to_string()) class=(ui::INPUT)>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"One ticket, USD"</span>
                    <input name="ticket_usd" type="number" step="0.01" min="0.01" value=(ticket) placeholder="Leave as is" class=(ui::INPUT)>
                </label>
            </div>
            <div><button type="submit" class=(ui::BUTTON)>"Save limits"</button></div>
        </form>
    })
}

#[component]
pub(super) async fn money_pane(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let status = &office.status;
    let (spent, daily) = (status.spent_24h, status.daily_usd);
    let held = extra.budget.as_ref().map(Budget::held);
    let ticket_usd = extra.budget.as_ref().and_then(|b| b.limits.ticket_usd);
    let projects = extra
        .budget
        .as_ref()
        .map(|b| by_project(&office, b))
        .unwrap_or_default();
    let project_total: f64 = projects.iter().map(|(_, _, usd)| usd).sum();
    let hires = extra.budget.as_ref().map(by_hire).unwrap_or_default();
    let hire_total: f64 = hires.iter().map(|(_, usd)| usd).sum();
    let hires: Vec<(String, String, f64)> = hires
        .into_iter()
        .map(|(hire, usd)| {
            let role = office.member(&hire).map_or("", |m| m.role.as_str());
            (hire.clone(), Role::of(role).class().to_string(), usd)
        })
        .collect();
    let (days, limit_at) = bars(&extra.days, daily);
    let first = days.first().map(|b| b.label.clone()).unwrap_or_default();
    let limit_note = format!("daily limit {}", ui::limit(daily));
    Ok(view! {
        section(label: "Last 24 hours", note: limit_note,
            spend_summary(spent: spent, daily: daily, held: held)
        )
        if !projects.is_empty() {
            section(label: "By project",
                <div class="flex flex-col gap-2">
                    for (name, index, usd) in projects {
                        split_row(name: name, fill: format!("var(--pj-{index})"), percent: share(usd, project_total), amount: ui::usd(usd), tint: Some(index))
                    }
                </div>
            )
        }
        if !days.is_empty() {
            section(label: "14 days", note: "dashed: daily limit",
                <div class="relative flex h-24 items-end gap-1 border-b border-line">
                    if let Some(at) = limit_at {
                        <span class="pointer-events-none absolute inset-x-0 border-t border-dashed border-attn" style=(format!("bottom: {at:.1}%"))></span>
                    }
                    for bar in days {
                        <i class=(if bar.label == "Today" { "block min-h-px flex-1 rounded-t-sm bg-run" } else { "block min-h-px flex-1 rounded-t-sm bg-fg-3" })
                            style=(format!("height: {:.1}%", bar.height)) title=(format!("{} · {}", bar.label, ui::usd(bar.usd)))></i>
                    }
                </div>
                <div class="flex justify-between text-[11.5px] text-fg-3"><span>(first)</span><span>"Today"</span></div>
            )
        }
        if !hires.is_empty() {
            section(label: "By hire",
                <div class="flex flex-col gap-2">
                    for (hire, class, usd) in hires {
                        split_row(name: hire, fill: "var(--shirt)", percent: share(usd, hire_total), amount: ui::usd(usd), shirt: class)
                    }
                </div>
            )
        }
        section(label: "Limits",
            limits_form(daily: daily, ticket: ticket_usd)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::panel::fixtures::{extra, html_of, office};

    #[tokio::test]
    async fn money_adds_up_what_studiod_says() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("$6.40"), "{html}");
        assert!(html.contains("$0.72 held by live runs"), "{html}");
        assert!(html.contains("$2.88 left"), "{html}");
        // By project: SHOP-3's $3.90 is the shop's; API-1's $1.10 the api's.
        assert!(html.contains("$3.90"), "{html}");
        assert!(html.contains("$1.10"), "{html}");
        assert!(html.contains("Oct 19"), "{html}");
        assert!(html.contains("daily limit $10"), "{html}");
        assert!(html.contains("@post('/office/limits'"), "{html}");
        assert!(html.contains("name=\"daily_usd\""), "{html}");
        assert!(html.contains("name=\"ticket_usd\""), "{html}");
        assert!(html.contains("value=\"2.5\""), "{html}");
    }

    #[test]
    fn the_chart_is_as_tall_as_the_biggest_day_or_the_limit() {
        let day = |usd| DaySpend {
            date: "2026-10-19".into(),
            start_ms: 0,
            usd,
        };
        let (bars, limit) = bars(&[day(2.0), day(5.0)], 10.0);
        assert_eq!(bars[1].height, 50.0);
        assert_eq!(limit, Some(100.0));
        assert_eq!(bars[1].label, "Today");
        assert_eq!(bars[0].label, "Oct 19");
        let (bars, limit) = super::bars(&[day(20.0)], 10.0);
        assert_eq!((bars[0].height, limit), (100.0, Some(50.0)));
        assert_eq!(super::bars(&[], 10.0).0.len(), 0);
    }
}
