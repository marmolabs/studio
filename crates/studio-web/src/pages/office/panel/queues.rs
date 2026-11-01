//! The panels of the places where tickets wait or pass: review, returns,
//! crates, counter, queue and lounge.

use topcoat::view::{View, component, view};

use super::team::hire_link;
use super::{capital, line, portrait_class, quiet, section, tint};
use crate::pages::office::facts::{self};
use crate::pages::office::floor::{self, Room, room};
use crate::pages::office::inspector::{self};
use crate::pages::office::{Office, ago, safe};
use crate::pages::timeline::relative;
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

/// What landed in the last 24 hours.
#[component]
pub(super) async fn crates_view(office: Office) -> topcoat::Result<impl View> {
    let window = office.window();
    let now = office.now;
    let landed: Vec<(String, String, usize, String)> = facts::landed(&office)
        .into_iter()
        .map(|(t, at)| {
            (
                t.id.to_string(),
                t.title.clone(),
                tint(&office, &t.project.to_string()),
                relative(now, at),
            )
        })
        .collect();
    let count = match landed.len() {
        1 => "1 ticket landed".to_string(),
        n => format!("{n} tickets landed"),
    };
    Ok(view! {
        section(label: "Landed", note: format!("in {window}"),
            if landed.is_empty() {
                quiet(text: "Nothing has landed yet.")
            } else {
                <b class="text-[14px] font-semibold">(count)</b>
                <div class="flex flex-col">
                    for (id, title, index, when) in landed {
                        line(id: id, title: title, tint: index, note: when)
                    }
                </div>
            }
        )
    })
}

/// Tickets that failed in the last 24 hours, to try again or cancel.
#[component]
pub(super) async fn returns_view(office: Office) -> topcoat::Result<impl View> {
    let window = office.window();
    let failed: Vec<(String, String, usize, String)> = floor::shipped(&office)
        .into_iter()
        .filter(|(t, _)| t.state.kind() == "failed")
        .map(|(t, _)| {
            let why = ui::failure(&t.state)
                .map(|(head, why)| {
                    if why.is_empty() {
                        head
                    } else {
                        format!("{head}: {why}")
                    }
                })
                .unwrap_or_default();
            (
                t.id.to_string(),
                t.title.clone(),
                tint(&office, &t.project.to_string()),
                why,
            )
        })
        .collect();
    Ok(view! {
        section(label: "Failed", note: format!("in {window}"),
            if failed.is_empty() {
                quiet(text: "Nothing has failed.")
            }
            for (id, title, index, why) in failed {
                <div class="card">
                    line(id: id.clone(), title: title, tint: index)
                    if !why.is_empty() {
                        <p class="m-0 text-[12.5px] text-bad-fg">(why)</p>
                    }
                    <div class=(inspector::ACTIONS)>
                        <button type="button" class=(ui::BUTTON_SM) data-on:click=(format!("@post('/tickets/{id}/retry')"))>
                            ui::icon(name: "refresh-cw", small: true)
                            "Try again"
                        </button>
                        inspector::cancel(id: id)
                    </div>
                </div>
            }
        )
    })
}

/// The approver's and the router's calls today.
#[component]
pub(super) async fn counter_view(office: Office) -> topcoat::Result<impl View> {
    let desk: Vec<(String, String)> = floor::front_desk(&office)
        .into_iter()
        .map(|m| {
            let day = office.today.get(&m.id).cloned().unwrap_or_default();
            let words = if m.role == "approver" {
                format!(
                    "{} allowed · {} denied · {} sent to you",
                    day.allowed, day.denied, day.to_owner
                )
            } else {
                format!("{} routed", day.routed)
            };
            (format!("{} · {}", m.id, capital(&m.role)), words)
        })
        .collect();
    Ok(view! {
        if desk.is_empty() {
            quiet(text: "No approver or router is hired.")
        }
        section(label: "Calls today",
            <div class="card">
                for (who, words) in desk {
                    <div><b class="block text-[13px] font-semibold">(who)</b><span class="text-[12.5px] text-fg-2">(words)</span></div>
                }
            </div>
        )
        quiet(text: "The approver decides the tool calls no rule covers; the router picks who gets each new ticket.")
        <div><a href="/rules" class=(ui::BUTTON)>"See the rules" ui::icon(name: "arrow-right", small: true)</a></div>
    })
}

/// The queued tickets, in the order they start.
#[component]
pub(super) async fn queue_view(office: Office) -> topcoat::Result<impl View> {
    let queued: Vec<(String, String, usize, String)> = office
        .tickets
        .iter()
        .filter(|t| room(t) == Room::UpNext)
        .map(|t| {
            (
                t.id.to_string(),
                t.title.clone(),
                tint(&office, &t.project.to_string()),
                ui::Wait::of(t, &office.tickets, &office.status).short(),
            )
        })
        .collect();
    Ok(view! {
        if queued.is_empty() {
            quiet(text: "Nothing is waiting.")
        } else {
            <ol class="m-0 flex list-none flex-col p-0">
                for (id, title, index, wait) in queued {
                    <li>line(id: id, title: title, tint: index, note: wait)</li>
                }
            </ol>
        }
    })
}

/// Hires with nothing to do, each with a ticket to give.
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
                <button type="button" class=(ui::BUTTON_SM) popovertarget="office-new-ticket" data-on:click=(format!("$pick = '{}'", safe(&id)))>"Give a ticket"</button>
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
    async fn returns_can_be_tried_again_or_cancelled() {
        let html = html_of(office(), extra(), "elements/returns").await;
        assert!(html.contains("API-1"), "{html}");
        assert!(html.contains("Checks failed: clippy"), "{html}");
        assert!(html.contains("@post('/tickets/API-1/retry')"), "{html}");
        assert!(html.contains("Try again"), "{html}");
        assert!(html.contains("/tickets/API-1/cancel"), "{html}");
    }

    #[tokio::test]
    async fn crates_list_what_landed() {
        let html = html_of(office(), extra(), "elements/crates").await;
        assert!(html.contains("1 ticket landed"), "{html}");
        assert!(html.contains("SHOP-2"), "{html}");
        assert!(html.contains("2h ago"), "{html}");
        assert!(!html.contains("API-1"), "{html}");
    }

    #[tokio::test]
    async fn the_counter_counts_calls_and_links_the_rules() {
        let html = html_of(office(), extra(), "elements/counter").await;
        assert!(
            html.contains("7 allowed · 2 denied · 1 sent to you"),
            "{html}"
        );
        assert!(html.contains("0 routed"), "{html}");
        assert!(html.contains("href=\"/rules\""), "{html}");
    }

    #[tokio::test]
    async fn the_queue_is_in_order_with_what_each_waits_for() {
        let html = html_of(office(), extra(), "elements/queue").await;
        let first = html.find(">SHOP-4<").unwrap();
        let second = html.find(">API-3<").unwrap();
        assert!(first < second, "{html}");
        assert!(
            html.contains("After SHOP-1") || html.contains("Waiting for a free desk"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn the_lounge_gives_tickets_to_idle_hires() {
        let html = html_of(office(), extra(), "elements/lounge").await;
        assert!(html.contains("Give a ticket"), "{html}");
        assert!(
            html.contains("popovertarget=\"office-new-ticket\""),
            "{html}"
        );
        assert!(html.contains("$pick = 'reviewer'"), "{html}");
        // dev is at a desk, so it isn't resting.
        assert!(!html.contains("$pick = 'dev'"), "{html}");
    }
}
