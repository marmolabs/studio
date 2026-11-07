use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    priority: u32,
    dependencies: Vec<String>,
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
    graph: Vec<GraphNode>,
    reviewing: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct GraphNode {
    work: Work,
    depth: usize,
    critical: bool,
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
    let blocked_by = ticket
        .depends_on
        .iter()
        .filter(|dependency| {
            office
                .ticket(&dependency.to_string())
                .is_none_or(|ticket| ticket.state.kind() != "landed")
        })
        .map(|dependency| dependency.to_string())
        .collect::<Vec<_>>();
    let next = match ticket.state.kind() {
        "queued" if !blocked_by.is_empty() => format!("Blocked by {}", blocked_by.join(", ")),
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
        priority: ticket.priority,
        dependencies: ticket.depends_on.iter().map(|id| id.to_string()).collect(),
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
            let graph = dependency_graph(office, &entry.project);

            Project {
                entry,
                goals,
                loose,
                graph,
                reviewing,
            }
        })
        .collect()
}

fn graph_depth(
    id: &str,
    edges: &HashMap<String, Vec<String>>,
    cache: &mut HashMap<String, usize>,
    visiting: &mut HashSet<String>,
) -> usize {
    if let Some(depth) = cache.get(id) {
        return *depth;
    }
    if !visiting.insert(id.to_string()) {
        return 0;
    }
    let depth = edges
        .get(id)
        .and_then(|dependencies| {
            dependencies
                .iter()
                .map(|dependency| graph_depth(dependency, edges, cache, visiting))
                .max()
        })
        .map_or(0, |depth| depth + 1);
    visiting.remove(id);
    cache.insert(id.to_string(), depth);
    depth
}

fn dependency_graph(office: &Office, project: &str) -> Vec<GraphNode> {
    let tickets = office
        .tickets
        .iter()
        .filter(|ticket| {
            ticket.project.to_string() == project
                && ticket.kind == TicketKind::Work
                && !matches!(ticket.state.kind(), "landed" | "cancelled")
        })
        .collect::<Vec<_>>();
    let open: HashSet<String> = tickets.iter().map(|ticket| ticket.id.to_string()).collect();
    let edges: HashMap<String, Vec<String>> = tickets
        .iter()
        .map(|ticket| {
            let dependencies = ticket
                .depends_on
                .iter()
                .map(|id| id.to_string())
                .filter(|id| open.contains(id))
                .collect();
            (ticket.id.to_string(), dependencies)
        })
        .collect();
    let mut depths = HashMap::new();
    for ticket in &tickets {
        graph_depth(
            &ticket.id.to_string(),
            &edges,
            &mut depths,
            &mut HashSet::new(),
        );
    }
    let mut end = None;
    let mut deepest = 0;
    for ticket in &tickets {
        let depth = depths.get(&ticket.id.to_string()).copied().unwrap_or(0);
        if depth > deepest {
            deepest = depth;
            end = Some(ticket.id.to_string());
        }
    }
    let mut critical = BTreeSet::new();
    let mut current = end;
    while let Some(id) = current {
        if !critical.insert(id.clone()) {
            break;
        }
        current = edges
            .get(&id)
            .into_iter()
            .flatten()
            .max_by_key(|dependency| depths.get(*dependency).copied().unwrap_or(0))
            .cloned();
    }
    // Only tasks that wait for another, or that another waits for, are in the
    // graph; the rest are in the list above it.
    let awaited: HashSet<&String> = edges.values().flatten().collect();
    tickets
        .into_iter()
        .filter(|ticket| {
            let id = ticket.id.to_string();
            edges.get(&id).is_some_and(|waits| !waits.is_empty()) || awaited.contains(&id)
        })
        .map(|ticket| {
            let id = ticket.id.to_string();
            GraphNode {
                work: work(office, ticket),
                depth: depths.get(&id).copied().unwrap_or(0),
                critical: critical.contains(&id),
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
                if work.priority != studio_core::ticket::DEFAULT_PRIORITY {
                    <span class="rounded-full border border-line-2 px-2 py-0.5 text-[11px] tabular-nums text-fg-2">"P" (work.priority)</span>
                }
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
    // Someone with nothing to do and nothing done is a line, not a row.
    let (workload, idle): (Vec<_>, Vec<_>) = scorecard.workload.into_iter().partition(|person| {
        person.active + person.queued + person.delivered > 0 || person.spend > 0.0
    });
    let idle = idle
        .into_iter()
        .map(|person| person.person.name)
        .collect::<Vec<_>>()
        .join(", ");
    let blocked = format!(
        "{} · {}",
        scorecard.blocked,
        ui::elapsed(scorecard.blocked_ms)
    );
    Ok(view! {
        <section id="scorecard" class="flex flex-col gap-4" aria-label="Company scorecard">
            <div>
                <h2 class="text-[18px] font-semibold">"Last 24 hours"</h2>
                <p class="m-0 text-[13px] text-fg-3">"What was delivered and what it cost. Blocked time is how long the work that is blocked now has been waiting."</p>
            </div>
            <div class="grid grid-cols-4 gap-3 max-[900px]:grid-cols-2 max-[520px]:grid-cols-1">
                metric(label: "Delivered", value: scorecard.delivered.to_string(), note: "tasks landed")
                metric(label: "Typical time to deliver", value: cycle, note: "from asked for to landed")
                metric(label: "Sent back", value: format!("{} · {}%", scorecard.rework, scorecard.rework_percent), note: "after failed checks or reviews")
                metric(label: "Cost per delivery", value: cost, note: "all spend divided by deliveries")
            </div>
            <div class="grid grid-cols-2 gap-3 max-[520px]:grid-cols-1">
                metric(label: "Blocked now", value: blocked, note: "tasks · time waiting")
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
                if !workload.is_empty() {
                <ul class="m-0 flex list-none flex-col rounded-xl border border-line bg-bg-2 p-0 px-4">
                    for person in workload {
                        <li class="flex flex-wrap items-center gap-3 border-t border-line py-2.5 first:border-t-0">
                            <a href=(person_href(&person.person.hire)) class="flex min-w-[150px] flex-1 items-center gap-2.5">
                                faces::face(seed: person.person.hire.clone(), role: person.person.role, tint: 0, size: 28)
                                <span class="min-w-0">
                                    <b class="block truncate text-[13px] font-medium">(person.person.name)</b>
                                    <span class="block truncate text-[11.5px] text-fg-3">(person.role)</span>
                                </span>
                            </a>
                            <span class="flex flex-wrap items-center gap-x-3 gap-y-0.5 max-[520px]:basis-full max-[520px]:pl-[38px]">
                                <span class="text-[12px] text-fg-2"><b class="tabular-nums">(person.active)</b> " active"</span>
                                <span class="text-[12px] text-fg-3"><b class="tabular-nums">(person.queued)</b> " queued"</span>
                                <span class="text-[12px] text-fg-3"><b class="tabular-nums">(person.delivered)</b> " delivered"</span>
                                <span class="min-w-[56px] text-[12px] tabular-nums text-fg-3 sm:text-right">(ui::usd(person.spend))</span>
                            </span>
                        </li>
                    }
                </ul>
                }
                if !idle.is_empty() {
                    <p class="m-0 text-[12.5px] text-fg-3">"Nothing assigned yet: " (idle)</p>
                }
            </div>
        </section>
    })
}

#[component]
async fn graph_view(nodes: Vec<GraphNode>) -> topcoat::Result<impl View> {
    let deepest = nodes.iter().map(|node| node.depth).max().unwrap_or(0);
    let critical = nodes
        .iter()
        .filter(|node| node.critical)
        .map(|node| node.work.title.clone())
        .collect::<Vec<_>>()
        .join(" → ");
    Ok(view! {
        <section class="border-t border-line px-4 py-4" aria-label="Dependency graph">
            <div class="mb-3">
                <h3 class="text-[12px] font-medium uppercase text-fg-3">"Dependency graph"</h3>
                if !critical.is_empty() {
                    <p class="m-0 mt-1 text-[12px] text-attn-fg">"Critical path: " (critical)</p>
                }
            </div>
            <div class="flex items-stretch gap-3 overflow-x-auto pb-2">
                for depth in 0..=deepest {
                    <div class="flex min-w-[210px] flex-1 flex-col gap-2" data-depth=(depth)>
                        for node in nodes.iter().filter(|node| node.depth == depth) {
                            <div class=(format!("rounded-xl border px-3 py-2.5 {}", if node.critical { "border-attn-line bg-attn-bg" } else { "border-line bg-bg" })) data-critical=(node.critical) data-task=(node.work.id.clone())>
                                <a href=(node.work.href.clone()) class="block text-[13px] font-medium hover:underline">(node.work.title.clone())</a>
                                <span class="mt-1 block text-[11.5px] text-fg-3">"P" (node.work.priority) " · " (node.work.next.clone())</span>
                                if !node.work.dependencies.is_empty() {
                                    <span class="mt-1 block text-[11.5px] text-fg-3">"after "
                                        for (index, dependency) in node.work.dependencies.iter().enumerate() {
                                            if index > 0 { ", " }
                                            <a href=(format!("/tickets/{dependency}")) class="font-mono hover:underline">(dependency)</a>
                                        }
                                    </span>
                                }
                            </div>
                        }
                    </div>
                    if depth < deepest {
                        <span class="self-center text-fg-3" aria-hidden="true">"→"</span>
                    }
                }
            </div>
        </section>
    })
}

#[component]
async fn project_map(project: Project) -> topcoat::Result<impl View> {
    let entry = project.entry;
    let goals = project.goals;
    let loose = project.loose;
    let graph = project.graph;
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
            if !graph.is_empty() {
                graph_view(nodes: graph)
            }
        </section>
    })
}

#[component]
async fn manager_log(
    actions: Vec<(ManagerAction, String)>,
    now: u64,
) -> topcoat::Result<impl View> {
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
                    for (action, lead) in actions.into_iter().take(30) {
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
                                    OperationState::Pending => "Waiting for you",
                                    OperationState::Executing => "Running now",
                                    OperationState::Executed => "Done",
                                    OperationState::Rejected => "Rejected",
                                    OperationState::Failed => "Failed",
                                })</span>
                            </div>
                            <p class="m-0 mt-1 text-[12.5px] text-fg-3">(action.act.label()) " · " (lead) if !action.result.is_empty() { " · " (action.result) }</p>
                            if let Some(detail) = operations::detail(&action.act) {
                                <p class="m-0 mt-1 whitespace-pre-wrap text-[12.5px] text-fg-2 [overflow-wrap:anywhere]">(detail)</p>
                            }
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
    let actions = office
        .operations
        .actions
        .iter()
        .map(|action| (action.clone(), office.name_of(&action.lead)))
        .collect::<Vec<_>>();
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
                    <div class=(format!("{} px-4 py-3", ui::CARD))><b class="block text-[24px] font-semibold tabular-nums">(working)</b><span class="text-[12.5px] text-fg-3">"being worked on"</span></div>
                    <div class=(format!("{} px-4 py-3", ui::CARD))><b class="block text-[24px] font-semibold tabular-nums">(blocked)</b><span class="text-[12.5px] text-fg-3">"blocked or failed"</span></div>
                    <div class=(format!("{} px-4 py-3", ui::CARD))><b class="block text-[24px] font-semibold tabular-nums">(pending)</b><span class="text-[12.5px] text-fg-3">"manager decisions for you"</span></div>
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
    fn the_dependency_graph_has_depth_and_a_critical_path() {
        let ticket = |id: &str, depends_on: &[&str]| {
            serde_json::from_value::<Ticket>(json!({
                "id":id, "project":"shop", "title":id, "goal":"",
                "hire":"dev", "state":"queued", "attempts":0,
                "base_branch":"main", "created_at":1, "depends_on":depends_on,
            }))
            .unwrap()
        };
        let office = Office {
            tickets: vec![
                ticket("S-1", &[]),
                ticket("S-2", &["S-1"]),
                ticket("S-3", &["S-2"]),
            ],
            ..Office::default()
        };
        let graph = dependency_graph(&office, "shop");
        assert_eq!(
            graph.iter().map(|node| node.depth).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(graph.iter().all(|node| node.critical));
        assert_eq!(graph[1].work.next, "Blocked by S-1");
    }

    #[test]
    fn tasks_that_wait_for_nothing_are_not_a_graph() {
        let ticket = |id: &str, depends_on: &[&str]| {
            serde_json::from_value::<Ticket>(json!({
                "id":id, "project":"shop", "title":id, "goal":"",
                "hire":"dev", "state":"queued", "attempts":0,
                "base_branch":"main", "created_at":1, "depends_on":depends_on,
            }))
            .unwrap()
        };
        // Four open tasks, two of which wait for one another; a dependency
        // that has landed (or is not here) is not a line in the graph.
        let office = Office {
            tickets: vec![
                ticket("S-1", &[]),
                ticket("S-2", &["S-1"]),
                ticket("S-3", &[]),
                ticket("S-4", &["S-9"]),
            ],
            ..Office::default()
        };
        let ids = |graph: Vec<GraphNode>| {
            graph
                .into_iter()
                .map(|node| node.work.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(dependency_graph(&office, "shop")), ["S-1", "S-2"]);
        let alone = Office {
            tickets: vec![ticket("S-1", &[]), ticket("S-3", &[])],
            ..Office::default()
        };
        assert!(dependency_graph(&alone, "shop").is_empty());
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
