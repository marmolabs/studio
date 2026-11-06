use std::collections::{BTreeMap, BTreeSet};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{ManagerAction, OperationState};
use topcoat::view::{View, component, view};

use super::shell::{self, Here};
use super::talk::thread::Person;
use super::{LeadEntry, Team, clock, faces, lead_href, operations, person_href};
use crate::pages::office::{Office, floor};
use crate::ui;

#[derive(Clone, Debug, PartialEq)]
struct Work {
    id: String,
    title: String,
    state: String,
    icon: &'static str,
    next: String,
    person: Person,
    cost: f64,
    href: String,
}

#[derive(Clone, Debug, PartialEq)]
struct Goal {
    work: Work,
    tasks: Vec<Work>,
}

#[derive(Clone, Debug, PartialEq)]
struct Project {
    entry: LeadEntry,
    goals: Vec<Goal>,
    loose: Vec<Work>,
    reviewing: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Scorecard {
    delivered: usize,
    median_cycle_ms: Option<u64>,
    rework: usize,
    rework_percent: usize,
    cost_per_deliverable: Option<f64>,
    spent: f64,
    blocked: usize,
    blocked_ms: u64,
    projects: Vec<ProjectScore>,
    workload: Vec<Workload>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ProjectScore {
    name: String,
    delivered: usize,
    open: usize,
    rework: usize,
    spend: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct Workload {
    person: Person,
    role: String,
    active: usize,
    queued: usize,
    delivered: usize,
    spend: f64,
}

fn scorecard(office: &Office) -> Scorecard {
    let work_tickets = office
        .tickets
        .iter()
        .filter(|ticket| ticket.kind == TicketKind::Work)
        .collect::<Vec<_>>();
    let mut delivered_ids = BTreeSet::new();
    let mut rework = 0;
    let mut by_project: BTreeMap<String, ProjectScore> = office
        .project_names()
        .into_iter()
        .map(|name| {
            (
                name.clone(),
                ProjectScore {
                    name,
                    ..ProjectScore::default()
                },
            )
        })
        .collect();
    let mut cycles = Vec::new();
    let mut delivered_by_hire: BTreeMap<String, usize> = BTreeMap::new();
    for happened in &office.recent {
        let Some(ticket) = office.ticket(&happened.ticket_id) else {
            continue;
        };
        if ticket.kind != TicketKind::Work {
            continue;
        }
        let project = ticket.project.to_string();
        match happened.event {
            TicketEvent::Landed => {
                if delivered_ids.insert(happened.ticket_id.clone()) {
                    cycles.push(happened.at_ms.saturating_sub(ticket.created_at));
                    *delivered_by_hire
                        .entry(ticket.hire.to_string())
                        .or_default() += 1;
                    by_project
                        .entry(project.clone())
                        .or_insert_with(|| ProjectScore {
                            name: project.clone(),
                            ..ProjectScore::default()
                        })
                        .delivered += 1;
                }
            }
            TicketEvent::ChecksFailed { .. } | TicketEvent::ReviewFailed { .. } => {
                rework += 1;
                by_project
                    .entry(project.clone())
                    .or_insert_with(|| ProjectScore {
                        name: project.clone(),
                        ..ProjectScore::default()
                    })
                    .rework += 1;
            }
            _ => {}
        }
    }
    let mut blocked = 0;
    let mut blocked_ms = 0;
    let mut workload: BTreeMap<String, Workload> = office
        .status
        .team
        .iter()
        .map(|member| {
            (
                member.id.clone(),
                Workload {
                    person: Person::of(office, &member.id),
                    role: member.role.clone(),
                    active: 0,
                    queued: 0,
                    delivered: delivered_by_hire.get(&member.id).copied().unwrap_or(0),
                    spend: 0.0,
                },
            )
        })
        .collect();
    for ticket in &work_tickets {
        let id = ticket.id.to_string();
        let project = ticket.project.to_string();
        let state = ticket.state.kind();
        let person = ticket.hire.to_string();
        let spend = office.spend.get(&id).copied().unwrap_or(0.0);
        let score = by_project
            .entry(project.clone())
            .or_insert_with(|| ProjectScore {
                name: project,
                ..ProjectScore::default()
            });
        score.spend += spend;
        if !matches!(state, "landed" | "cancelled") {
            score.open += 1;
        }
        if let Some(member) = workload.get_mut(&person) {
            member.spend += spend;
            if ui::at_work(state) {
                member.active += 1;
            } else if state == "queued" {
                member.queued += 1;
            }
        }
        if matches!(state, "asking" | "held" | "failed") {
            blocked += 1;
            let since = office
                .state_since
                .get(&id)
                .copied()
                .or_else(|| {
                    office.last(&id, |event| {
                        matches!(
                            event,
                            TicketEvent::AskOpened { .. }
                                | TicketEvent::Held { .. }
                                | TicketEvent::Failed { .. }
                        )
                    })
                })
                .unwrap_or(office.now);
            blocked_ms += office.now.saturating_sub(since);
        }
    }
    cycles.sort_unstable();
    let median_cycle_ms = match cycles.len() {
        0 => None,
        odd if odd % 2 == 1 => cycles.get(odd / 2).copied(),
        even => {
            let high = even / 2;
            Some((cycles[high - 1] + cycles[high]) / 2)
        }
    };
    let delivered = delivered_ids.len();
    let attempts = delivered + rework;
    let mut projects = by_project.into_values().collect::<Vec<_>>();
    projects.sort_by(|a, b| {
        b.delivered
            .cmp(&a.delivered)
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut workload = workload.into_values().collect::<Vec<_>>();
    workload.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.queued.cmp(&a.queued))
            .then_with(|| a.person.name.cmp(&b.person.name))
    });
    Scorecard {
        delivered,
        median_cycle_ms,
        rework,
        rework_percent: (rework * 100).checked_div(attempts).unwrap_or(0),
        cost_per_deliverable: (delivered > 0).then(|| office.status.spent_24h / delivered as f64),
        spent: office.status.spent_24h,
        blocked,
        blocked_ms,
        projects,
        workload,
    }
}

fn work(office: &Office, ticket: &Ticket) -> Work {
    let id = ticket.id.to_string();
    let project = ticket.project.to_string();
    let state = ticket.state.kind().to_string();
    let next = match ticket.state.kind() {
        "queued" => "Waiting for a desk".to_string(),
        "claimed" | "working" => "Being built now".to_string(),
        "checking" => "Checks are running".to_string(),
        "reviewing" => "An independent review is running".to_string(),
        "ready" => "Ready for your review".to_string(),
        "asking" => "Blocked until you answer".to_string(),
        "held" => ui::state_detail(&ticket.state)
            .map(|reason| format!("Blocked: {reason}"))
            .unwrap_or_else(|| "Blocked or paused".to_string()),
        "failed" => "Needs a retry or a new direction".to_string(),
        "landed" => "Work landed".to_string(),
        "cancelled" => "Cancelled".to_string(),
        _ => "No next step recorded".to_string(),
    };
    Work {
        href: format!("{}?look={id}", lead_href(&project)),
        id: id.clone(),
        title: ticket.title.clone(),
        state,
        icon: ui::shown(&ticket.state),
        next,
        person: Person::of(office, &ticket.hire.to_string()),
        cost: office.spend.get(&id).copied().unwrap_or(0.0),
    }
}

fn projects(office: &Office, team: &Team) -> Vec<Project> {
    team.leads
        .iter()
        .cloned()
        .map(|entry| {
            let goals = floor::all_goals(office)
                .into_iter()
                .filter(|goal| goal.plan.project.to_string() == entry.project)
                .map(|goal| Goal {
                    work: work(office, &goal.plan),
                    tasks: goal.filed.iter().map(|task| work(office, task)).collect(),
                })
                .collect::<Vec<_>>();
            let loose = office
                .tickets
                .iter()
                .filter(|ticket| {
                    ticket.project.to_string() == entry.project
                        && ticket.kind != TicketKind::Plan
                        && ticket.parent.is_none()
                        && ticket.state.kind() != "cancelled"
                })
                .map(|ticket| work(office, ticket))
                .collect();
            let reviewing = office
                .operations
                .reviewing
                .iter()
                .any(|project| project == &entry.project);
            Project {
                entry,
                goals,
                loose,
                reviewing,
            }
        })
        .collect()
}

#[component]
async fn work_row(work: Work, nested: bool) -> topcoat::Result<impl View> {
    let blocked = matches!(work.state.as_str(), "asking" | "held" | "failed");
    let done = work.state == "landed";
    Ok(view! {
        <li class=(if nested { "relative ml-4 border-l border-line-2 pl-4" } else { "" })>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-line py-3 first:border-t-0">
                <a href=(work.href.clone()) class="flex min-w-0 flex-[1_1_280px] items-start gap-2.5 group">
                    <span class="mt-0.5">ui::state_icon(state: work.icon, small: true)</span>
                    <span class="min-w-0">
                        <b class="block text-[13.5px] font-medium group-hover:underline [overflow-wrap:anywhere]">(work.title)</b>
                        <span class=(format!("block text-[12px] {}", if blocked { "text-attn-fg" } else if done { "text-ok-fg" } else { "text-fg-3" }))>(work.next)</span>
                    </span>
                </a>
                <a href=(person_href(&work.person.hire)) class="flex min-w-0 items-center gap-2 text-[12.5px] text-fg-2 hover:underline">
                    faces::face(seed: work.person.hire.clone(), role: work.person.role, tint: 0, size: 24)
                    <span class="max-w-[120px] truncate">(work.person.name)</span>
                </a>
                <span class="w-[70px] text-right text-[12px] tabular-nums text-fg-3">(ui::usd(work.cost))</span>
                <a href=(work.href) class=(ui::BUTTON_SM)>"Open"</a>
            </div>
        </li>
    })
}

#[component]
async fn metric(
    label: &'static str,
    value: String,
    note: &'static str,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="rounded-xl border border-line bg-bg-2 px-4 py-3">
            <span class="block text-[12px] text-fg-3">(label)</span>
            <b class="mt-1 block text-[23px] font-semibold tabular-nums">(value)</b>
            <span class="block text-[11.5px] text-fg-3">(note)</span>
        </div>
    })
}

#[component]
async fn scorecard_view(scorecard: Scorecard) -> topcoat::Result<impl View> {
    let cycle = scorecard
        .median_cycle_ms
        .map(ui::elapsed)
        .unwrap_or_else(|| "—".to_string());
    let cost = scorecard
        .cost_per_deliverable
        .map(ui::usd)
        .unwrap_or_else(|| "—".to_string());
    let blocked = format!(
        "{} · {}",
        scorecard.blocked,
        ui::elapsed(scorecard.blocked_ms)
    );
    Ok(view! {
        <section id="scorecard" class="flex flex-col gap-4" aria-label="Company scorecard">
            <div>
                <h2 class="text-[18px] font-semibold">"Last 24 hours"</h2>
                <p class="m-0 text-[13px] text-fg-3">"Delivery and cost from actual ticket events and the spend ledger. Blocked time is the age of work still blocked now."</p>
            </div>
            <div class="grid grid-cols-4 gap-3 max-[900px]:grid-cols-2 max-[520px]:grid-cols-1">
                metric(label: "Delivered", value: scorecard.delivered.to_string(), note: "work tickets landed")
                metric(label: "Median cycle time", value: cycle, note: "created to landed")
                metric(label: "Sent back", value: format!("{} · {}%", scorecard.rework, scorecard.rework_percent), note: "failed checks or reviews")
                metric(label: "Cost per delivery", value: cost, note: "all company spend ÷ deliveries")
            </div>
            <div class="grid grid-cols-2 gap-3 max-[520px]:grid-cols-1">
                metric(label: "Blocked now", value: blocked, note: "tasks · accumulated age")
                metric(label: "Company spend", value: ui::usd(scorecard.spent), note: "against the daily budget")
            </div>
            if !scorecard.projects.is_empty() {
                <div class="overflow-x-auto rounded-xl border border-line bg-bg-2">
                    <div class="grid min-w-[600px] grid-cols-[minmax(160px,1fr)_100px_100px_100px_120px] gap-3 border-b border-line px-4 py-2 text-[11px] font-semibold uppercase text-fg-3">
                        <span>"Project"</span><span>"Delivered"</span><span>"Open"</span><span>"Sent back"</span><span class="text-right">"Ticket spend"</span>
                    </div>
                    for project in scorecard.projects {
                        <a href=(lead_href(&project.name)) class="grid min-w-[600px] grid-cols-[minmax(160px,1fr)_100px_100px_100px_120px] gap-3 border-b border-line px-4 py-2.5 text-[13px] last:border-b-0 hover:bg-bg-3">
                            <b class="font-medium">(project.name)</b>
                            <span class="tabular-nums">(project.delivered)</span>
                            <span class="tabular-nums">(project.open)</span>
                            <span class="tabular-nums">(project.rework)</span>
                            <span class="text-right tabular-nums">(ui::usd(project.spend))</span>
                        </a>
                    }
                </div>
            }
            <div class="flex flex-col gap-2">
                <h3 class="text-[13px] font-semibold">"Employee workload"</h3>
                <ul class="m-0 flex list-none flex-col rounded-xl border border-line bg-bg-2 p-0 px-4">
                    for person in scorecard.workload {
                        <li class="flex flex-wrap items-center gap-3 border-t border-line py-2.5 first:border-t-0">
                            <a href=(person_href(&person.person.hire)) class="flex min-w-[150px] flex-1 items-center gap-2.5">
                                faces::face(seed: person.person.hire.clone(), role: person.person.role, tint: 0, size: 28)
                                <span class="min-w-0">
                                    <b class="block truncate text-[13px] font-medium">(person.person.name)</b>
                                    <span class="block truncate text-[11.5px] text-fg-3">(person.role)</span>
                                </span>
                            </a>
                            <span class="text-[12px] text-fg-2"><b class="tabular-nums">(person.active)</b> " active"</span>
                            <span class="text-[12px] text-fg-3"><b class="tabular-nums">(person.queued)</b> " queued"</span>
                            <span class="text-[12px] text-fg-3"><b class="tabular-nums">(person.delivered)</b> " delivered"</span>
                            <span class="w-[70px] text-right text-[12px] tabular-nums text-fg-3">(ui::usd(person.spend))</span>
                        </li>
                    }
                </ul>
            </div>
        </section>
    })
}

#[component]
async fn project_map(project: Project) -> topcoat::Result<impl View> {
    let entry = project.entry;
    let goals = project.goals;
    let loose = project.loose;
    let tint = entry.tint;
    let lead = entry.lead.clone();
    let open = goals
        .iter()
        .flat_map(|goal| goal.tasks.iter())
        .chain(loose.iter())
        .filter(|work| !matches!(work.state.as_str(), "landed" | "cancelled"))
        .count();
    Ok(view! {
        <section class="rounded-2xl border border-line bg-bg-2" aria-label=(format!("{} work map", entry.project))>
            <header class="flex flex-wrap items-center gap-3 border-b border-line px-4 py-3.5">
                <a href=(lead_href(&entry.project)) class="flex min-w-0 flex-1 items-center gap-3">
                    faces::face(seed: lead.unwrap_or_else(|| entry.project.clone()), role: entry.role, tint: tint, size: 34)
                    <span class="min-w-0">
                        <b class="block truncate text-[15px] font-semibold">(entry.name)</b>
                        <span class="block truncate text-[12.5px] text-fg-3">(entry.project.clone()) " · " (entry.phase_words) " · " (open) " open"</span>
                    </span>
                </a>
                if entry.lead.is_some() {
                    operations::review_button(project: entry.project.clone(), reviewing: project.reviewing)
                }
            </header>
            <div class="px-4 pb-2">
                if goals.is_empty() && loose.is_empty() {
                    <p class="py-5 text-[13px] text-fg-3">"No work yet. Talk to the lead about the next goal."</p>
                }
                for goal in goals {
                    <div class="py-2">
                        <ul class="m-0 list-none p-0">work_row(work: goal.work, nested: false)</ul>
                        if !goal.tasks.is_empty() {
                            <ul class="m-0 list-none p-0">
                                for task in goal.tasks { work_row(work: task, nested: true) }
                            </ul>
                        }
                    </div>
                }
                if !loose.is_empty() {
                    <ul class="m-0 list-none p-0">
                        for task in loose { work_row(work: task, nested: false) }
                    </ul>
                }
            </div>
        </section>
    })
}

#[component]
async fn manager_log(actions: Vec<ManagerAction>, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <section id="manager-log" class="flex flex-col gap-3" aria-label="Manager decision log">
            <div>
                <h2 class="text-[18px] font-semibold">"Manager decision log"</h2>
                <p class="m-0 text-[13px] text-fg-3">"Every proposed action, who made it, and what happened."</p>
            </div>
            if actions.is_empty() {
                <p class="rounded-xl border border-dashed border-line-2 px-4 py-5 text-[13px] text-fg-3">"Run a manager review to start the decision log."</p>
            } else {
                <ol class="m-0 flex list-none flex-col rounded-xl border border-line bg-bg-2 p-0 px-4">
                    for action in actions.into_iter().take(30) {
                        <li class="border-t border-line py-3 first:border-t-0">
                            <div class="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                                <b class="text-[13.5px] font-semibold">(action.title)</b>
                                <span class="text-[12px] text-fg-3">(action.project) " · " (clock::said(action.at, now))</span>
                                <span class=(format!("ml-auto text-[12px] font-medium {}", match action.state {
                                    OperationState::Pending => "text-attn-fg",
                                    OperationState::Executing => "text-run-fg",
                                    OperationState::Executed => "text-ok-fg",
                                    OperationState::Failed => "text-bad-fg",
                                    OperationState::Rejected => "text-fg-3",
                                }))>(match action.state {
                                    OperationState::Pending => "Needs approval",
                                    OperationState::Executing => "Executing",
                                    OperationState::Executed => "Executed",
                                    OperationState::Rejected => "Rejected",
                                    OperationState::Failed => "Failed",
                                })</span>
                            </div>
                            <p class="m-0 mt-1 text-[12.5px] text-fg-3">(action.act.label()) " · " (action.lead) if !action.result.is_empty() { " · " (action.result) }</p>
                        </li>
                    }
                </ol>
            }
        </section>
    })
}

#[component]
pub async fn company(team: Team, office: Office) -> topcoat::Result<impl View> {
    let projects = projects(&office, &team);
    let scorecard = scorecard(&office);
    let blocked = office
        .tickets
        .iter()
        .filter(|ticket| matches!(ticket.state.kind(), "asking" | "held" | "failed"))
        .count();
    let working = office.status.at_work() as usize;
    let pending = office.operations.pending();
    let actions = office.operations.actions.clone();
    let now = office.now;
    Ok(view! {
        shell::page(team: team, here: Here::Company,
            <div class="mx-auto flex w-full max-w-[1080px] flex-col gap-7">
                <header class="flex flex-wrap items-end justify-between gap-3">
                    <div>
                        <h1 class=(ui::PAGE_TITLE)>"Company"</h1>
                        <p class="m-0 mt-1 text-[14px] text-fg-3">"Who owns what, what is blocked, and what happens next."</p>
                    </div>
                    <a href="/" class=(ui::BUTTON)>"Open approval inbox" if pending > 0 { " · " (pending) }</a>
                </header>
                if let Some(problem) = office.operations.problem.clone() {
                    <p class="rounded-xl border border-bad-line bg-bad-bg px-4 py-3 text-[13px] text-bad-fg" role="alert">(problem) " New model runs and setting changes are blocked."</p>
                }
                <div class="grid grid-cols-3 gap-3 max-[600px]:grid-cols-1" aria-label="Company now">
                    <div class=(ui::CARD)><b class="block text-[24px] font-semibold tabular-nums">(working)</b><span class="text-[12.5px] text-fg-3">"being worked on"</span></div>
                    <div class=(ui::CARD)><b class="block text-[24px] font-semibold tabular-nums">(blocked)</b><span class="text-[12.5px] text-fg-3">"blocked or failed"</span></div>
                    <div class=(ui::CARD)><b class="block text-[24px] font-semibold tabular-nums">(pending)</b><span class="text-[12.5px] text-fg-3">"manager decisions for you"</span></div>
                </div>
                scorecard_view(scorecard: scorecard)
                <section id="work-map" class="flex flex-col gap-4" aria-label="Live work map">
                    <div>
                        <h2 class="text-[18px] font-semibold">"Live work map"</h2>
                        <p class="m-0 text-[13px] text-fg-3">"Goals branch into owned tasks. Open any row for its result, proof and code diff."</p>
                    </div>
                    for project in projects { project_map(project: project) }
                </section>
                manager_log(actions: actions, now: now)
            </div>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client;
    use serde_json::json;

    #[test]
    fn the_map_keeps_goal_relationships_owners_costs_and_next_steps() {
        let plan: Ticket = serde_json::from_value(json!({
            "id":"S-1", "project":"shop", "title":"Plan checkout", "goal":"Checkout",
            "hire":"nora", "state":"landed", "kind":"plan", "attempts":0,
            "base_branch":"main", "created_at":0,
        }))
        .unwrap();
        let task: Ticket = serde_json::from_value(json!({
            "id":"S-2", "project":"shop", "title":"Cart drawer", "goal":"Checkout",
            "hire":"dev", "state":"working", "parent":"S-1", "attempts":0,
            "base_branch":"main", "created_at":0,
        }))
        .unwrap();
        let mut office = Office {
            tickets: vec![plan, task],
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects":["shop"],
            "team":[{"id":"nora","name":"Nora","role":"lead","model":"opus"},{"id":"dev","name":"Dev","role":"engineer","model":"sonnet"}]
        })).unwrap();
        office.projects = vec![studiod::api::ProjectView {
            name: "shop".into(),
            lead: Some("nora".into()),
            ..Default::default()
        }];
        office.spend.insert("S-2".into(), 1.25);
        let team = Team::of(&office);
        let projects = projects(&office, &team);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].goals[0].tasks.len(), 1);
        let task = &projects[0].goals[0].tasks[0];
        assert_eq!(task.person.name, "Dev");
        assert_eq!(task.cost, 1.25);
        assert_eq!(task.next, "Being built now");
        assert_eq!(task.href, "/leads/shop?look=S-2");
    }

    #[test]
    fn the_scorecard_uses_real_delivery_rework_spend_and_workload() {
        let ticket: Ticket = serde_json::from_value(json!({
            "id":"S-1", "project":"shop", "title":"Cart drawer", "goal":"Checkout",
            "hire":"dev", "state":"landed", "attempts":1,
            "base_branch":"main", "created_at":1_000,
        }))
        .unwrap();
        let mut office = Office {
            tickets: vec![ticket],
            now: 100_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects":["shop"], "spent_24h":10.0,
            "team":[{"id":"dev","name":"Dev","role":"engineer","model":"sonnet"}]
        }))
        .unwrap();
        office.recent = vec![
            client::Happened {
                ticket_id: "S-1".into(),
                at_ms: 50_000,
                event: TicketEvent::ChecksFailed {
                    detail: "no".into(),
                },
            },
            client::Happened {
                ticket_id: "S-1".into(),
                at_ms: 61_000,
                event: TicketEvent::Landed,
            },
        ];
        office.spend.insert("S-1".into(), 2.0);
        let score = scorecard(&office);
        assert_eq!(score.delivered, 1);
        assert_eq!(score.median_cycle_ms, Some(60_000));
        assert_eq!(score.rework, 1);
        assert_eq!(score.rework_percent, 50);
        assert_eq!(score.cost_per_deliverable, Some(10.0));
        assert_eq!(score.projects[0].spend, 2.0);
        assert_eq!(score.workload[0].delivered, 1);
        assert_eq!(score.workload[0].role, "engineer");
    }

    #[test]
    fn blocked_time_uses_the_current_state_timestamp_or_zero_when_unknown() {
        let ticket = |id: &str| {
            serde_json::from_value::<Ticket>(json!({
                "id":id, "project":"shop", "title":"Failed work", "goal":"",
                "hire":"dev", "state":{"failed":{"class":"tooling","detail":"no"}},
                "attempts":1, "base_branch":"main", "created_at":1_000,
            }))
            .unwrap()
        };
        let mut office = Office {
            tickets: vec![ticket("S-1"), ticket("S-2")],
            now: 100_000,
            ..Office::default()
        };
        office.state_since.insert("S-1".into(), 50_000);
        let score = scorecard(&office);
        assert_eq!(score.blocked, 2);
        assert_eq!(score.blocked_ms, 50_000);
    }
}
