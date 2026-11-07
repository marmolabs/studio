//! HQ: the owner's morning round-up. A line from each lead, with the cards
//! that wait for the owner under it: questions, work ready to ship and
//! releases that wait for a go-ahead. Leads with something for the owner come
//! first.

use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{
    AskView, CheckState, ManagerAction, OperationState, ReleaseState, ReleaseView, ShipCheck,
};
use topcoat::view::{Child, View, component, view};

use super::shell::{self, Here};
use super::talk::asked;
use super::talk::thread::{Person, Sizes};
use super::{
    LeadEntry, Team, clock, faces, goal_result, lead_href, person_href, task_result, version,
};
use crate::pages::ask_card::DECISION;
use crate::pages::office::floor;
use crate::pages::office::{Office, facts};
use crate::ui;

// ------------------------------------------------------------ what waits

/// Work that is ready to ship: a goal whose tasks are all done, or one task
/// that passed review.
#[derive(Clone, Debug, PartialEq)]
pub struct Ship {
    /// What "Look closer" opens: the goal's plan ticket or the task.
    pub look: String,
    pub title: String,
    /// What the Ship button posts.
    pub post: String,
    /// Lines added and removed, when the studio knows them.
    pub size: Option<(u64, u64)>,
    /// What the goal's checks came to, once the studio has run them.
    pub checks: Option<ShipCheck>,
    /// A reviewer on another model read it.
    pub reviewed: bool,
    /// What the work came to, in a line; empty when its agents said none.
    pub result: String,
}

/// Everything that waits for the owner in one project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waiting {
    pub asks: Vec<AskView>,
    /// Tasks held for the owner.
    pub held: Vec<Ticket>,
    pub ships: Vec<Ship>,
    /// Actions proposed by the project's manager.
    pub actions: Vec<ManagerAction>,
    pub reviewing: bool,
    /// A release that waits for the go-ahead.
    pub release: Option<ReleaseView>,
}

impl Waiting {
    /// How many things wait.
    pub fn count(&self) -> usize {
        self.asks.len()
            + self.held.len()
            + self.ships.len()
            + self.actions.len()
            + usize::from(self.release.is_some())
    }
}

/// What waits for the owner in `project`. `sizes` are the lines added and
/// removed by finished goals (by their plan tickets), which the office does
/// not give.
pub fn waiting(office: &Office, project: &str, sizes: &Sizes) -> Waiting {
    let in_project = |ticket: &Ticket| ticket.project.to_string() == project;
    Waiting {
        asks: office
            .asks
            .iter()
            .filter(|ask| office.ticket(&ask.ticket).is_some_and(in_project))
            .cloned()
            .collect(),
        held: facts::needs_you(office)
            .held
            .into_iter()
            .filter(|ticket| in_project(ticket))
            .cloned()
            .collect(),
        ships: ships(office, project, sizes),
        actions: office
            .operations
            .actions
            .iter()
            .filter(|action| action.project == project && action.state == OperationState::Pending)
            .cloned()
            .collect(),
        reviewing: office
            .operations
            .reviewing
            .iter()
            .any(|reviewing| reviewing == project),
        release: office
            .project(project)
            .and_then(|view| view.release.clone())
            .filter(|release| release.state == ReleaseState::Waiting),
    }
}

/// The work of `project` that is ready to ship: its finished goals, then its
/// tasks that passed review. A goal's size is what its branch adds to the base
/// it ships to, told only when studiod could tell it.
pub fn ships(office: &Office, project: &str, sizes: &Sizes) -> Vec<Ship> {
    let mut ships = Vec::new();
    for goal in floor::goals(office)
        .into_iter()
        .filter(|goal| goal.plan.project.to_string() == project && goal.state() == "done")
    {
        let Some(landed) = goal.filed.iter().find(|t| t.state.kind() == "landed") else {
            continue;
        };
        let plan = goal.plan.id.to_string();
        ships.push(Ship {
            title: goal.name(),
            post: format!("@post('/tickets/{}/merge')", landed.id),
            size: office
                .sizes
                .get(&plan)
                .or_else(|| sizes.get(&plan))
                .copied()
                .filter(|(added, removed)| added + removed > 0),
            checks: office.ship_checks.get(&plan).cloned(),
            reviewed: false,
            result: goal_result(office, &goal),
            look: plan,
        });
    }
    for ticket in office.tickets.iter().filter(|t| {
        t.project.to_string() == project && t.kind != TicketKind::Plan && t.state.kind() == "ready"
    }) {
        let id = ticket.id.to_string();
        ships.push(Ship {
            title: ticket.title.clone(),
            post: format!("@post('/tickets/{id}/land')"),
            size: office
                .sizes
                .get(&id)
                .copied()
                .filter(|(added, removed)| added + removed > 0),
            checks: None,
            reviewed: !office.unreviewed.contains(&id),
            result: task_result(office, &id),
            look: id,
        });
    }
    ships
}

// ----------------------------------------------------------- the lines

/// One lead's line on HQ.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub entry: LeadEntry,
    /// The lead's few words.
    pub text: String,
    /// When they were written, as the owner's clock shows it.
    pub at: Option<String>,
    pub waiting: Waiting,
}

/// A line for each lead, those with something for the owner first, each group
/// in the studio's order.
pub fn lines(team: &Team, office: &Office, sizes: &Sizes) -> Vec<Line> {
    let mut lines: Vec<Line> = team
        .leads
        .iter()
        .map(|entry| {
            let waiting = waiting(office, &entry.project, sizes);
            let said = office.standups.get(&entry.project);
            Line {
                text: said
                    .map(|standup| standup.text.trim().to_string())
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| fallback(office, entry, &waiting)),
                at: said.map(|standup| clock::said(standup.at, office.now)),
                waiting,
                entry: entry.clone(),
            }
        })
        .collect();
    // Stable: the studio's order stays within each group.
    lines.sort_by_key(|line| line.waiting.count() == 0);
    lines
}

/// What to say of a project when its lead has not written a standup, or it
/// has no lead: the day's facts in a few plain words.
fn fallback(office: &Office, entry: &LeadEntry, waiting: &Waiting) -> String {
    let in_project = |id: &str| {
        office
            .ticket(id)
            .is_some_and(|ticket| ticket.project.to_string() == entry.project)
    };
    let done = facts::landed(office)
        .iter()
        .filter(|(ticket, _)| ticket.project.to_string() == entry.project)
        .count();
    let back = office
        .recent
        .iter()
        .filter(|happened| {
            matches!(
                happened.event,
                TicketEvent::ChecksFailed { .. } | TicketEvent::ReviewFailed { .. }
            ) && in_project(&happened.ticket_id)
        })
        .count();
    let mut said = if done + back > 0 {
        let tasks = if done == 1 { "task" } else { "tasks" };
        format!("{done} {tasks} done, {back} sent back.")
    } else if waiting.count() == 0 {
        // With something waiting, that is the news, not the phase.
        format!("{}.", entry.phase_words.trim_end_matches('…'))
    } else {
        String::new()
    };
    let waits = match waiting.count() {
        0 => String::new(),
        1 => "1 waits for you.".to_string(),
        n => format!("{n} wait for you."),
    };
    if !said.is_empty() && !waits.is_empty() {
        said.push(' ');
    }
    said.push_str(&waits);
    said
}

/// The last day in a line: what was done, sent back and spent.
fn totals(office: &Office) -> String {
    let done = facts::landed(office).len();
    let back = office
        .recent
        .iter()
        .filter(|happened| {
            matches!(
                happened.event,
                TicketEvent::ChecksFailed { .. } | TicketEvent::ReviewFailed { .. }
            )
        })
        .count();
    format!(
        "The last 24 hours: {done} {} done, {back} sent back, {} of {} spent",
        if done == 1 { "task" } else { "tasks" },
        ui::usd(office.status.spent_24h),
        ui::limit(office.status.daily_usd),
    )
}

// ------------------------------------------------------ company activity

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub person: Person,
    pub project: String,
    pub tint: usize,
    pub task: String,
    pub title: String,
    pub action: String,
    pub at: u64,
    pub tone: &'static str,
}

/// The company's latest events, newest first. These are the same stored ticket
/// events that drive the rest of HQ, not a separate activity record.
pub fn activity(office: &Office) -> Vec<Activity> {
    let projects = office.project_names();
    let mut events: Vec<_> = office.recent.iter().collect();
    events.sort_by_key(|happened| happened.at_ms);
    events
        .into_iter()
        .rev()
        .filter_map(|happened| {
            let ticket = office.ticket(&happened.ticket_id)?;
            let (action, tone) = action(&happened.event, ticket);
            let project = ticket.project.to_string();
            Some(Activity {
                person: Person::of(office, &ticket.hire.to_string()),
                tint: projects
                    .iter()
                    .position(|name| name == &project)
                    .unwrap_or(0),
                task: ticket.id.to_string(),
                title: ticket.title.clone(),
                action: action.to_string(),
                at: happened.at_ms,
                tone,
                project,
            })
        })
        .take(20)
        .collect()
}

fn action(event: &TicketEvent, ticket: &Ticket) -> (&'static str, &'static str) {
    match event {
        TicketEvent::Claimed => ("started working", "run"),
        TicketEvent::AskOpened { .. } => ("needs an answer", "attn"),
        TicketEvent::AskResolved { approved: true, .. } => ("was allowed to continue", "run"),
        TicketEvent::AskResolved {
            approved: false, ..
        } => ("had a request declined", "attn"),
        TicketEvent::ChecksStarted => ("started checks", "run"),
        TicketEvent::ChecksFailed { .. } => ("was sent back after checks failed", "bad"),
        TicketEvent::ReviewFailed { .. } => ("was sent back after review", "bad"),
        TicketEvent::Held { .. } => ("is on hold", "attn"),
        TicketEvent::Landed if ticket.kind == TicketKind::Plan => ("filed the plan", "ok"),
        TicketEvent::Landed => ("landed the work", "ok"),
        TicketEvent::Failed { .. } => ("failed", "bad"),
        TicketEvent::Cancelled => ("was cancelled", "bad"),
        _ => ("updated the task", "run"),
    }
}

#[component]
async fn company_activity(items: Vec<Activity>, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <section id="company-activity" class="flex flex-col gap-3 border-t border-line pt-5" aria-label="Company activity">
            <div class="flex items-baseline justify-between gap-3">
                <h2 class="text-[16px] font-semibold">"Company activity"</h2>
                <span class="text-[12.5px] text-fg-3">"Last 24 hours"</span>
            </div>
            <ol class="flex flex-col">
                for item in items {
                    <li class="grid grid-cols-[30px_minmax(0,1fr)_auto] items-start gap-3 border-t border-line py-3 first:border-t-0">
                        <a href=(person_href(&item.person.hire)) tabindex="-1" aria-hidden="true">
                            faces::face(seed: item.person.hire.clone(), role: item.person.role, tint: item.tint, size: 30)
                        </a>
                        <div class="min-w-0 text-[13.5px] leading-5">
                            <span>
                                <a href=(person_href(&item.person.hire)) class="font-semibold hover:underline">(item.person.name)</a>
                                " " (item.action) " on "
                                <a href=(format!("/tickets/{}", item.task)) class="font-medium hover:underline">(item.title)</a>
                            </span>
                            <span class="block text-[12.5px] text-fg-3">(item.project)</span>
                        </div>
                        <span class=(format!("pt-0.5 text-[12px] text-fg-3 activity-{}", item.tone))>(clock::said(item.at, now))</span>
                    </li>
                }
            </ol>
        </section>
    })
}

// ------------------------------------------------------------ the page

/// The whole page: the frame around the round-up.
#[component]
pub async fn hq(team: Team, office: Office, sizes: Sizes) -> topcoat::Result<impl View> {
    let local = clock::Local::at(office.now, clock::offset());
    let (date, greeting) = (local.date(), local.greeting());
    let needs = team.needs;
    let lines = lines(&team, &office, &sizes);
    let activity = activity(&office);
    let footer = totals(&office);
    let empty = team.leads.is_empty();
    Ok(view! {
        shell::page(team: team, here: Here::Hq,
            <div class="mx-auto flex max-w-[760px] flex-col gap-[30px]">
                <div class="flex flex-wrap items-end justify-between gap-2">
                    <div class="flex flex-col gap-1">
                        <span class="text-[12.5px] text-fg-3">(date)</span>
                        <h1 class="text-[30px] font-semibold leading-[1.2] tracking-[-0.01em] max-[760px]:text-[24px]">(greeting)</h1>
                    </div>
                    if needs > 0 {
                        <span class="inline-flex h-[22px] items-center gap-1.5 whitespace-nowrap rounded-full border border-attn-line bg-attn-bg px-[9px] text-[12px] font-semibold text-attn-fg">
                            <i class="size-[7px] rounded-full bg-current"></i>
                            (needs) " need you"
                        </span>
                    } else {
                        <span class="inline-flex h-[22px] items-center gap-1.5 whitespace-nowrap rounded-full border border-line-2 bg-bg-3 px-[9px] text-[12px] font-semibold text-fg-2">
                            "Nothing waits for you"
                        </span>
                    }
                </div>
                if empty {
                    <p class="text-[15px] text-fg-2">
                        "No project yet. "
                        <a href="/new" class="text-fg underline decoration-line-3 underline-offset-3 hover:decoration-fg">"Hire a lead for one."</a>
                    </p>
                }
                for line in lines {
                    lead_line(line: line)
                }
                if !activity.is_empty() {
                    company_activity(items: activity, now: office.now)
                }
                if !empty {
                    <p class="border-t border-line pt-3.5 text-center text-[12.5px] tabular-nums text-fg-3">(footer)</p>
                }
            </div>
        )
    })
}

/// A lead's face, name and few words, and the cards that wait in its project.
#[component]
async fn lead_line(line: Line) -> topcoat::Result<impl View> {
    let Line {
        entry,
        text,
        at,
        waiting,
    } = line;
    let Waiting {
        asks,
        held,
        ships,
        actions,
        reviewing,
        release,
    } = waiting;
    let href = lead_href(&entry.project);
    let from = match at {
        Some(at) => format!("{} · {at}", entry.project),
        None => entry.project.clone(),
    };
    let led = entry.lead.is_some();
    let seed = entry.lead.clone().unwrap_or_else(|| entry.project.clone());
    // A project without a lead has a row of its own that says so, which is
    // something to do for the owner, though not something that waits.
    let cards = !asks.is_empty()
        || !held.is_empty()
        || !ships.is_empty()
        || !actions.is_empty()
        || release.is_some()
        || !led;
    let project = entry.project.clone();
    Ok(view! {
        <section class="grid grid-cols-[40px_minmax(0,1fr)] gap-x-3" aria-label=(if led { format!("{} leads {}", entry.name, entry.project) } else { format!("{} has no lead", entry.project) }) data-lead=(entry.project.clone())>
            <a href=(href.clone()) tabindex="-1" aria-hidden="true">faces::face(seed: seed, role: entry.role, tint: entry.tint, size: 40)</a>
            <div class=(format!("flex min-w-0 flex-col {}", if cards { "gap-2.5" } else { "gap-1.5" }))>
                <a href=(href.clone()) class="flex items-baseline gap-2">
                    <b class="text-[14.5px] font-semibold">(entry.name.clone())</b>
                    // Without a lead the name is the project's: it is not said twice.
                    if led { <span class="text-[12.5px] text-fg-3">(from)</span> }
                </a>
                <p class="text-[15px] leading-[1.5] [overflow-wrap:anywhere]">ui::prose(text: text)</p>
                if led {
                    <div>super::operations::review_button(project: project.clone(), reviewing: reviewing)</div>
                }
                if !led {
                    no_lead_row(project: project.clone())
                }
                for ask in asks {
                    ask_row(ask: ask)
                }
                for ticket in held {
                    held_row(ticket: ticket)
                }
                for ship in ships {
                    ship_row(ship: ship, project: project.clone())
                }
                for action in actions {
                    super::operations::action_row(action: action, by: entry.name.clone())
                }
                if let Some(release) = release {
                    release_row(release: release, project: project.clone())
                }
            </div>
        </section>
    })
}

/// A card that waits for the owner: a tile in the colour of what it is, the
/// title, a line under it, and the buttons in `child`.
#[component]
pub(super) async fn card(
    tone: &'static str,
    #[into] title: String,
    #[into] meta: Child<'_>,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    let (frame, tile) = match tone {
        "ship" => ("border-ok-line bg-bg-3", "bg-ok text-[#08130b]"),
        "release" => ("border-attn-line bg-attn-bg", "bg-[#f08a3c] text-[#200d00]"),
        // Nothing waits: a project to give a lead.
        "lead" => ("border-line-2 bg-bg-2", "bg-run-bg text-run-fg"),
        _ => ("border-attn-line bg-attn-bg", "bg-attn text-[#1d1300]"),
    };
    Ok(view! {
        <div class=(format!("flex flex-wrap items-center gap-x-3.5 gap-y-2.5 rounded-[14px] border px-3.5 py-3 {frame}")) data-card=(tone)>
            <div class="flex min-w-0 flex-[1_1_260px] items-center gap-2.5">
                <span class=(format!("grid size-[30px] shrink-0 place-items-center rounded-[9px] {tile}"))>
                    if tone == "ship" {
                        ui::icon(name: "git-merge")
                    } else if tone == "lead" {
                        ui::icon(name: "users")
                    } else if tone == "release" {
                        <svg class="size-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"></path><path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"></path><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"></path><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"></path></svg>
                    } else {
                        ui::icon(name: "circle-alert")
                    }
                </span>
                <span class="flex min-w-0 flex-col">
                    <b class="text-[15px] font-semibold [overflow-wrap:anywhere]">(title)</b>
                    <span class="text-[12.5px] text-fg-3 [overflow-wrap:anywhere]">(meta)</span>
                </span>
            </div>
            <div class="flex flex-wrap gap-2">(child)</div>
        </div>
    })
}

/// A project nobody leads: it says so, and a link leads to the project's
/// conversation, where it is given a lead.
#[component]
async fn no_lead_row(project: String) -> topcoat::Result<impl View> {
    Ok(view! {
        card(tone: "lead", title: "No lead yet", meta: view! { "A lead plans the work and answers the team's questions." },
            <a href=(lead_href(&project)) class=(ui::BUTTON_SM)>"Give " (project.clone()) " a lead"</a>
        )
    })
}

/// A question for the owner: the lead's own words for it, else the card's
/// label, and why it came up. The buttons answer it where it is a call to
/// allow or deny; a question the agent asks is answered on its task's page.
#[component]
async fn ask_row(ask: AskView) -> topcoat::Result<impl View> {
    let title = asked::title(&ask);
    let why = asked::why(&ask);
    let answer = ask.kind == "question";
    let submit = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    let task = format!("/tickets/{}", ask.ticket);
    Ok(view! {
        card(tone: "ask", title: title, meta: view! { (why) },
            if answer {
                <a href=(task) class=(ui::PRIMARY_SM)>"Answer"</a>
            } else {
                <form class="contents" data-on:submit=(submit)>
                    <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY_SM)>"Allow"</button>
                    <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON_SM)>"Deny"</button>
                </form>
                <a href=(task) class=(ui::BUTTON_SM)>"Details"</a>
            }
        )
    })
}

/// A task held for the owner, with why.
#[component]
async fn held_row(ticket: Ticket) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let why = ui::state_detail(&ticket.state).unwrap_or_default();
    let resume = format!("@post('/tickets/{id}/resume')");
    Ok(view! {
        card(tone: "ask", title: ticket.title.clone(), meta: view! { "Held: " (why) },
            <button type="button" class=(ui::PRIMARY_SM) data-on:click=(resume)>"Resume"</button>
            <a href=(format!("/tickets/{id}")) class=(ui::BUTTON_SM)>"Details"</a>
        )
    })
}

/// Work that is ready to ship: its size, how its checks went, Ship, and a
/// closer look.
#[component]
async fn ship_row(ship: Ship, project: String) -> topcoat::Result<impl View> {
    let Ship {
        look,
        title,
        post,
        size,
        checks,
        reviewed,
        result,
    } = ship;
    let failed = checks
        .as_ref()
        .is_some_and(|check| check.state == CheckState::Failed);
    let checked = checks.map(|check| match check.state {
        CheckState::Passed => "checks passed".to_string(),
        CheckState::Failed if check.summary.trim().is_empty() => "checks failed".to_string(),
        CheckState::Failed => format!("checks failed: {}", check.summary.trim()),
        CheckState::Running => "checks running".to_string(),
    });
    // The checks of a goal that failed are said first.
    let ship = if failed {
        format!("confirm('The checks failed. Ship it anyway?') && {post}")
    } else {
        post
    };
    let look_at = format!("{}?look={look}", lead_href(&project));
    Ok(view! {
        card(tone: "ship", title: format!("{title} is ready to ship"),
            meta: view! {
                if !result.is_empty() {
                    <span class="block truncate text-fg-2" title=(result.clone())>(result.clone())</span>
                }
                <span class="tabular-nums">
                    if let Some((added, removed)) = size {
                        <span class=(ui::ADD)>"+" (added)</span> " " <span class=(ui::DEL)>"−" (removed)</span>
                    }
                </span>
                if let Some(checked) = checked.clone() {
                    if size.is_some() { " · " }
                    <span class=(if failed { ui::DEL } else { "" })>(checked)</span>
                }
                if reviewed { if size.is_some() || checked.is_some() { " · " } "reviewed" }
            },
            <button type="button" class=(ui::PRIMARY_SM) data-on:click=(ship)>"Ship"</button>
            <a href=(look_at) class=(ui::BUTTON_SM)>"Look closer"</a>
        )
    })
}

/// A release that waits for the owner's go-ahead.
#[component]
async fn release_row(release: ReleaseView, project: String) -> topcoat::Result<impl View> {
    let title = format!("Publish {project} {}?", version(&release.version));
    let approve = "@post('/office/releases/approve', {contentType: 'form'})";
    let cancel = "@post('/office/releases/cancel', {contentType: 'form'})";
    Ok(view! {
        card(tone: "release", title: title, meta: view! { "The final checks passed. Nothing goes out until you say so." },
            <form class="contents" data-on:submit=(approve)>
                <input type="hidden" name="project" value=(project.clone())>
                <button type="submit" class=(ui::PRIMARY_SM)>"Publish"</button>
            </form>
            <form class="contents" data-on:submit=(cancel)>
                <input type="hidden" name="project" value=(project)>
                <button type="submit" class=(ui::BUTTON_SM)>"Not yet"</button>
            </form>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ticket(id: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({
            "id": id, "project": "shop", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            fields.extend(extra.clone());
        }
        serde_json::from_value(fields).unwrap()
    }

    /// A shop whose goal, "Save carts", has two tasks done, and one more task
    /// that is ready.
    fn office() -> Office {
        let mut office = Office {
            tickets: vec![
                ticket(
                    "S-1",
                    json!("landed"),
                    json!({"kind": "plan", "goal": "Save carts"}),
                ),
                ticket("S-2", json!("landed"), json!({"parent": "S-1"})),
                ticket("S-3", json!("landed"), json!({"parent": "S-1"})),
                ticket("S-4", json!("ready"), json!({})),
            ],
            now: 10_000_000,
            ..Office::default()
        };
        office.sizes.insert("S-4".into(), (12, 3));
        office
    }

    fn sizes(known: &[(&str, (u64, u64))]) -> Sizes {
        known
            .iter()
            .map(|(id, lines)| (id.to_string(), *lines))
            .collect()
    }

    #[test]
    fn a_finished_goal_has_the_size_of_its_branch_when_studiod_could_tell_it() {
        let office = office();
        // The goal is asked for by its plan ticket, not by its tasks.
        let known = sizes(&[("S-1", (79, 8))]);
        let ready = ships(&office, "shop", &known);
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].look, "S-1");
        assert_eq!(ready[0].size, Some((79, 8)));
        // The task that is ready has the size the office gives.
        assert_eq!(ready[1].size, Some((12, 3)));
        // A size of a task is not the goal's.
        let task = sizes(&[("S-2", (30, 2)), ("S-3", (49, 6))]);
        assert_eq!(ships(&office, "shop", &task)[0].size, None);
        // A goal that adds nothing says no size, and so does one not told.
        let none = sizes(&[("S-1", (0, 0))]);
        assert_eq!(ships(&office, "shop", &none)[0].size, None);
        assert_eq!(ships(&office, "shop", &Sizes::new())[0].size, None);
    }
}
