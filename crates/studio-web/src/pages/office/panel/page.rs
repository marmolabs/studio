//! A project's page: its phase and pipeline, what its milestone has asked
//! for, the milestone's tasks in four columns, the release, and its lead.

use studio_core::ticket::Ticket;
use studiod::api::AskView;
use topcoat::view::{View, ViewExt, component, view};

use super::release::{checklist, last_release, start_release};
use super::standing::{Mile, Person, Standing};
use super::{capital, phase_pill, pipeline, quiet, section};
use crate::pages::ask_card::ask_card;
use crate::pages::office::facts::dot_class;
use crate::pages::office::inspector::{ACTIONS, BACK};
use crate::pages::office::{Office, key, safe, select};
use crate::ui;

/// The most cards a column shows; the rest are counted.
const COLUMN_SHOWN: usize = 4;

/// The four columns of a milestone's tasks, in order.
const COLUMNS: [&str; 4] = ["To do", "Doing", "Checking", "Done"];

/// Which of `COLUMNS` a task is in.
fn column(ticket: &Ticket) -> usize {
    match ticket.state.kind() {
        "queued" => 0,
        "checking" | "reviewing" | "ready" => 2,
        "landed" => 3,
        // Working, asking, held and failed work is somebody's to deal with.
        _ => 1,
    }
}

/// A task in a column.
#[derive(Clone, Debug, PartialEq)]
struct Card {
    key: String,
    title: String,
    /// Who has it, once it has started.
    hire: Option<Person>,
    /// What a task not started waits for.
    wait: Option<String>,
    /// What it needs, when it needs something.
    flag: Option<&'static str>,
    /// It waits for the owner.
    needs: bool,
}

impl Card {
    fn of(ticket: &Ticket, office: &Office) -> Card {
        let kind = ticket.state.kind();
        let queued = kind == "queued";
        let owner = ui::needs_owner(&ticket.state);
        let (flag, needs) = match kind {
            "asking" => (Some("Needs your OK"), true),
            "held" if owner => (Some("Needs you"), true),
            "held" => (Some("On hold"), false),
            "failed" => (Some("Failed"), false),
            _ => (None, false),
        };
        Card {
            key: key("tickets", &ticket.id.to_string()),
            title: ticket.title.clone(),
            hire: Some(ticket.hire.to_string())
                .filter(|h| !queued && !h.is_empty())
                .map(|h| Person::of(office, &h)),
            wait: queued.then(|| ui::Wait::of(ticket, &office.tickets, &office.status).short()),
            flag,
            needs,
        }
    }
}

/// A milestone's tasks sorted into the columns.
fn columns(mile: &Mile, office: &Office) -> [Vec<Card>; 4] {
    let mut out: [Vec<Card>; 4] = Default::default();
    for task in &mile.tasks {
        out[column(task)].push(Card::of(task, office));
    }
    out
}

/// "Kit", "Kit and Leo", "Kit, Leo and Mei".
fn names(people: &[Person]) -> String {
    match people {
        [] => String::new(),
        [one] => one.name.clone(),
        [init @ .., last] => format!(
            "{} and {}",
            init.iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            last.name
        ),
    }
}

/// What the page says under its title about who is on the project.
fn about(project: &Standing) -> String {
    let team = names(&project.team);
    let are = if project.team.len() == 1 { "is" } else { "are" };
    match (&project.lead, project.team.is_empty()) {
        (Some(lead), true) => format!(
            "{} leads {}. Nobody is working on it right now.",
            lead.name, project.name
        ),
        (Some(lead), false) => format!(
            "{} leads {}. {team} {are} working on it.",
            lead.name, project.name
        ),
        (None, true) => "Nobody leads it or works on it yet.".to_string(),
        (None, false) => format!("{team} {are} working on it. It has no lead yet."),
    }
}

/// A project's page, under the tab bar.
#[component]
pub(super) async fn project_page(office: Office, project: Standing) -> topcoat::Result<impl View> {
    let now = office.now;
    let asks: Vec<AskView> = office
        .asks
        .iter()
        .filter(|a| {
            office
                .ticket(&a.ticket)
                .is_some_and(|t| t.project.to_string() == project.name)
        })
        .cloned()
        .collect();
    let team: Vec<(String, String, String)> = office
        .status
        .team
        .iter()
        .map(|m| {
            (
                m.id.clone(),
                office.name_of(&m.id),
                format!("{} · {}", capital(&m.role), m.model),
            )
        })
        .collect();
    let about = about(&project);
    let milestone = project.milestone.clone();
    let release_shown = project.release.is_some();
    let tint = format!("pj-{}", project.tint);
    let name = project.name.clone();
    let new_work = format!(
        "$pick = ''; (document.querySelector('#office-new-goal [name=project]') || {{}}).value = '{}'",
        safe(&name)
    );
    Ok(view! {
        <div class=(format!("{tint} flex flex-col gap-[18px]"))>
            <button type="button" class=(format!("-my-1 -ml-1.5 self-start {}", ui::GHOST_SM)) data-on:click=(BACK)>
                ui::icon(name: "chevron-left", small: true)
                "Back to the company"
            </button>
            <div class="flex flex-col gap-3">
                <div class="flex items-center gap-2.5">
                    <i class=(format!("{} size-3.5", dot_class(project.tint)))></i>
                    <h2 class="m-0 min-w-0 truncate text-[22px] font-bold leading-[1.15] tracking-[-0.01em]">(name.clone())</h2>
                    phase_pill(phase: project.phase)
                </div>
                pipeline(phase: project.phase)
                <p class="m-0 text-[13px] leading-5 text-fg-2">(about)</p>
            </div>
            for ask in asks {
                ask_card(project: name.clone(), by: office.name_of(&ask.hire), ask: ask, compact: true, now: now)
            }
            milestone_box(milestone: milestone.clone(), led: project.lead.is_some())
            if release_shown {
                checklist(project: project.clone(), now: now)
            }
            board(milestone: milestone, office: office.clone())
            start_release(project: project.clone())
            last_release(project: project.clone(), now: now)
            lead_section(project: name.clone(), lead: project.lead.clone(), team: team)
            <div>
                <button type="button" class=(ui::PRIMARY) popovertarget="office-new-goal" data-on:click=(new_work)>
                    ui::icon(name: "plus")
                    "New work for " (name.clone())
                </button>
            </div>
        </div>
    })
}

/// The milestone: its name and version, how many tasks are done, and a bar.
/// A `led` project has a lead to plan the work.
#[component]
async fn milestone_box(milestone: Option<Mile>, led: bool) -> topcoat::Result<impl View> {
    let Some(mile) = milestone else {
        let words = if led {
            "No milestone is open. Start some work and the lead plans it."
        } else {
            "No milestone is open. Start some work and a planner breaks it into tasks."
        };
        return Ok(view! { quiet(text: words) }.boxed());
    };
    let pick = select(&mile.key);
    let (done, total) = (mile.done(), mile.total());
    Ok(view! {
        <div class="flex flex-col gap-2 rounded-xl border border-line bg-bg-3 p-3">
            <button type="button" class="flex w-full cursor-pointer items-baseline justify-between gap-2 border-0 bg-transparent p-0 text-left" data-on:click=(pick)>
                <b class="min-w-0 truncate text-[15px] font-semibold" title=(mile.name())>(mile.short())</b>
                <span class="whitespace-nowrap text-[12.5px] text-fg-2">(done) " of " (total) " tasks"</span>
            </button>
            ui::meter(value: done as f64, max: total as f64, tone: "run")
        </div>
    }
    .boxed())
}

/// The milestone's tasks in four short columns.
#[component]
async fn board(milestone: Option<Mile>, office: Office) -> topcoat::Result<impl View> {
    let Some(mile) = milestone else {
        return Ok(view! {}.boxed());
    };
    let cols = columns(&mile, &office);
    let (done, total) = (mile.done(), mile.total());
    let note = if total > 0 && done == total {
        format!("all {total} done")
    } else {
        format!("{done} of {total} done")
    };
    let columns: Vec<(&str, usize, Vec<Card>, usize)> = COLUMNS
        .iter()
        .zip(cols)
        .map(|(title, cards)| {
            let count = cards.len();
            let hidden = count.saturating_sub(COLUMN_SHOWN);
            let shown: Vec<Card> = cards.into_iter().take(COLUMN_SHOWN).collect();
            (*title, count, shown, hidden)
        })
        .collect();
    Ok(view! {
        section(label: "Tasks", note: note,
            if total == 0 {
                quiet(text: "No tasks are planned yet.")
            } else {
                <div class="grid grid-cols-4 gap-1.5">
                    for (title, count, cards, hidden) in columns {
                        <div class="flex min-w-0 flex-col gap-1.5 rounded-[10px] bg-bg-3 px-[5px] pb-1.5 pt-2" data-column=(title)>
                            <h4 class="m-0 flex items-baseline justify-between gap-1 pl-0.5 pr-0.5 text-[11.5px] font-semibold text-fg-2">
                                <span>(title)</span>
                                <em class="text-[11px] not-italic">(count)</em>
                            </h4>
                            if cards.is_empty() {
                                <span class="rounded-lg border border-dashed border-line px-1.5 py-1.5 text-[11.5px] italic text-fg-3">
                                    (match title {
                                        "To do" => "Nothing waiting",
                                        "Doing" => "Nothing in progress",
                                        "Checking" => "Nothing to check",
                                        _ => "Nothing done yet",
                                    })
                                </span>
                            }
                            for card in cards {
                                task_card(card: card)
                            }
                            if hidden > 0 {
                                <span class="px-1 py-0.5 text-[11.5px] text-fg-3">"+" (hidden) " more"</span>
                            }
                        </div>
                    }
                </div>
            }
        )
    }
    .boxed())
}

/// A task in a column: its title, what it needs and who has it. Picking it
/// opens its panel.
#[component]
async fn task_card(card: Card) -> topcoat::Result<impl View> {
    let pick = select(&card.key);
    let class = if card.needs {
        "border-attn bg-attn-bg"
    } else {
        "border-line bg-bg"
    };
    let who = card.wait.clone();
    Ok(view! {
        <button type="button" class=(format!("flex w-full min-w-0 cursor-pointer flex-col gap-1.5 rounded-lg border p-[7px] text-left text-[12px] leading-[1.25] text-fg {class}")) data-on:click=(pick)>
            <b class="[overflow-wrap:anywhere] text-[12px] font-semibold">(card.title.clone())</b>
            if let Some(flag) = card.flag {
                <span class=(if card.needs { "text-[11px] font-semibold text-attn-fg" } else { "text-[11px] font-semibold text-fg-3" })>(flag)</span>
            }
            if let Some(words) = who {
                <span class="text-[11px] text-fg-3">(words)</span>
            } else if let Some(hire) = card.hire.clone() {
                <span class="flex flex-wrap items-center gap-1 text-[11px] text-fg-3">
                    ui::avatar(hire: hire.id, name: hire.name.clone())
                    (hire.name)
                </span>
            }
        </button>
    })
}

/// The lead, with a way to change it: any of the team, or none.
#[component]
async fn lead_section(
    project: String,
    lead: Option<Person>,
    team: Vec<(String, String, String)>,
) -> topcoat::Result<impl View> {
    let current = lead.as_ref().map(|l| l.id.clone()).unwrap_or_default();
    let pick = lead.as_ref().map(|l| select(&key("hires", &l.id)));
    Ok(view! {
        section(label: "Lead",
            if let Some(lead) = lead {
                <button type="button" class="grid w-full cursor-pointer grid-cols-[34px_minmax(0,1fr)] items-center gap-2.5 border-0 bg-transparent p-0 text-left" data-on:click=(pick.unwrap_or_default())>
                    ui::avatar(hire: lead.id.clone(), name: lead.name.clone(), large: true)
                    <span class="flex min-w-0 flex-col">
                        <b class="font-semibold">(lead.name.clone())</b>
                        <small class="text-[12px] text-fg-3">"Plans the work and answers the team. Asks you only when it can't decide."</small>
                    </span>
                </button>
            } else {
                quiet(text: "No lead yet. A lead plans the work and answers the team's routine questions.")
            }
            <form class=(format!("{ACTIONS} items-center")) data-on:submit="@post('/office/projects/lead', {contentType: 'form'})">
                <input type="hidden" name="project" value=(project)>
                <select name="hire" class=(format!("{} min-w-0 flex-1", ui::INPUT)) aria-label="Lead">
                    <option value="" selected=(current.is_empty().then_some(""))>"No lead"</option>
                    for (id, name, about) in team {
                        <option value=(id.clone()) selected=((id == current).then_some(""))>(name) " · " (about)</option>
                    }
                </select>
                <button type="submit" class=(ui::BUTTON)>"Change lead"</button>
            </form>
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::pages::office::inspector;
    use crate::pages::office::panel::fixtures::{extra, html_of, office, release, ticket};
    use crate::probe::{find, find_containing, named, text_of, with};
    use serde_json::json;
    use studiod::api::ReleaseState;

    async fn page(office: crate::pages::office::Office) -> String {
        html_of(office, extra(), "projects/shop").await
    }

    #[tokio::test]
    async fn the_page_has_the_name_phase_pipeline_and_a_way_back() {
        let html = page(office()).await;
        let text = text_of(&html);
        assert!(text.contains("Back to the company"), "{html}");
        assert!(html.contains(inspector::BACK), "{html}");
        let title = named(&html, "h2");
        assert_eq!(title.len(), 1, "{html}");
        assert_eq!(title[0].text(), "shop");
        assert!(text.contains("Releasing"), "{text}");
        // The tabs stay above it, with Projects the one that is on.
        assert!(
            find(&html, "role", "tablist")
                .inner_html()
                .contains("Company"),
            "{html}"
        );
        let projects = find(
            &html,
            "data-on:click",
            "$tab = 'projects'; $sel = ''; @get('/office/panel')",
        );
        assert!(projects.attr_is("aria-selected", "true"), "{html}");
        // The pipeline: Releasing lit, what came before ticked.
        let lit = with(&html, "aria-current", "step");
        assert_eq!(lit.len(), 1, "{html}");
        assert_eq!(lit[0].text(), "Releasing");
        let steps = with(&html, "role", "listitem");
        let marks: Vec<String> = steps
            .iter()
            .map(|s| s.attr("data-step").unwrap_or_default())
            .collect();
        assert_eq!(marks, ["done", "done", "done", "on", ""], "{html}");
        // Who is on it.
        assert!(
            text.contains("Nora leads shop. dev is working on it."),
            "{text}"
        );
    }

    #[tokio::test]
    async fn the_tasks_of_the_milestone_are_in_four_columns() {
        let mut office = office();
        office.tickets[3] = ticket(
            "SHOP-4",
            "shop",
            json!({"asking": {"ask": "a1"}}),
            json!({"parent": "SHOP-1", "title": "Pay with a card"}),
        );
        office.tickets.push(ticket(
            "SHOP-6",
            "shop",
            json!("queued"),
            json!({"parent": "SHOP-1"}),
        ));
        office.tickets.push(ticket(
            "SHOP-7",
            "shop",
            json!("reviewing"),
            json!({"parent": "SHOP-1"}),
        ));
        let html = page(office).await;
        let column = |name: &str| find(&html, "data-column", name).inner_html().to_string();
        let todo = column("To do");
        assert!(todo.contains("Title SHOP-6"), "{todo}");
        assert!(todo.contains("Waiting for a free desk"), "{todo}");
        // Working and asking work are in Doing, the asking one flagged.
        let doing = column("Doing");
        assert!(doing.contains("Title SHOP-3"), "{doing}");
        assert!(
            doing.contains("Pay with a card") || doing.contains("Title SHOP-4"),
            "{doing}"
        );
        assert!(doing.contains("Needs your OK"), "{doing}");
        assert!(doing.contains("dev"), "{doing}");
        assert!(column("Checking").contains("Title SHOP-7"));
        assert!(column("Done").contains("Title SHOP-2"));
        // Each opens the task's panel.
        assert!(
            doing.contains("$sel = 'tickets/SHOP-3'; @get('/office/panel')"),
            "{doing}"
        );
        // The counts head each column.
        let heads: Vec<String> = named(&html, "h4").iter().map(|h| h.text()).collect();
        assert_eq!(
            heads,
            ["To do 1", "Doing 2", "Checking 1", "Done 1"],
            "{html}"
        );
        let text = text_of(&html);
        assert!(text.contains("Tasks"), "{text}");
        assert!(text.contains("1 of 5 done"), "{text}");
    }

    #[tokio::test]
    async fn a_long_column_counts_what_it_leaves_out() {
        let mut office = office();
        for i in 10..16 {
            office.tickets.push(ticket(
                &format!("SHOP-{i}"),
                "shop",
                json!("landed"),
                json!({"parent": "SHOP-1"}),
            ));
        }
        let html = page(office).await;
        let done = find(&html, "data-column", "Done").text();
        assert!(done.contains("Done 7"), "{done}");
        assert!(done.contains("+3 more"), "{done}");
    }

    #[tokio::test]
    async fn a_release_waiting_shows_its_checklist_with_approve_and_cancel() {
        let html = page(office()).await;
        let text = text_of(&html);
        assert!(text.contains("Release v1.2"), "{text}");
        assert!(text.contains("1 of 5 steps"), "{text}");
        // The steps read in plain order, each release command as a step.
        assert!(
            text.contains("Final checks passed. Next: your go-ahead, then it publishes v1.2."),
            "{text}"
        );
        let rows = with(&html, "data-release-step", "done");
        assert_eq!(rows.len(), 1, "{html}");
        for label in ["Release step 1", "Release step 2", "Tag v1.2"] {
            assert!(text.contains(label), "{label}: {text}");
        }
        let commands: Vec<String> = named(&html, "small")
            .iter()
            .filter(|c| c.attr_has("class", "font-mono"))
            .map(|c| c.text())
            .collect();
        assert_eq!(commands, ["./scripts/build.sh", "./scripts/publish.sh"]);
        let waiting = find(&html, "data-release-step", "waiting");
        let words = waiting.text();
        assert!(words.contains("Your go-ahead"), "{words}");
        assert!(words.contains("Needs your OK"), "{words}");
        assert!(words.contains("Approve"), "{words}");
        assert!(words.contains("Cancel"), "{words}");
        let inner = waiting.inner_html();
        assert!(inner.contains("/office/releases/approve"), "{inner}");
        assert!(inner.contains("/office/releases/cancel"), "{inner}");
        assert!(inner.contains("name=\"project\" value=\"shop\""), "{inner}");
        // Nothing can start while it is under way.
        assert!(!text.contains("Start a release"), "{text}");
        assert!(
            with(
                &html,
                "data-on:submit",
                "@post('/office/releases/start', {contentType: 'form'})"
            )
            .is_empty()
        );
    }

    #[tokio::test]
    async fn while_it_checks_only_cancel_is_offered() {
        let mut office = office();
        office.projects[0].release = Some(release(ReleaseState::Checking, 1));
        let html = page(office).await;
        assert!(html.contains("/office/releases/cancel"), "{html}");
        assert!(!html.contains("/office/releases/approve"), "{html}");
        assert!(text_of(&html).contains("Cancel the release"), "{html}");
        assert!(
            with(&html, "data-release-step", "waiting").is_empty(),
            "{html}"
        );
    }

    #[tokio::test]
    async fn a_project_that_can_be_released_starts_a_release_with_a_version() {
        let mut office = office();
        office.projects[0].release = None;
        let html = page(office).await;
        let form = find_containing(&html, "form", "data-on:submit", "/office/releases/start");
        let inner = form.inner_html();
        assert!(inner.contains("name=\"project\" value=\"shop\""), "{inner}");
        assert!(inner.contains("name=\"version\""), "{inner}");
        assert!(form.text().contains("Start a release"), "{html}");
        // Its last release is told.
        assert!(text_of(&html).contains("v0.3 went out 4d ago."), "{html}");
        // Without release commands there is nothing to start.
        let mut plain = office_without_release();
        plain.projects[0].releasable = false;
        let html = page(plain).await;
        assert!(
            with(
                &html,
                "data-on:submit",
                "@post('/office/releases/start', {contentType: 'form'})"
            )
            .is_empty(),
            "{html}"
        );
        // It says how to add them, with a short example.
        let text = text_of(&html);
        assert!(text.contains("no release commands"), "{html}");
        assert!(text.contains("/work/shop/.studio.toml"), "{text}");
        assert!(
            text.contains(r#"release = [["./scripts/publish.sh"]]"#),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_live_project_suggests_the_next_version() {
        let version = |html: &str| {
            find_containing(html, "input", "name", "version")
                .attr("value")
                .unwrap_or_default()
        };
        // Live: nothing open, a release out. shop's last is v0.3.
        let mut live = office();
        live.tickets.retain(|t| t.project.to_string() != "shop");
        live.projects[0].release = None;
        assert_eq!(version(&page(live).await), "0.4");
        // Still building: nothing is suggested.
        let building = page(office_without_release()).await;
        assert_eq!(version(&building), "", "{building}");
    }

    #[tokio::test]
    async fn a_long_goal_is_a_short_title_with_the_whole_on_hover() {
        let mut office = office();
        let long = "Give the store its look: logo, product cards, launch email";
        office.tickets[0] = ticket(
            "SHOP-1",
            "shop",
            json!("landed"),
            json!({"kind": "plan", "goal": long, "version": "0.4"}),
        );
        let html = page(office).await;
        let text = text_of(&html);
        assert!(text.contains("v0.4 Give the store its look…"), "{text}");
        assert!(!text.contains("launch email"), "{text}");
        assert!(html.contains(&format!("title=\"v0.4 {long}\"")), "{html}");
    }

    fn office_without_release() -> crate::pages::office::Office {
        let mut office = office();
        office.projects[0].release = None;
        office
    }

    #[tokio::test]
    async fn the_lead_can_be_changed_or_cleared() {
        let html = page(office()).await;
        let form = find_containing(&html, "form", "data-on:submit", "/office/projects/lead");
        let inner = form.inner_html();
        assert!(inner.contains("name=\"project\" value=\"shop\""), "{inner}");
        assert!(form.text().contains("Change lead"), "{inner}");
        // None clears it; each of the team is a choice, the lead's picked.
        let options = named(inner, "option");
        let values: Vec<String> = options
            .iter()
            .map(|o| o.attr("value").unwrap_or_default())
            .collect();
        assert_eq!(
            values,
            ["", "dev", "nora", "reviewer", "approver", "router"],
            "{inner}"
        );
        // Each is shown by name, the lead among them.
        assert!(form.text().contains("Nora · Lead · opus"), "{inner}");
        assert!(!text_of(&html).contains("prepares releases"), "{html}");
        let picked: Vec<String> = options
            .iter()
            .filter(|o| o.has("selected"))
            .map(|o| o.attr("value").unwrap_or_default())
            .collect();
        assert_eq!(picked, ["nora"], "{inner}");
        // The lead opens their panel.
        assert!(html.contains("$sel = 'hires/nora'"), "{html}");
        // A project with none has the empty choice picked.
        let api = html_of(office(), extra(), "projects/api").await;
        let options = named(&api, "option");
        let picked: Vec<String> = options
            .iter()
            .filter(|o| o.has("selected"))
            .map(|o| o.attr("value").unwrap_or_default())
            .collect();
        assert_eq!(picked, [""], "{api}");
        assert!(text_of(&api).contains("No lead yet."), "{api}");
    }

    #[tokio::test]
    async fn new_work_opens_the_form_for_this_project() {
        let html = page(office()).await;
        let button = find_containing(&html, "button", "popovertarget", "office-new-goal");
        assert!(button.text().contains("New work for shop"), "{html}");
        assert!(button.attr_has("data-on:click", "value = 'shop'"), "{html}");
    }

    #[tokio::test]
    async fn the_asks_of_the_project_are_on_its_page() {
        let html = page(office()).await;
        assert!(html.contains("@post('/asks/a1'"), "{html}");
        let api = html_of(office(), extra(), "projects/api").await;
        assert!(!api.contains("@post('/asks/a1'"), "{api}");
        // A project the office does not have falls back to the tabs.
        let none = html_of(office(), extra(), "projects/nope").await;
        assert!(!with(&none, "role", "tablist").is_empty(), "{none}");
        assert!(!none.contains("Back to the company"), "{none}");
    }

    #[tokio::test]
    async fn a_milestone_with_no_tasks_says_so() {
        let mut office = office();
        office.tickets.retain(|t| t.parent.is_none());
        let html = page(office).await;
        assert!(
            text_of(&html).contains("No tasks are planned yet."),
            "{html}"
        );
        // Without a lead, a planner breaks new work into tasks.
        let api = html_of(
            crate::pages::office::panel::fixtures::office(),
            extra(),
            "projects/api",
        )
        .await;
        let text = text_of(&api);
        assert!(text.contains("No milestone is open."), "{api}");
        assert!(text.contains("a planner breaks it into tasks"), "{text}");
        assert!(!text.contains("the lead plans it"), "{text}");
    }
}
