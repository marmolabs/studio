//! Studio settings: the money limits, the desks, the hires, the rules and
//! opening or closing the office. The forms are in `office/controls.rs`.

use topcoat::view::{Child, View, component, view};

use studiod::api::Request;

use super::shell::{self, Here};
use super::{Team, faces};
use crate::client::Member;
use crate::pages::office::controls::{Budget, desks_view, hire_view, limits_form, open_view};
use crate::pages::office::model::Role;
use crate::pages::office::{Office, facts};
use crate::{AppConfig, client, ui};

/// What one task may cost, when a limit is set: it is not in the office, so
/// the page asks for it. Read each time, as the owner may have just changed it.
pub async fn ticket_usd(config: &AppConfig) -> Option<f64> {
    client::call::<Budget>(&config.socket, Request::Budget)
        .await
        .ok()
        .and_then(|budget| budget.limits.ticket_usd)
}

/// The whole page: the frame around the settings. `ticket_usd` is what one
/// task may cost, when a limit is set.
#[component]
pub async fn settings(
    team: Team,
    office: Office,
    ticket_usd: Option<f64>,
) -> topcoat::Result<impl View> {
    let projects = office.project_names();
    // Each hire with the colour its face takes: a lead's is its project's.
    let hires: Vec<(Member, Role, usize)> = office
        .status
        .team
        .iter()
        .map(|member| {
            let led = office
                .projects
                .iter()
                .find(|project| project.lead.as_deref() == Some(member.id.as_str()));
            let tint = led.map_or(0, |project| facts::tint(&projects, &project.name));
            let member = Member {
                name: office.name_of(&member.id),
                ..member.clone()
            };
            (member.clone(), Role::of(&member.role), tint)
        })
        .collect();
    let (spent, daily) = (office.status.spent_24h, office.status.daily_usd);
    let (desks, closed, rules) = (
        office.status.max_runs,
        office.status.closed,
        office.status.rules,
    );
    Ok(view! {
        shell::page(team: team, here: Here::Studio,
            <div class="mx-auto flex w-full max-w-[640px] flex-col gap-5">
                <h1 class=(ui::PAGE_TITLE)>"Studio settings"</h1>
                part(title: "Money", note: "What the studio may spend. Agents stop when a limit is reached.",
                    <div class="flex flex-col gap-1.5" title="Spend over the last 24 hours, against the daily limit">
                        <span class="flex items-baseline justify-between text-[13px]">
                            <span class="text-fg-3">"Spent in the last 24 hours"</span>
                            <span class="tabular-nums">(ui::usd(spent)) <span class="text-fg-3">" of " (ui::limit(daily))</span></span>
                        </span>
                        ui::meter(value: spent, max: daily)
                    </div>
                    limits_form(daily: daily, ticket: ticket_usd)
                )
                part(title: "Desks", note: "How many agents work at once.",
                    desks_view(desks: desks)
                )
                part(title: "Hires", note: "Everyone who works for the studio.",
                    if hires.is_empty() {
                        <p class="m-0 text-[13px] text-fg-3">"Nobody is hired yet."</p>
                    } else {
                        <ul class="m-0 flex list-none flex-col p-0">
                            for (member, role, tint) in hires {
                                <li class="flex items-center gap-3 border-t border-line py-2 first:border-t-0">
                                    faces::face(seed: member.id.clone(), role: role, tint: tint, size: 30)
                                    <span class="min-w-0 flex-1">
                                        <b class="block truncate font-medium">(member.name.clone())</b>
                                        <span class="block truncate text-[12.5px] text-fg-3">(member.role.clone()) " · " (member.model.clone())</span>
                                    </span>
                                </li>
                            }
                        </ul>
                    }
                    // Opened to hire someone, it stays open, with what is typed in it,
                    // as the page is patched.
                    <details id="hire" data-preserve-attr="open" class="group rounded-lg border border-line">
                        <summary class="cursor-pointer list-none px-3 py-2 text-[13.5px] font-medium">"Hire someone"</summary>
                        <div class="flex flex-col gap-3 border-t border-line p-3">hire_view()</div>
                    </details>
                )
                part(title: "Rules", note: "What agents may and may not do.",
                    <div class=(ui::ASK_ACTIONS)>
                        <a href="/rules" class=(ui::BUTTON)>"Open the rules"</a>
                        <span class="text-[12.5px] text-fg-3">(rules) if rules == 1 { " rule" } else { " rules" } " in force"</span>
                    </div>
                )
                part(title: "The office", note: "Closing it stops new work from starting.",
                    open_view(closed: closed)
                )
            </div>
        )
    })
}

/// One part of the page: a box with a title and a line on what it is for.
#[component]
async fn part(
    title: &'static str,
    note: &'static str,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <section class="setup-box" aria-label=(title)>
            <div class="flex flex-col gap-0.5">
                <h2 class=(ui::SECTION_TITLE)>(title)</h2>
                <p class="m-0 text-[12.5px] text-fg-3">(note)</p>
            </div>
            (child)
        </section>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Tag, named, text_of};

    fn button<'a>(html: &'a str, label: &str) -> Option<Tag<'a>> {
        named(html, "button")
            .into_iter()
            .find(|button| button.text().contains(label))
    }
    use serde_json::json;
    use topcoat::context::Cx;

    fn office() -> Office {
        Office {
            status: serde_json::from_value(json!({
            "projects": ["shop"], "max_runs": 3, "daily_usd": 10.0, "spent_24h": 6.5, "rules": 2,
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "role": "engineer", "model": "sonnet"},
            ],
        }))
            .unwrap(),
            projects: vec![studiod::api::ProjectView {
            name: "shop".into(),
            lead: Some("nora".into()),
            ..studiod::api::ProjectView::default()
        }],
            ..Office::default()
        }
    }

    async fn page(office: Office, ticket_usd: Option<f64>) -> String {
        let cx = Cx::default();
        let team = Team::of(&office);
        ui::html(
            &cx,
            view! { cx => settings(team: team, office: office, ticket_usd: ticket_usd) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_settings_hold_money_desks_hires_rules_and_the_office() {
        let html = page(office(), Some(2.5)).await;
        let text = text_of(&html);
        for title in ["Money", "Desks", "Hires", "Rules", "The office"] {
            assert!(text.contains(title), "{title}");
        }
        assert!(text.contains("$6.50 of $10"), "{text}");
        assert!(text.contains("Up to 3 agents work at once"), "{text}");
        // The hires with their roles and models.
        assert!(text.contains("Nora"), "{text}");
        assert!(text.contains("lead · opus"), "{text}");
        assert!(text.contains("engineer · sonnet"), "{text}");
        assert!(text.contains("2 rules in force"), "{text}");
        assert!(
            named(&html, "a")
                .iter()
                .any(|a| a.attr_is("href", "/rules"))
        );
        assert!(text.contains("Daily limit"), "{text}");
        assert!(html.contains(r#"value="2.5""#), "{html}");
        assert!(text.contains("One task"), "{text}");
    }

    #[tokio::test]
    async fn the_office_can_be_closed_and_opened() {
        let html = page(office(), None).await;
        assert!(button(&html, "Close the office").is_some());
        let mut closed = office();
        closed.status.closed = true;
        let html = page(closed, None).await;
        assert!(button(&html, "Reopen the office").is_some());
    }
}
