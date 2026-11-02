//! The panels of the rooms people wait or work in: review and the lounge.

use topcoat::view::{View, component, view};

use super::team::hire_link;
use super::{capital, line, portrait_class, quiet, tint};
use crate::pages::office::floor::{self, Room, room};
use crate::pages::office::{Office, ago, safe};
use crate::{client, ui};

/// The ticket a judge is reading: its id, title and project tint.
type Reading = (String, String, usize);

/// Reviewers and critics, and what each is reading now.
#[component]
pub(super) async fn review_view(office: Office) -> topcoat::Result<impl View> {
    let judges: Vec<(client::Member, Option<Reading>)> = office
        .status
        .team
        .iter()
        .filter(|m| matches!(m.role.as_str(), "reviewer" | "critic"))
        .map(|m| {
            let reading = office
                .tickets
                .iter()
                .filter(|t| room(t) == Room::Review)
                .find(|t| {
                    office
                        .judges
                        .get(&t.id.to_string())
                        .is_some_and(|j| j.hire == m.id)
                })
                .map(|t| {
                    (
                        t.id.to_string(),
                        t.title.clone(),
                        tint(&office, &t.project.to_string()),
                    )
                });
            (m.clone(), reading)
        })
        .collect();
    Ok(view! {
        if judges.is_empty() {
            quiet(text: "No reviewer or critic is hired yet.")
        }
        for (member, reading) in judges {
            <div class="card">
                <div class="flex items-center gap-2">
                    <span class=(format!("portrait {}", portrait_class(&member.id, &member.role)))></span>
                    <div class="min-w-0">
                        <b class="block text-[13px] font-semibold">(member.id.clone())</b>
                        <span class="block text-[12px] text-fg-3">(capital(&member.role)) " · " (member.model.clone())</span>
                    </div>
                </div>
                if let Some((id, title, index)) = reading {
                    <span class="text-[12px] text-fg-3">"Reading now"</span>
                    line(id: id, title: title, tint: index)
                } else {
                    <span class="text-[12.5px] text-fg-3">"Nothing to read right now."</span>
                }
            </div>
        }
        hire_link()
    })
}

/// People with nothing to do, each with a task to give.
#[component]
pub(super) async fn lounge_view(office: Office) -> topcoat::Result<impl View> {
    let idle: Vec<(String, String, String)> = floor::resting(&office)
        .into_iter()
        .map(|m| {
            let rest = floor::idle_since(&office, &m.id).map_or_else(
                || "resting".to_string(),
                |at| format!("resting {}", ago(office.now.saturating_sub(at))),
            );
            (
                m.id.clone(),
                portrait_class(&m.id, &m.role),
                format!("{} · {rest}", capital(&m.role)),
            )
        })
        .collect();
    Ok(view! {
        if idle.is_empty() {
            quiet(text: "Everyone is at work.")
        }
        for (id, look, words) in idle {
            <div class="member">
                <span class=(format!("portrait {look}"))></span>
                <div class="who min-w-0"><b>(id.clone())</b><span>(words)</span></div>
                <button type="button" class=(ui::BUTTON_SM) popovertarget="office-new-ticket" data-on:click=(format!("$pick = '{}'", safe(&id)))>"Give a task"</button>
            </div>
        }
    })
}

#[cfg(test)]
mod tests {

    use crate::pages::office::panel::fixtures::{NOW, extra, html_of, office, ticket};
    use crate::summary::Judge;
    use serde_json::json;

    #[tokio::test]
    async fn review_names_the_judges_and_what_they_read() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("reviewing"),
            json!({"parent": "SHOP-1"}),
        );
        office.judges.insert(
            "SHOP-3".into(),
            Judge {
                hire: "reviewer".into(),
                model: "opus".into(),
                gate: None,
                at: NOW,
            },
        );
        let html = html_of(office, extra(), "elements/review").await;
        assert!(html.contains("reviewer"), "{html}");
        assert!(html.contains("Reading now"), "{html}");
        assert!(html.contains(">SHOP-3<"), "{html}");
    }

    #[tokio::test]
    async fn the_lounge_gives_tasks_to_idle_people() {
        let html = html_of(office(), extra(), "elements/lounge").await;
        assert!(html.contains("Give a task"), "{html}");
        assert!(
            html.contains("popovertarget=\"office-new-ticket\""),
            "{html}"
        );
        assert!(html.contains("$pick = 'reviewer'"), "{html}");
        // dev is at a desk, so it isn't resting.
        assert!(!html.contains("$pick = 'dev'"), "{html}");
    }
}
