//! The For you tab: everything across the projects that waits for the owner,
//! each card naming its project with its colour.

use studio_core::ticket::Ticket;
use studiod::api::AskView;
use topcoat::view::{View, component, view};

use super::release::release_card;
use super::standing;
use super::{needs_count, project_tag, quiet, tint};
use crate::pages::ask_card::ask_card;
use crate::pages::office::facts;
use crate::pages::office::floor::{self, Room, room};
use crate::pages::office::inspector;
use crate::pages::office::{Office, key, select};
use crate::ui;

/// A row's frame in a list of what waits.
const ROW: &str = "grid grid-cols-[16px_minmax(0,1fr)_auto] items-center gap-3 rounded-[10px] border border-line px-3 py-2";

/// What the owner's leads answered today, from what the office knows of
/// each lead's day: the calls it allowed or denied.
fn answered_by_leads(office: &Office) -> (Vec<String>, u32) {
    let mut leads: Vec<String> = Vec::new();
    for lead in office
        .projects
        .iter()
        .filter_map(|p| p.lead.clone())
        .filter(|l| !l.is_empty())
    {
        if !leads.contains(&lead) {
            leads.push(lead);
        }
    }
    let answered = leads
        .iter()
        .filter_map(|lead| office.today.get(lead))
        .map(|day| day.allowed + day.denied)
        .sum();
    (leads, answered)
}

/// A finished milestone, every task done, ready to ship.
struct Finished {
    key: String,
    name: String,
    project: String,
    tint: usize,
    post: String,
    base: String,
}

/// A task that failed lately, with why.
struct Failure {
    id: String,
    title: String,
    project: String,
    tint: usize,
    why: String,
}

/// What waits for the owner: releases that wait for a go-ahead, asks,
/// held work, what is ready to ship, failed work and, apart, holds that end
/// on their own.
#[component]
pub(super) async fn needs_pane(office: Office) -> topcoat::Result<impl View> {
    let waiting = facts::needs_you(&office);
    let count = needs_count(&office);
    let held: Vec<Ticket> = waiting.held.into_iter().cloned().collect();
    let on_hold: Vec<Ticket> = waiting.on_hold.into_iter().cloned().collect();
    let releases: Vec<standing::Standing> = standing::standings(&office)
        .into_iter()
        .filter(|s| s.waiting().is_some())
        .collect();
    let ready: Vec<Ticket> = office
        .tickets
        .iter()
        .filter(|t| room(t) == Room::Ready)
        .cloned()
        .collect();
    // Milestones whose tasks have all landed wait for the owner to ship them.
    let finished: Vec<Finished> = floor::goals(&office)
        .into_iter()
        .filter(|g| g.state() == "done")
        .filter_map(|g| {
            let landed = g.filed.iter().find(|t| t.state.kind() == "landed")?;
            let project = g.plan.project.to_string();
            Some(Finished {
                key: key("goals", &g.plan.id.to_string()),
                name: g.name(),
                tint: tint(&office, &project),
                project,
                post: format!("@post('/tickets/{}/merge')", landed.id),
                base: landed.base_branch.clone(),
            })
        })
        .collect();
    let failed: Vec<Failure> = floor::shipped(&office)
        .into_iter()
        .filter(|(t, _)| t.state.kind() == "failed")
        .map(|(t, _)| {
            let project = t.project.to_string();
            let why = ui::failure(&t.state)
                .map(|(head, why)| {
                    if why.is_empty() {
                        head
                    } else {
                        format!("{head}: {why}")
                    }
                })
                .unwrap_or_default();
            Failure {
                id: t.id.to_string(),
                title: t.title.clone(),
                tint: tint(&office, &project),
                project,
                why,
            }
        })
        .collect();
    let asks: Vec<(AskView, String, usize)> = office
        .asks
        .iter()
        .map(|ask| {
            let project = office
                .ticket(&ask.ticket)
                .map(|t| t.project.to_string())
                .unwrap_or_default();
            let index = tint(&office, &project);
            (ask.clone(), project, index)
        })
        .collect();
    let (leads, answered) = answered_by_leads(&office);
    let lead_faces: Vec<standing::Person> = leads
        .iter()
        .map(|id| standing::Person::of(&office, id))
        .collect();
    let projects_with_things = {
        let mut names: Vec<&str> = releases.iter().map(|s| s.name.as_str()).collect();
        names.extend(asks.iter().map(|(_, p, _)| p.as_str()));
        names.extend(held.iter().map(|t| t.project.0.as_str()));
        names.sort_unstable();
        names.dedup();
        names.retain(|n| !n.is_empty());
        names.len()
    };
    let now = office.now;
    // A new studio starts bare: the first steps come before anything else.
    let no_projects = office.status.projects.is_empty();
    let start = no_projects || office.tickets.is_empty();
    let nothing = count == 0 && ready.is_empty() && finished.is_empty() && failed.is_empty();
    Ok(view! {
        if start {
            start_here(no_projects: no_projects)
        }
        if count > 0 {
            <div class="sec-h">
                <span>"Waiting for you"</span>
                <span>"Across " (projects_with_things) if projects_with_things == 1 { " project" } else { " projects" }</span>
            </div>
        }
        if nothing {
            <p class="m-0 text-[13px] text-fg-3">"Nothing is waiting for you right now."</p>
        }
        for project in releases {
            release_card(project: project, now: now)
        }
        for (ask, project, index) in asks {
            <div class="flex flex-col gap-1.5">
                if !project.is_empty() {
                    project_button(name: project.clone(), tint: index)
                }
                ask_card(project: project, by: office.name_of(&ask.hire), ask: ask, compact: true, now: now)
            </div>
        }
        for ticket in held {
            held_row(project: ticket.project.to_string(), tint: tint(&office, &ticket.project.to_string()), ticket: ticket)
        }
        if !ready.is_empty() || !finished.is_empty() {
            <section>
                <h3 class=(inspector::SECTION_LABEL)>"Ready to ship"</h3>
                <div class="flex flex-col gap-2">
                    for ticket in ready {
                        ready_row(project: ticket.project.to_string(), tint: tint(&office, &ticket.project.to_string()), size: office.sizes.get(&ticket.id.to_string()).copied(), ticket: ticket)
                    }
                    for done in finished {
                        <div class=(ROW)>
                            ui::state_icon(state: "landed")
                            <button type="button" class="flex min-w-0 cursor-pointer flex-col text-left" data-on:click=(select(&done.key))>
                                <span class="truncate font-medium" title=(done.name.clone())>(done.name.clone())</span>
                                <span class="flex items-center gap-1.5 truncate text-[12.5px] text-fg-3">
                                    <i class=(facts::dot_class(done.tint))></i>
                                    (done.project.clone()) " · every task is done"
                                </span>
                            </button>
                            <button type="button" class=(ui::BUTTON_SM) data-on:click=(done.post.clone())>
                                ui::icon(name: "git-merge", small: true)
                                "Ship to " (done.base.clone())
                            </button>
                        </div>
                    }
                </div>
            </section>
        }
        if !failed.is_empty() {
            <section>
                <h3 class=(inspector::SECTION_LABEL)>"Failed lately"</h3>
                <div class="flex flex-col gap-2">
                    for failure in failed {
                        <div class="flex flex-col gap-1.5 rounded-[10px] border border-line px-3 py-2">
                            <span class="flex items-center gap-1.5 text-[12.5px] text-fg-3">
                                <i class=(facts::dot_class(failure.tint))></i>
                                (failure.project.clone())
                            </span>
                            super::line(id: failure.id.clone(), title: failure.title.clone(), tint: failure.tint)
                            if !failure.why.is_empty() {
                                <p class="m-0 text-[12.5px] text-bad-fg">(failure.why.clone())</p>
                            }
                            <div class=(inspector::ACTIONS)>
                                <button type="button" class=(ui::BUTTON_SM) data-on:click=(format!("@post('/tickets/{}/retry')", failure.id))>
                                    ui::icon(name: "refresh-cw", small: true)
                                    "Try again"
                                </button>
                                inspector::cancel(id: failure.id.clone())
                            </div>
                        </div>
                    }
                </div>
            </section>
        }
        if !on_hold.is_empty() {
            <section>
                <h3 class=(inspector::SECTION_LABEL)>"On hold, starts by itself"</h3>
                <div class="flex flex-col gap-2">
                    for ticket in on_hold {
                        held_row(project: ticket.project.to_string(), tint: tint(&office, &ticket.project.to_string()), ticket: ticket)
                    }
                </div>
            </section>
        }
        if answered > 0 {
            <div class="flex flex-col gap-1.5 rounded-xl border border-line px-3 py-2.5">
                <div class="flex items-center gap-2.5">
                    super::faces(people: lead_faces)
                    <b class="text-[13px] font-semibold">
                        "Your leads answered " (answered) if answered == 1 { " routine question" } else { " routine questions" } " today."
                    </b>
                </div>
                quiet(text: "Leads answer routine questions themselves and only pass up what they can't decide.")
            </div>
        }
    })
}

/// A project's name and colour, opening the project.
#[component]
async fn project_button(name: String, tint: usize) -> topcoat::Result<impl View> {
    let pick = select(&key("projects", &name));
    Ok(view! {
        <button type="button" class="flex w-fit max-w-full cursor-pointer items-center gap-2 border-0 bg-transparent p-0 text-left hover:underline" data-on:click=(pick) aria-label=(format!("Open {name}"))>
            project_tag(name: name, tint: tint)
        </button>
    })
}

/// The first steps of a new studio: add a project, then start some work.
#[component]
async fn start_here(no_projects: bool) -> topcoat::Result<impl View> {
    let add = select("elements/add-project");
    Ok(view! {
        <section>
            <h3 class=(inspector::SECTION_LABEL)>"Start here"</h3>
            <div class="flex flex-col gap-2">
                if no_projects {
                    <button type="button" class=(ui::PRIMARY) data-on:click=(add)>
                        ui::icon(name: "plus")
                        "Add a project"
                    </button>
                }
                <button type="button" class=(if no_projects { ui::BUTTON } else { ui::PRIMARY })
                    popovertarget="office-new-goal" disabled=(no_projects.then_some("")) data-on:click=(crate::pages::office::NO_PICK)>
                    ui::icon(name: "target")
                    "Start new work"
                </button>
            </div>
            if no_projects {
                <p class="mt-2 text-[12.5px] text-fg-3">"Work needs a project, so add one first."</p>
            }
        </section>
    })
}

/// Held work: its title and why, in its project, and its panel.
#[component]
async fn held_row(ticket: Ticket, project: String, tint: usize) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    Ok(view! {
        <div class=(ROW)>
            ui::state_icon(state: ui::shown(&ticket.state))
            <div class="flex min-w-0 flex-col">
                <span class="truncate font-medium">(ticket.title.clone())</span>
                <span class="flex items-center gap-1.5 truncate text-[12.5px] text-fg-3">
                    <i class=(facts::dot_class(tint))></i>
                    (project) " · " (why)
                </span>
            </div>
            <div class="flex items-center gap-1.5">
                <button type="button" class=(ui::BUTTON_SM) data-on:click=(format!("@post('/tickets/{id}/resume')")) aria-label=(format!("Resume {id}"))>"Resume"</button>
                <button type="button" class=(ui::GHOST_SM) data-on:click=(select(&key("tickets", &id)))>"Details"</button>
            </div>
        </div>
    })
}

/// Work that is ready to ship, with its size and a button.
#[component]
async fn ready_row(
    ticket: Ticket,
    project: String,
    tint: usize,
    size: Option<(u64, u64)>,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let ship = format!("@post('/tickets/{id}/land')");
    Ok(view! {
        <div class=(ROW)>
            ui::state_icon(state: "ready")
            <button type="button" class="flex min-w-0 cursor-pointer flex-col text-left" data-on:click=(select(&key("tickets", &id)))>
                <span class="truncate font-medium" title=(ticket.title.clone())>(ticket.title.clone())</span>
                <span class="flex items-center gap-1.5 truncate text-[12.5px] text-fg-3">
                    <i class=(facts::dot_class(tint))></i>
                    (project)
                    if let Some((added, removed)) = size {
                        " · " ui::diff_size(added: added, removed: removed)
                    }
                </span>
            </button>
            <button type="button" class=(ui::BUTTON_SM) data-on:click=(ship) aria-label=(format!("Ship {id}"))>"Ship"</button>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use crate::pages::office::panel::fixtures::{extra, html_of, office, pane, ticket};
    use crate::probe::{find, find_containing, text_of};
    use serde_json::json;

    #[tokio::test]
    async fn a_release_waiting_for_a_go_ahead_is_a_card_with_approve() {
        let html = html_of(office(), extra(), "").await;
        let text = text_of(&html);
        assert!(text.contains("Publish shop v1.2?"), "{text}");
        // The steps in plain order.
        assert!(
            text.contains("Final checks passed. Next: your go-ahead, then it publishes v1.2."),
            "{text}"
        );
        assert!(!text.contains("steps are done"), "{text}");
        let approve = find_containing(&html, "form", "data-on:submit", "/office/releases/approve");
        assert!(approve.inner_html().contains("value=\"shop\""), "{html}");
        assert!(approve.text().contains("Approve"), "{html}");
        let not_yet = find_containing(&html, "form", "data-on:submit", "/office/releases/cancel");
        assert!(not_yet.text().contains("Not yet"), "{html}");
        // The card says whose lead it is and opens the project.
        assert!(text.contains("Nora, lead"), "{text}");
        assert!(html.contains("$sel = 'projects/shop'"), "{html}");
        // It sits in the shop's colour.
        assert!(html.contains("pj-0"), "{html}");
    }

    #[tokio::test]
    async fn an_ask_names_its_project_and_answers_in_place() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("@post('/asks/a1'"), "{html}");
        for word in ["Allow", "Deny", "Always allow", "Note for the agent"] {
            assert!(html.contains(word), "{word}: {html}");
        }
        // The ask's project above it, opening that project.
        let open = find(&html, "aria-label", "Open shop");
        assert!(open.attr_has("data-on:click", "projects/shop"), "{html}");
        assert!(open.text().contains("shop"), "{html}");
    }

    #[tokio::test]
    async fn work_ready_to_ship_failed_and_held_work_wait_here() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("Ready to ship"), "{html}");
        assert!(html.contains("@post('/tickets/SHOP-5/land')"), "{html}");
        assert!(html.contains("aria-label=\"Ship SHOP-5\""), "{html}");
        // Held for the owner, with a way on.
        assert!(html.contains("@post('/tickets/API-2/resume')"), "{html}");
        // Failed lately: try again or cancel.
        assert!(html.contains("Failed lately"), "{html}");
        assert!(html.contains("Checks failed: clippy"), "{html}");
        assert!(html.contains("@post('/tickets/API-1/retry')"), "{html}");
        assert!(html.contains("Try again"), "{html}");
        assert!(html.contains("/tickets/API-1/cancel"), "{html}");
        // In the plain words of the owner.
        let text = text_of(pane(&html, "needs"));
        for word in ["ticket", "Land", "Merge", "producer"] {
            assert!(!text.contains(word), "{word}: {text}");
        }
    }

    #[tokio::test]
    async fn a_finished_milestone_is_shipped_to_its_branch() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets.remove(3);
        let html = html_of(office, extra(), "").await;
        assert!(html.contains("every task is done"), "{html}");
        assert!(html.contains("Ship to main"), "{html}");
        assert!(html.contains("@post('/tickets/SHOP-2/merge')"), "{html}");
    }

    #[tokio::test]
    async fn when_nothing_waits_it_says_so_and_what_the_leads_handled() {
        let mut quiet = office();
        quiet.asks.clear();
        quiet.projects[0].release = None;
        quiet.tickets.retain(|t| {
            t.state.kind() == "landed" || t.state.kind() == "working" || t.state.kind() == "queued"
        });
        quiet.recent.clear();
        quiet.today.insert(
            "nora".into(),
            serde_json::from_value(json!({"allowed": 7, "denied": 2})).unwrap(),
        );
        let html = html_of(quiet.clone(), extra(), "").await;
        let text = text_of(&html);
        assert!(
            text.contains("Nothing is waiting for you right now."),
            "{text}"
        );
        assert!(
            text.contains("Your leads answered 9 routine questions today."),
            "{text}"
        );
        // Without a lead's day there is nothing to say about it.
        quiet.today.clear();
        let text = text_of(&html_of(quiet, extra(), "").await);
        assert!(!text.contains("Your leads answered"), "{text}");
    }

    #[tokio::test]
    async fn a_new_studio_starts_with_the_first_steps() {
        let mut bare = office();
        bare.status.projects.clear();
        bare.projects.clear();
        bare.tickets.clear();
        bare.asks.clear();
        let html = html_of(bare, extra(), "").await;
        assert!(html.contains("Start here"), "{html}");
        assert!(html.contains("$sel = 'elements/add-project'"), "{html}");
        assert!(html.contains("Start new work"), "{html}");
    }
}
