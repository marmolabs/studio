//! The Company tab: who reports to whom, then the company's settings: desks,
//! open or closed, house rules, trophies and hiring.

use studiod::api::Trophy;
use topcoat::view::{View, component, view};

use super::rules::rules_pane;
use super::standing::{self, Standing};
use super::team::{Member, member_row, members};
use super::{Extra, capital, phase_pill, quiet, section};
use crate::pages::office::facts::dot_class;
use crate::pages::office::inspector;
use crate::pages::office::{Office, select};
use crate::pages::timeline::relative;
use crate::ui;

/// The most desks the office can have.
const MOST_DESKS: u64 = studiod::api::MAX_RUNS_LIMIT;

/// A project in the chart: its lead and the people on it now.
struct Branch {
    project: Standing,
    /// The lead's card, when the lead is a hire the studio has loaded.
    lead: Option<Member>,
    people: Vec<Member>,
}

/// The chart: the projects with their leads and teams, the free people and
/// the front desk, from the studio's people.
struct Chart {
    branches: Vec<Branch>,
    /// Free: no lead of a project, at no desk, and not at the front desk.
    lounge: Vec<Member>,
    front: Vec<Member>,
}

fn chart(office: &Office, extra: &Extra) -> Chart {
    let [work, resting, front] = members(office, extra);
    let everyone: Vec<Member> = work
        .iter()
        .chain(resting.iter())
        .chain(front.iter())
        .cloned()
        .collect();
    let find = |id: &str| everyone.iter().find(|m| m.id == id).cloned();
    let projects = standing::standings(office);
    let leads: Vec<String> = projects
        .iter()
        .filter_map(|p| p.lead.as_ref().map(|l| l.id.clone()))
        .collect();
    let branches = projects
        .into_iter()
        .map(|project| Branch {
            lead: project.lead.as_ref().and_then(|l| find(&l.id)),
            people: project.team.iter().filter_map(|p| find(&p.id)).collect(),
            project,
        })
        .collect();
    Chart {
        branches,
        lounge: resting
            .into_iter()
            .filter(|m| !leads.contains(&m.id))
            .collect(),
        front,
    }
}

#[component]
pub(super) async fn company_pane(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let Chart {
        branches,
        lounge,
        front,
    } = chart(&office, &extra);
    let desks = office.status.max_runs;
    let closed = office.status.closed;
    let trophies = extra.trophies.clone();
    let rules = extra.rules.clone();
    let now = office.now;
    let nobody = branches.is_empty() && lounge.is_empty() && front.is_empty();
    Ok(view! {
        section(label: "Who reports to whom",
            if nobody {
                quiet(text: "No one is hired yet.")
            }
            <div class="flex flex-col">
                <div class="flex items-center gap-2.5 rounded-xl border border-line-2 bg-bg-4 px-3 py-2">
                    <span class="grid size-[34px] place-items-center rounded-[10px] bg-bg-2 text-fg-2">ui::icon(name: "user-round")</span>
                    <span class="flex flex-col"><b class="text-[14px] font-semibold">"You"</b><small class="text-[12px] text-fg-3">"Owner"</small></span>
                </div>
                <div class="ml-[26px] flex flex-col gap-3 border-l-2 border-line-2 pb-1 pl-[18px] pt-3">
                    for branch in branches {
                        branch_view(branch_lead: branch.lead, project: branch.project, people: branch.people)
                    }
                    if !lounge.is_empty() {
                        <div class="flex flex-col gap-1.5 rounded-[14px] border-[1.5px] border-dashed border-line-2 p-2.5">
                            <div class="sec-h"><span>"Lounge"</span><span>"free"</span></div>
                            for member in lounge { person(member: member) }
                        </div>
                    }
                    if !front.is_empty() {
                        <div class="flex flex-col gap-1.5">
                            <div class="sec-h"><span>"Front desk"</span><span>"answer for everyone"</span></div>
                            for member in front { person(member: member) }
                        </div>
                    }
                </div>
            </div>
            <p class="m-0 text-[12.5px] text-fg-3">"Levels come from the tasks a person has finished."</p>
        )
        section(label: "Desks", note: "agents at once",
            desks_view(desks: desks)
        )
        section(label: "Open or closed",
            open_view(closed: closed)
        )
        rules_pane(rules: rules)
        section(label: "Trophies",
            trophies_view(trophies: trophies, now: now)
        )
        section(label: "Hire someone",
            hire_view()
        )
    })
}

/// A person in the chart, opening their panel.
#[component]
async fn person(member: Member) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="rounded-xl border border-line bg-bg-3 px-2.5 py-1">
            member_row(member: member)
        </div>
    })
}

/// A project's lead, with the people on it now under them.
#[component]
async fn branch_view(
    branch_lead: Option<Member>,
    project: Standing,
    people: Vec<Member>,
) -> topcoat::Result<impl View> {
    let pick = select(&project.key());
    let label = format!("Open {}", project.name);
    Ok(view! {
        <div class=(format!("pj-{} flex flex-col gap-2", project.tint))>
            <div class="flex flex-col gap-1 rounded-xl border border-line border-l-4 border-l-[var(--pj)] bg-bg-3 px-2.5 py-1.5">
                <button type="button" class="flex w-full cursor-pointer items-center gap-2 border-0 bg-transparent p-0 text-left text-[12.5px] text-fg-3" data-on:click=(pick) aria-label=(label)>
                    <i class=(dot_class(project.tint))></i>
                    <b class="min-w-0 truncate font-semibold text-fg">(project.name.clone())</b>
                    <span class="ml-auto">phase_pill(phase: project.phase)</span>
                </button>
                if let Some(lead) = branch_lead {
                    member_row(member: lead)
                } else if let Some(lead) = project.lead.clone() {
                    <span class="text-[13px]"><b class="font-medium">(lead.name)</b> " leads it."</span>
                } else {
                    <span class="text-[13px] text-fg-3">"No lead yet. Open the project to pick one."</span>
                }
            </div>
            if !people.is_empty() {
                <div class="ml-5 flex flex-col gap-1.5 border-l-2 border-line pl-3">
                    for member in people { person(member: member) }
                </div>
            }
        </div>
    })
}

/// How many desks there are, and a way to add or remove one.
#[component]
pub(super) async fn desks_view(desks: u64) -> topcoat::Result<impl View> {
    let words = match desks {
        0 => "Nothing new starts until there is a desk.".to_string(),
        1 => "One agent works at a time. Each desk lets one more work at once.".to_string(),
        n => format!("Up to {n} agents work at once, one at each desk."),
    };
    Ok(view! {
        <div class="flex flex-col gap-1.5">
            <div class="flex items-baseline gap-2">
                <b class="text-[28px] font-bold leading-none tabular-nums">(desks)</b>
                <span class="text-[12.5px] text-fg-3">"desks now"</span>
            </div>
            quiet(text: words)
        </div>
        <div class=(inspector::ACTIONS)>
            if desks < MOST_DESKS {
                desk_button(label: "Add a desk", max_runs: desks + 1, primary: true)
            } else {
                <span class="text-[12.5px] text-fg-3">"That is the most desks the office holds."</span>
            }
            if desks > 1 {
                desk_button(label: "Remove a desk", max_runs: desks - 1)
            }
        </div>
    })
}

/// Sets the number of desks.
#[component]
async fn desk_button(
    label: &'static str,
    max_runs: u64,
    #[default] primary: bool,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <form data-on:submit="@post('/office/desks', {contentType: 'form'})">
            <input type="hidden" name="max_runs" value=(max_runs.to_string())>
            <button type="submit" class=(if primary { ui::PRIMARY } else { ui::BUTTON })>(label)</button>
        </form>
    })
}

/// Open or closed, and the button for the other.
#[component]
pub(super) async fn open_view(closed: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex items-center gap-2">
            ui::dot(state: if closed { "failed" } else { "ready" })
            <b class="text-[14px] font-semibold">(if closed { "The office is closed" } else { "The office is open" })</b>
        </div>
        quiet(text: "Closing the office stops new work from starting. Work already running carries on, and waiting tasks wait until you reopen it.")
        <div class=(inspector::ACTIONS)>
            if closed {
                <button type="button" class=(ui::PRIMARY) data-on:click="@post('/office/open')">"Reopen the office"</button>
            } else {
                <button type="button" class=(ui::BUTTON) data-on:click="@post('/office/close')">"Close the office"</button>
            }
        </div>
    })
}

/// Trophies won, and the next one with how far along it is.
#[component]
pub(super) async fn trophies_view(trophies: Vec<Trophy>, now: u64) -> topcoat::Result<impl View> {
    let won: Vec<(Trophy, String)> = trophies
        .iter()
        .filter_map(|t| {
            let at = t.earned_at?;
            Some((t.clone(), relative(now, at)))
        })
        .collect();
    let next = trophies
        .iter()
        .find(|t| t.earned_at.is_none() && t.progress.is_some())
        .cloned();
    Ok(view! {
        if trophies.is_empty() {
            quiet(text: "No trophies to show yet.")
        }
        if !won.is_empty() {
            <div class="flex flex-col gap-3">
                for (trophy, when) in won {
                    <div class="ach">
                        <span class="medal">"★"</span>
                        <div><b>(trophy.title.clone())</b><span>(trophy.detail.clone())</span></div>
                        <time>(when)</time>
                    </div>
                }
            </div>
        }
        if let Some(trophy) = next {
            <div class="ach locked">
                <span class="medal">"★"</span>
                <div>
                    <b>(trophy.title.clone())</b><span>(trophy.detail.clone())</span>
                    if let Some((done, needed)) = trophy.progress {
                        <div class="mt-1.5">ui::meter(value: done as f64, max: needed as f64, tone: "run")</div>
                    }
                </div>
                if let Some((done, needed)) = trophy.progress {
                    <time>(done) " of " (needed)</time>
                }
            </div>
        }
    })
}

/// The form that hires someone, and a way to the hires editor for more.
#[component]
pub(super) async fn hire_view() -> topcoat::Result<impl View> {
    Ok(view! {
        quiet(text: "Someone new takes a seat in the lounge and picks up work that suits their role.")
        <form class="flex flex-col gap-3" data-on:submit="@post('/office/hires', {contentType: 'form'})">
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"Name"</span>
                <input name="name" required="" maxlength="40" pattern="[a-z0-9][a-z0-9\\-]*" autocomplete="off" spellcheck="false" placeholder="quick-dev" class=(ui::INPUT)>
                <span class=(ui::HINT)>"Lowercase letters, digits and dashes. It is their id and their file's name."</span>
            </label>
            <div class="grid grid-cols-2 gap-3">
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Role"</span>
                    <select name="role" class=(ui::INPUT)>
                        for role in crate::pages::office::ROLES { <option value=(role)>(capital(role))</option> }
                    </select>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Model"</span>
                    <select name="model" class=(ui::INPUT)>
                        for model in crate::pages::office::MODELS { <option value=(model)>(model)</option> }
                    </select>
                </label>
            </div>
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"How they work"</span>
                <input name="about" autocomplete="off" placeholder="Optional. One line on how they work." class=(ui::INPUT)>
            </label>
            <div class=(inspector::ACTIONS)>
                <button type="submit" class=(ui::PRIMARY)>
                    ui::icon(name: "users")
                    "Hire"
                </button>
                <a href="/hires?new" class=(ui::GHOST)>"Write a longer brief in the hires editor"</a>
            </div>
        </form>
    })
}

#[cfg(test)]
mod tests {
    use crate::pages::office::panel::fixtures::{extra, html_on, office};
    use crate::probe::{find, find_containing, text_of, with};

    async fn company(office: crate::pages::office::Office) -> String {
        html_on(office, extra(), "", "company").await
    }

    /// The text of the pane of the Company tab.
    fn pane(html: &str) -> String {
        text_of(find(html, "data-show", "$tab == 'company'").inner_html())
    }

    #[tokio::test]
    async fn the_chart_goes_from_you_to_each_lead_to_the_people_on_the_project() {
        let html = company(office()).await;
        let text = pane(&html);
        // You first, then shop with its lead and who works on it, then api
        // without a lead, then the lounge and the front desk.
        let at = |word: &str| text.find(word).unwrap_or_else(|| panic!("{word}: {text}"));
        assert!(at("You") < at("shop"), "{text}");
        assert!(at("shop") < at("Nora"), "{text}");
        assert!(at("Nora") < at("dev"), "{text}");
        assert!(at("dev") < at("api"), "{text}");
        assert!(text.contains("No lead yet."), "{text}");
        assert!(at("api") < at("Lounge"), "{text}");
        assert!(at("Lounge") < at("reviewer"), "{text}");
        assert!(at("reviewer") < at("Front desk"), "{text}");
        assert!(at("Front desk") < at("approver"), "{text}");
        // Each person opens their panel, a project its page.
        for hire in ["nora", "dev", "reviewer", "approver"] {
            find_containing(&html, "button", "data-on:click", &format!("hires/{hire}"));
        }
        assert!(find(&html, "aria-label", "Open shop").attr_has("data-on:click", "projects/shop"));
        // nora leads shop, so she is not in the lounge as well.
        let lounge = text
            .split("Lounge")
            .nth(1)
            .unwrap_or_default()
            .split("Front desk")
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(!lounge.contains("Nora"), "{lounge}");
        assert!(lounge.contains("reviewer"), "{lounge}");
        // The chart says how people level.
        assert!(text.contains("Levels come from the tasks"), "{text}");
    }

    #[tokio::test]
    async fn a_person_is_under_every_project_they_work_on() {
        // dev has SHOP-3 at a desk; give them a desk task of api as well.
        let mut office = office();
        office
            .tickets
            .push(crate::pages::office::panel::fixtures::ticket(
                "API-9",
                "api",
                serde_json::json!("working"),
                serde_json::json!({}),
            ));
        let html = company(office).await;
        let text = pane(&html);
        let shop = text.find("shop").unwrap_or_default();
        let api = text.find("api").unwrap_or_default();
        let lounge = text.find("Lounge").unwrap_or_default();
        let under = |from: usize, to: usize| text[from..to].matches("Engineer · sonnet").count();
        assert_eq!(under(shop, api), 1, "{text}");
        assert_eq!(under(api, lounge), 1, "{text}");
    }

    #[tokio::test]
    async fn a_lead_is_counted_for_planning_and_deciding() {
        let mut office = office();
        office.today.insert(
            "nora".into(),
            serde_json::from_value(
                serde_json::json!({"planned": 2, "allowed": 7, "denied": 2, "to_owner": 1}),
            )
            .unwrap(),
        );
        let html = company(office).await;
        let nora = find_containing(&html, "button", "data-on:click", "hires/nora").text();
        assert!(nora.contains("Nora"), "{nora}");
        assert!(nora.contains("2 planned"), "{nora}");
        assert!(nora.contains("10 decided"), "{nora}");
    }

    #[tokio::test]
    async fn the_company_holds_the_settings_the_floor_used_to() {
        let html = company(office()).await;
        let text = pane(&html);
        // Desks: how many, and add or remove.
        assert!(text.contains("Up to 3 agents work at once"), "{text}");
        assert!(html.contains("@post('/office/desks'"), "{html}");
        assert!(html.contains("name=\"max_runs\" value=\"4\""), "{html}");
        assert!(html.contains("name=\"max_runs\" value=\"2\""), "{html}");
        // Open or closed.
        assert!(text.contains("The office is open"), "{text}");
        assert!(html.contains("@post('/office/close')"), "{html}");
        // House rules, with their editors.
        assert!(text.contains("House rules"), "{text}");
        assert!(
            !with(&html, "href", "/rules?file=no-push").is_empty(),
            "{html}"
        );
        assert!(!with(&html, "href", "/rules?new").is_empty(), "{html}");
        // Trophies.
        assert!(
            text.contains("First goal merged") || text.contains("First goal"),
            "{text}"
        );
        assert!(
            text.contains("25 tasks") || text.contains("25 tickets"),
            "{text}"
        );
        // Hiring.
        assert!(text.contains("Hire someone"), "{text}");
        assert!(html.contains("@post('/office/hires'"), "{html}");
    }

    #[tokio::test]
    async fn desks_stop_at_the_ends_and_the_office_can_be_reopened() {
        let mut one = office();
        one.status.max_runs = 1;
        assert!(!company(one).await.contains("Remove a desk"));
        let mut most = office();
        most.status.max_runs = 9;
        assert!(!company(most).await.contains("Add a desk"));
        let mut shut = office();
        shut.status.closed = true;
        let html = company(shut).await;
        assert!(pane(&html).contains("The office is closed"), "{html}");
        assert!(html.contains("Reopen the office"), "{html}");
        assert!(html.contains("@post('/office/open')"), "{html}");
        assert!(
            pane(&html).contains("Work already running carries on"),
            "{html}"
        );
    }
}
