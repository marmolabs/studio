//! Where the money went: runs added up by kind, then by project, with how many
//! calls each took and which model ran them. Shared by the company page, a
//! task's page and Look closer.

use studiod::api::{SpendLine, by_kind, model_names};
use topcoat::view::{View, component, view};

use crate::ui;

/// One line of a split: what it is, what it came to, and a quiet note.
#[derive(Clone, Debug, PartialEq)]
pub struct CostRow {
    pub label: String,
    pub usd: f64,
    /// Calls, the model and who ran them, such as "19 calls · Opus · Nora".
    pub note: String,
}

/// What a hire is called in a cost row: the name in its file, else its id
/// with a capital, as a hire since removed has only that left.
pub fn name_in(team: &[crate::client::Member], hire: &str) -> String {
    let named = team
        .iter()
        .find(|m| m.id == hire)
        .map(|m| m.name.trim())
        .filter(|name| !name.is_empty());
    match named {
        Some(name) => name.to_string(),
        None => {
            let mut chars = hire.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

fn calls(n: u64) -> String {
    format!("{n} {}", if n == 1 { "call" } else { "calls" })
}

/// A list of names in a sentence: the first few, then a count.
fn some(names: Vec<String>) -> String {
    match names.len() {
        0..=2 => names.join(", "),
        n => format!("{} and {} more", names[..2].join(", "), n - 2),
    }
}

/// The lines by kind, the dearest first. `name_of` turns a hire's id into
/// the name people know it by.
pub fn by_kind_rows(lines: &[SpendLine], name_of: &dyn Fn(&str) -> String) -> Vec<CostRow> {
    by_kind(lines)
        .into_iter()
        .map(|total| {
            let mut hires: Vec<(String, f64)> = Vec::new();
            for line in lines.iter().filter(|l| l.kind == total.kind) {
                match hires.iter_mut().find(|(h, _)| *h == line.hire) {
                    Some((_, usd)) => *usd += line.usd,
                    None => hires.push((line.hire.clone(), line.usd)),
                }
            }
            hires.sort_by(|a, b| b.1.total_cmp(&a.1));
            let models = total.models;
            let who: Vec<String> = hires.iter().map(|(h, _)| name_of(h)).collect();
            let note = format!("{} · {} · {}", calls(total.calls), some(models), some(who));
            CostRow {
                label: total.kind.label().to_string(),
                usd: total.usd,
                note,
            }
        })
        .collect()
}

/// The lines by project, the dearest first.
pub fn by_project_rows(lines: &[SpendLine]) -> Vec<CostRow> {
    let mut names: Vec<&str> = Vec::new();
    for line in lines {
        if !names.contains(&line.project.as_str()) {
            names.push(&line.project);
        }
    }
    let mut rows: Vec<CostRow> = names
        .into_iter()
        .map(|project| {
            let own: Vec<SpendLine> = lines
                .iter()
                .filter(|l| l.project == project)
                .cloned()
                .collect();
            // The models, the dearest first, each once.
            let mut dearest: Vec<(String, f64)> = Vec::new();
            for line in &own {
                match dearest.iter_mut().find(|(m, _)| *m == line.model) {
                    Some((_, usd)) => *usd += line.usd,
                    None => dearest.push((line.model.clone(), line.usd)),
                }
            }
            dearest.sort_by(|a, b| b.1.total_cmp(&a.1));
            let models = model_names(dearest.iter().map(|(m, _)| m.as_str()));
            CostRow {
                label: project.to_string(),
                usd: own.iter().map(|l| l.usd).sum(),
                note: format!(
                    "{} · {}",
                    calls(own.iter().map(|l| l.calls).sum()),
                    some(models)
                ),
            }
        })
        .collect();
    rows.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    rows
}

/// A one-line warning, quiet: a small mark and plain words.
#[component]
pub async fn heads_up(words: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <p class="m-0 flex items-start gap-2 text-[13px] text-attn-fg" role="note">
            <span class="mt-0.5 shrink-0">ui::icon(name: "circle-alert", small: true)</span>
            <span class="min-w-0 [overflow-wrap:anywhere]">(words)</span>
        </p>
    })
}

/// The rows, each with its share of the dearest as a thin bar. `framed` puts
/// the list in a box like the other lists; otherwise it sits bare.
#[component]
pub async fn cost_list(rows: Vec<CostRow>, #[default] framed: bool) -> topcoat::Result<impl View> {
    let most = rows.iter().map(|r| r.usd).fold(0.0, f64::max);
    let class = if framed {
        format!("m-0 list-none p-0 {}", ui::LIST)
    } else {
        "m-0 flex list-none flex-col gap-3 p-0".to_string()
    };
    let row = if framed {
        "flex flex-col gap-1.5 border-t border-line px-4 py-2.5 first:border-t-0"
    } else {
        "flex flex-col gap-1"
    };
    Ok(view! {
        <ul class=(class)>
            for row_of in rows {
                <li class=(row)>
                    <div class="flex items-baseline justify-between gap-3">
                        <b class="min-w-0 truncate text-[13px] font-medium">(row_of.label)</b>
                        <span class="shrink-0 text-[13px] tabular-nums">(ui::usd(row_of.usd))</span>
                    </div>
                    <span class="text-[12px] text-fg-3 [overflow-wrap:anywhere]">(row_of.note)</span>
                    ui::meter(value: row_of.usd, max: most, tone: "faint")
                </li>
            }
        </ul>
    })
}

/// What a task and its goal cost, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Costs {
    /// A title and the rows under it, the narrowest first.
    pub parts: Vec<(String, Vec<CostRow>)>,
    /// One plain line when approvals or routing cost more than the coding.
    pub warning: Option<String>,
}

/// The ids of a goal: its plan ticket and every ticket filed under it. A
/// ticket with no goal has none.
pub fn goal_ids(
    tickets: &[studio_core::ticket::Ticket],
    ticket: &studio_core::ticket::Ticket,
) -> Vec<String> {
    let plan = if ticket.kind == studio_core::ticket::TicketKind::Plan {
        ticket.id.clone()
    } else {
        match &ticket.parent {
            Some(plan) => plan.clone(),
            None => return Vec::new(),
        }
    };
    std::iter::once(plan.to_string())
        .chain(
            tickets
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&plan))
                .map(|t| t.id.to_string()),
        )
        .collect()
}

/// What these tickets cost, split by kind; empty when studiod can't say.
pub async fn read(socket: &std::path::Path, tickets: Vec<String>) -> Vec<SpendLine> {
    if tickets.is_empty() {
        return Vec::new();
    }
    crate::client::call(
        socket,
        studiod::api::Request::SpendBreakdown {
            tickets,
            hours: None,
        },
    )
    .await
    .unwrap_or_default()
}

/// The parts from `(title, scope, lines)`, with the first warning that applies
/// (the widest part is looked at first, so pass the goal before the task).
pub fn costs(parts: Vec<(&str, &str, Vec<SpendLine>)>, name_of: &dyn Fn(&str) -> String) -> Costs {
    let warning = parts
        .iter()
        .find_map(|(_, scope, lines)| studiod::api::warning(lines, scope));
    Costs {
        parts: parts
            .into_iter()
            .filter(|(_, _, lines)| !lines.is_empty())
            .map(|(title, _, lines)| (title.to_string(), by_kind_rows(&lines, name_of)))
            .collect(),
        warning,
    }
}

/// The split of each part under its title, with the warning above.
#[component]
pub async fn costs_view(costs: Costs, #[default] framed: bool) -> topcoat::Result<impl View> {
    let Costs { parts, warning } = costs;
    Ok(view! {
        <div class="flex flex-col gap-3">
            if let Some(words) = warning {
                heads_up(words: words)
            }
            for (title, rows) in parts {
                <div class="flex min-w-0 flex-col gap-2">
                    <h4 class="m-0 text-[12px] font-medium text-fg-3">(title)</h4>
                    cost_list(rows: rows, framed: framed)
                </div>
            }
        </div>
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use studiod::api::SpendKind;

    fn line(
        kind: SpendKind,
        project: &str,
        hire: &str,
        model: &str,
        calls: u64,
        usd: f64,
    ) -> SpendLine {
        SpendLine {
            kind,
            project: project.into(),
            hire: hire.into(),
            model: model.into(),
            calls,
            usd,
        }
    }

    #[test]
    fn a_hire_is_called_by_its_name_or_its_id_with_a_capital() {
        let team = [crate::client::Member {
            id: "shop-lead".into(),
            name: "Nora".into(),
            role: "lead".into(),
            model: "opus".into(),
        }];
        assert_eq!(name_in(&team, "shop-lead"), "Nora");
        assert_eq!(name_in(&team, "gone"), "Gone");
        assert_eq!(name_in(&team, ""), "");
    }

    #[test]
    fn models_are_named_as_people_say_them_once() {
        let lines = [
            line(SpendKind::Coding, "shop", "dev", "claude-opus-5-5", 1, 2.0),
            line(SpendKind::Coding, "shop", "dev", "opus", 1, 1.0),
            line(SpendKind::Coding, "shop", "dev", "claude-haiku-5-5", 1, 0.5),
        ];
        assert_eq!(
            by_kind_rows(&lines, &|h| h.into())[0].note,
            "3 calls · Opus, Haiku · dev"
        );
        assert_eq!(by_project_rows(&lines)[0].note, "3 calls · Opus, Haiku");
    }

    #[test]
    fn rows_say_calls_model_and_who_biggest_first() {
        let lines = [
            line(SpendKind::Coding, "shop", "dev", "sonnet", 1, 1.0),
            line(SpendKind::Approval, "shop", "nora", "opus", 19, 1.61),
            line(SpendKind::Approval, "docs", "nora", "opus", 1, 0.1),
        ];
        let name = |hire: &str| hire.to_uppercase();
        let rows = by_kind_rows(&lines, &name);
        assert_eq!(rows[0].label, "Approvals");
        assert_eq!(rows[0].note, "20 calls · Opus · NORA");
        assert_eq!(rows[1].note, "1 call · Sonnet · DEV");
        let projects = by_project_rows(&lines);
        assert_eq!(projects[0].label, "shop");
        assert_eq!(projects[0].note, "20 calls · Opus, Sonnet");
        assert_eq!(projects[1].label, "docs");
    }

    #[test]
    fn a_goal_is_its_plan_and_what_was_filed_under_it() {
        use studio_core::ticket::Ticket;
        let make = |id: &str, kind: &str, parent: Option<&str>| -> Ticket {
            serde_json::from_value(serde_json::json!({
                "id": id, "project": "shop", "title": id, "goal": "g", "hire": "dev",
                "state": "queued", "kind": kind, "parent": parent, "attempts": 0,
                "base_branch": "main", "created_at": 0,
            }))
            .unwrap()
        };
        let all = [
            make("S-1", "plan", None),
            make("S-2", "work", Some("S-1")),
            make("S-3", "work", Some("S-1")),
            make("S-4", "work", None),
        ];
        assert_eq!(goal_ids(&all, &all[1]), ["S-1", "S-2", "S-3"]);
        assert_eq!(goal_ids(&all, &all[0]), ["S-1", "S-2", "S-3"]);
        assert!(goal_ids(&all, &all[3]).is_empty());
    }
}
