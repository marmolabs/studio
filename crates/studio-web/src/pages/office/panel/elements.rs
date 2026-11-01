//! The panels of the things on the floor: board, desks, racks, safe and the rest.

use studiod::api::{ProjectCheck, RuleFile, Trophy};
use topcoat::context::Cx;
use topcoat::view::{BoxView, View, ViewExt, component, view};

use super::money::{limits_form, spend_summary};
use super::queues::{
    counter_view, crates_view, lounge_view, queue_view, returns_view, review_view,
};
use super::rules::{new_rule, rule_cards};
use super::team::{Member, hire_link, members};
use super::{Budget, Extra, capital, line, quiet, section, tint};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::floor::{self};
use crate::pages::office::inspector::{self};
use crate::pages::office::model::Element;
use crate::pages::office::{Office, key, select};
use crate::pages::timeline::relative;
use crate::ui;

/// The most desks the office can have.
const MOST_DESKS: u64 = studiod::api::MAX_RUNS_LIMIT;

/// An element's view, under a header with its name and a way back.
pub(super) fn element_view(cx: &Cx, element: Element, office: Office, extra: Extra) -> BoxView<'_> {
    let (title, icon) = match element {
        Element::Board => ("Whiteboard", "target"),
        Element::Corkboard => ("Team board", "users"),
        Element::Posters => ("House rules", "file-text"),
        Element::Racks => ("Checks", "flask-conical"),
        Element::AddDesk => ("Desks", "plus"),
        Element::Lounge => ("Lounge", "coffee"),
        Element::Coffee => ("Coffee machine", "coffee"),
        Element::Trophies => ("Trophies", "check"),
        Element::Review => ("Review", "eye"),
        Element::Crates => ("Shipped", "inbox"),
        Element::Returns => ("Returns", "refresh-cw"),
        Element::Counter => ("Front desk", "shield-check"),
        Element::Queue => ("Up next", "clock"),
        Element::Safe => ("Safe", "wallet"),
        Element::AddProject => ("Add a project", "plus"),
        Element::Hire => ("Hire someone", "users"),
    };
    let body: BoxView = match element {
        Element::Board => view! { cx => board_view(office: office) }.boxed(),
        Element::Corkboard => view! { cx => corkboard_view(office: office, extra: extra) }.boxed(),
        Element::Posters => view! { cx => posters_view(rules: extra.rules) }.boxed(),
        Element::Racks => view! { cx => racks_view(office: office, checks: extra.checks) }.boxed(),
        Element::AddDesk => view! { cx => desks_view(desks: office.status.max_runs) }.boxed(),
        Element::Lounge => view! { cx => lounge_view(office: office) }.boxed(),
        Element::Coffee => view! { cx => coffee_view(closed: office.status.closed) }.boxed(),
        Element::Trophies => {
            view! { cx => trophies_view(trophies: extra.trophies, now: office.now) }.boxed()
        }
        Element::Review => view! { cx => review_view(office: office) }.boxed(),
        Element::Crates => view! { cx => crates_view(office: office) }.boxed(),
        Element::Returns => view! { cx => returns_view(office: office) }.boxed(),
        Element::Counter => view! { cx => counter_view(office: office) }.boxed(),
        Element::Queue => view! { cx => queue_view(office: office) }.boxed(),
        Element::Safe => view! { cx => safe_view(office: office, extra: extra) }.boxed(),
        Element::AddProject => view! { cx => add_project_view(office: office) }.boxed(),
        Element::Hire => view! { cx => hire_view() }.boxed(),
    };
    view! { cx =>
        inspector::head(
            ui::icon(name: icon)
            <b class="font-medium">(title)</b>
        )
        <div class=(inspector::BODY)>(body)</div>
    }
    .boxed()
}

/// An open goal on the board: its key, name, tickets landed and filed, and
/// whether it is done.
type GoalEntry = (String, String, usize, usize, bool);

/// Open goals by project, each with how far it has come, and a way to pin a
/// new one.
#[component]
async fn board_view(office: Office) -> topcoat::Result<impl View> {
    let goals = floor::goals(&office);
    let groups: Vec<(String, usize, Vec<GoalEntry>)> = office
        .status
        .projects
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let own: Vec<_> = goals
                .iter()
                .filter(|g| g.plan.project.to_string() == *name)
                .map(|g| {
                    (
                        key("goals", &g.plan.id.to_string()),
                        g.name(),
                        g.landed(),
                        g.filed.len(),
                        g.state() == "done",
                    )
                })
                .collect();
            (!own.is_empty()).then(|| (name.clone(), facts::wrap(i), own))
        })
        .collect();
    Ok(view! {
        if groups.is_empty() {
            quiet(text: "No goals are open. Pin one for the studio to start on.")
        }
        for (name, index, goals) in groups {
            section(label: "Goals", note: name.clone(),
                <div class="flex flex-col gap-3">
                    for (pick, title, landed, total, done) in goals {
                        <button type="button" class="flex w-full cursor-pointer flex-col gap-1.5 border-0 bg-transparent p-0 text-left" data-on:click=(select(&pick))>
                            <span class="flex items-baseline gap-2">
                                <i class=(dot_class(index))></i>
                                <b class="min-w-0 flex-1 truncate text-[13px] font-semibold">(title)</b>
                                <span class="font-mono text-[11.5px] text-fg-2">(landed) " of " (total)</span>
                            </span>
                            ui::meter(value: landed as f64, max: total as f64, tone: if done { "ok" } else { "run" })
                        </button>
                    }
                </div>
            )
        }
        <div>
            <button type="button" class=(ui::PRIMARY) popovertarget="office-new-goal" data-on:click="$pick = ''">
                ui::icon(name: "target")
                "Pin a new goal"
            </button>
        </div>
    })
}

/// The team in brief, and a way to hire.
#[component]
async fn corkboard_view(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let [work, resting, front] = members(&office, &extra);
    let all: Vec<Member> = work.into_iter().chain(resting).chain(front).collect();
    Ok(view! {
        if all.is_empty() {
            quiet(text: "No one is hired yet.")
        } else {
            <div class="card">
                for member in all { member_brief(member: member) }
            </div>
        }
        hire_link()
    })
}

#[component]
async fn member_brief(member: Member) -> topcoat::Result<impl View> {
    let pick = select(&key("hires", &member.id));
    Ok(view! {
        <button type="button" class="member w-full border-0 bg-transparent text-left" data-on:click=(pick)>
            <span class=(format!("portrait {}", member.look))>
                if let Some((lv, _)) = member.level {
                    <span class="lv">"Lv " (lv)</span>
                }
            </span>
            <div class="who min-w-0">
                <b>(member.id.clone())</b>
                <span>(member.line.clone())</span>
            </div>
            <span></span>
        </button>
    })
}

#[component]
async fn posters_view(rules: Vec<RuleFile>) -> topcoat::Result<impl View> {
    Ok(view! {
        quiet(text: "What every agent follows.")
        rule_cards(rules: rules)
        new_rule()
    })
}

/// The form that adds a project, and the projects there are now.
#[component]
async fn add_project_view(office: Office) -> topcoat::Result<impl View> {
    let projects: Vec<(String, usize)> = office
        .status
        .projects
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), facts::wrap(i)))
        .collect();
    let note = match projects.len() {
        0 => String::new(),
        n => n.to_string(),
    };
    Ok(view! {
        // Coming back to this panel starts from the form, not from a check
        // made earlier.
        <span hidden="" data-init="$_found.path = ''"></span>
        quiet(text: "Pick the folder of a git repository on this computer. It gets a rack of its own in the checks lab.")
        // The first submit only checks. What studiod says would be added
        // fills `_found`, which swaps the form for the card below; the card's
        // button then posts the same form again to add it.
        <form id="add-project" class="flex flex-col gap-3" data-show="!$_found.path" data-on:submit="@post('/office/projects', {contentType: 'form'})">
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"Folder"</span>
                <input name="path" required="" autocomplete="off" spellcheck="false" placeholder="/path/to/your/repository" class=(ui::INPUT)>
            </label>
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"Name"</span>
                <input name="name" autocomplete="off" spellcheck="false" placeholder="The folder's name" class=(ui::INPUT)>
            </label>
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"Branch work starts from"</span>
                <input name="branch" autocomplete="off" spellcheck="false" placeholder="The branch checked out there" class=(ui::INPUT)>
            </label>
            <div class=(inspector::ACTIONS)>
                <button type="submit" class=(ui::PRIMARY)>
                    ui::icon(name: "plus")
                    "Add project"
                </button>
            </div>
        </form>
        <div id="add-project-found" class="flex flex-col gap-3" style="display: none" data-show="!!$_found.path" aria-live="polite">
            <p class="m-0 text-[13px] leading-5 text-fg-2">
                "This adds " <b class="font-medium text-fg" data-text="$_found.name"></b> " to the studio."
            </p>
            <dl class="m-0 flex flex-col gap-2 text-[13px]">
                <div>
                    <dt class="text-[12px] text-fg-3">"Folder"</dt>
                    <dd class="m-0 break-all font-mono text-[12.5px]" data-text="$_found.path"></dd>
                </div>
                <div>
                    <dt class="text-[12px] text-fg-3">"Work starts from the branch"</dt>
                    <dd class="m-0 font-mono text-[12.5px]" data-text="$_found.branch"></dd>
                </div>
                <div class="text-fg-2" data-text="$_found.checks"></div>
            </dl>
            <div class=(inspector::ACTIONS)>
                <button type="button" class=(ui::PRIMARY) data-on:click="@post('/office/projects?add=yes', {contentType: 'form', selector: '#add-project'}); $_found.path = ''">
                    ui::icon(name: "plus")
                    "Add it"
                </button>
                <button type="button" class=(ui::BUTTON) data-on:click="$_found.path = ''">"Cancel"</button>
            </div>
        </div>
        section(label: "Projects now", note: note,
            if projects.is_empty() {
                quiet(text: "No projects yet.")
            }
            <div class="flex flex-col gap-1.5">
                for (name, index) in projects {
                    <div class="flex items-center gap-2 text-[13px]"><i class=(dot_class(index))></i><b class="font-medium">(name)</b></div>
                }
            </div>
        )
    })
}

/// The form that hires someone, and a way to the hires editor for more.
#[component]
async fn hire_view() -> topcoat::Result<impl View> {
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

/// A project's checks: what runs, what runs now and how the last ended.
#[component]
async fn racks_view(office: Office, checks: Vec<ProjectCheck>) -> topcoat::Result<impl View> {
    let now = office.now;
    let none = office.status.projects.is_empty();
    let rows: Vec<(ProjectCheck, usize)> = checks
        .into_iter()
        .map(|c| {
            let index = tint(&office, &c.project);
            (c, index)
        })
        .collect();
    Ok(view! {
        if none {
            quiet(text: "No projects yet. Each project you add gets a rack here for its checks.")
            <div class=(inspector::ACTIONS)>
                <button type="button" class=(ui::PRIMARY) data-on:click=(select("elements/add-project"))>
                    ui::icon(name: "plus")
                    "Add a project"
                </button>
            </div>
        } else if rows.is_empty() {
            quiet(text: "No project checks to show.")
        }
        for (check, index) in rows {
            <div class="card">
                <div class="flex items-center gap-2"><i class=(dot_class(index))></i><b class="text-[13px] font-semibold">(check.project.clone())</b></div>
                if check.verify.is_empty() {
                    <span class="text-[12.5px] text-fg-3">"No checks are set for this project."</span>
                }
                for command in check.verify.clone() {
                    <div class="check-row">
                        <span class="text-fg-3">ui::icon(name: "chevron-right", small: true)</span>
                        <code>(command.join(" "))</code>
                        <span></span>
                    </div>
                }
                if !check.running.is_empty() {
                    <div class="flex flex-col gap-0.5">
                        <span class="text-[12px] text-fg-3">"Running checks now"</span>
                        for id in check.running.clone() {
                            line(title: office.ticket(&id).map(|t| t.title.clone()).unwrap_or_default(), tint: index, id: id)
                        }
                    </div>
                }
                if let Some(last) = check.latest.clone() {
                    <div class="check-row">
                        ui::state_icon(state: if last.passed { "landed" } else { "failed" }, small: true)
                        <span>
                            (if last.passed { "Passed" } else { "Failed" }) " on "
                            <a href=(format!("/tickets/{}", last.ticket)) class=(inspector::TICKET_LINK)>(last.ticket.clone())</a>
                        </span>
                        <span>(relative(now, last.at_ms))</span>
                    </div>
                }
            </div>
        }
    })
}

/// How many desks there are, and a way to add or remove one.
#[component]
async fn desks_view(desks: u64) -> topcoat::Result<impl View> {
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
async fn coffee_view(closed: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex items-center gap-2">
            ui::dot(state: if closed { "failed" } else { "ready" })
            <b class="text-[14px] font-semibold">(if closed { "The office is closed" } else { "The office is open" })</b>
        </div>
        quiet(text: "Closing the office stops new work from starting. Work already running carries on, and queued tickets wait until you reopen it.")
        <div class=(inspector::ACTIONS)>
            if closed {
                <button type="button" class=(ui::PRIMARY) data-on:click="@post('/office/open')">"Reopen the office"</button>
            } else {
                <button type="button" class=(ui::BUTTON) data-on:click="@post('/office/close')">"Close the office"</button>
            }
        </div>
    })
}

/// What was spent against the limit, and the limits.
#[component]
async fn safe_view(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let (spent, daily) = (office.status.spent_24h, office.status.daily_usd);
    let held = extra.budget.as_ref().map(Budget::held);
    let ticket_usd = extra.budget.as_ref().and_then(|b| b.limits.ticket_usd);
    Ok(view! {
        section(label: "Last 24 hours", note: format!("daily limit {}", ui::limit(daily)),
            spend_summary(spent: spent, daily: daily, held: held)
        )
        section(label: "Limits",
            limits_form(daily: daily, ticket: ticket_usd)
        )
    })
}

/// Trophies won, and the next one with how far along it is.
#[component]
async fn trophies_view(trophies: Vec<Trophy>, now: u64) -> topcoat::Result<impl View> {
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
            section(label: "Earned",
                <div class="flex flex-col gap-3">
                    for (trophy, when) in won {
                        <div class="ach">
                            <span class="medal">"★"</span>
                            <div><b>(trophy.title.clone())</b><span>(trophy.detail.clone())</span></div>
                            <time>(when)</time>
                        </div>
                    }
                </div>
            )
        }
        if let Some(trophy) = next {
            section(label: "Next",
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
            )
        }
    })
}

#[cfg(test)]
mod tests {

    use crate::pages::office::panel::fixtures::{extra, html_of, office};

    #[tokio::test]
    async fn desks_can_be_added_and_removed() {
        let html = html_of(office(), extra(), "elements/add-desk").await;
        assert!(html.contains("Up to 3 agents work at once"), "{html}");
        assert!(html.contains("@post('/office/desks'"), "{html}");
        assert!(
            html.contains("Add a desk") && html.contains("Remove a desk"),
            "{html}"
        );
        assert!(html.contains("name=\"max_runs\" value=\"4\""), "{html}");
        assert!(html.contains("name=\"max_runs\" value=\"2\""), "{html}");
        assert!(html.contains("$sel = ''; @get('/office/panel')"), "{html}");
        // At the ends, only the other way is offered.
        let mut one = office();
        one.status.max_runs = 1;
        assert!(
            !html_of(one, extra(), "elements/add-desk")
                .await
                .contains("Remove a desk")
        );
        let mut most = office();
        most.status.max_runs = 16;
        assert!(
            !html_of(most, extra(), "elements/add-desk")
                .await
                .contains("Add a desk")
        );
    }

    #[tokio::test]
    async fn the_coffee_machine_closes_and_reopens() {
        let open = html_of(office(), extra(), "elements/coffee").await;
        assert!(open.contains("The office is open"), "{open}");
        assert!(open.contains("Close the office"), "{open}");
        assert!(open.contains("@post('/office/close')"), "{open}");
        assert!(!open.contains("/office/open"), "{open}");
        let mut shut = office();
        shut.status.closed = true;
        let closed = html_of(shut, extra(), "elements/coffee").await;
        assert!(closed.contains("The office is closed"), "{closed}");
        assert!(closed.contains("Reopen the office"), "{closed}");
        assert!(closed.contains("@post('/office/open')"), "{closed}");
        assert!(
            closed.contains("Work already running carries on"),
            "{closed}"
        );
    }

    #[tokio::test]
    async fn the_safe_shows_the_budget() {
        let html = html_of(office(), extra(), "elements/safe").await;
        assert!(html.contains("$6.40"), "{html}");
        assert!(html.contains("$0.72 held by live runs"), "{html}");
        assert!(html.contains("$2.88 left"), "{html}");
        assert!(html.contains("@post('/office/limits'"), "{html}");
    }

    #[tokio::test]
    async fn the_board_pins_goals() {
        let html = html_of(office(), extra(), "elements/board").await;
        assert!(html.contains("Checkout flow"), "{html}");
        assert!(html.contains("1 of 3"), "{html}");
        assert!(html.contains("Pin a new goal"), "{html}");
        assert!(html.contains("popovertarget=\"office-new-goal\""), "{html}");
        assert!(html.contains("$sel = 'goals/SHOP-1'"), "{html}");
    }

    #[tokio::test]
    async fn the_racks_show_commands_runs_and_the_last_result() {
        let html = html_of(office(), extra(), "elements/racks").await;
        assert!(html.contains("cargo test"), "{html}");
        assert!(html.contains("Running checks now"), "{html}");
        assert!(html.contains(">SHOP-3<"), "{html}");
        assert!(html.contains("Passed"), "{html}");
        assert!(html.contains("SHOP-2"), "{html}");
        assert!(html.contains("5m ago"), "{html}");
    }

    #[tokio::test]
    async fn trophies_show_what_was_won_and_the_next() {
        let html = html_of(office(), extra(), "elements/trophies").await;
        assert!(html.contains("First goal merged"), "{html}");
        assert!(html.contains("3h ago"), "{html}");
        assert!(html.contains("25 tickets landed"), "{html}");
        assert!(html.contains("12 of 25"), "{html}");
    }

    #[tokio::test]
    async fn the_corkboard_and_posters_have_their_links() {
        let board = html_of(office(), extra(), "elements/corkboard").await;
        assert!(board.contains("Hire someone"), "{board}");
        assert!(board.contains("href=\"/hires?new\""), "{board}");
        assert!(board.contains("Engineer"), "{board}");
        let posters = html_of(office(), extra(), "elements/posters").await;
        assert!(posters.contains("no-push"), "{posters}");
        assert!(posters.contains("New rule"), "{posters}");
    }
}
