//! Who sits where on the floor, and the rooms that show it.

use serde_json::Value;
use std::collections::HashMap;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{HeldReason, Ticket, TicketKind, TicketState};
use studiod::api::{AskView, HireDay};
use topcoat::view::{Child, View, component, view};

use super::{Office, ago, key, pressed_when, select};
use crate::pages::ask_card::{opened, questions};
use crate::pages::board::finished;
use crate::summary::{Judge, tool_line, truncate};
use crate::{client, ui, words};

/// Where a ticket is on the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Room {
    /// At a desk: its agent is at work, or it waits on the owner.
    Desk,
    Checks,
    Review,
    /// A producer at the planning table.
    Planning,
    Ready,
    UpNext,
    /// Landed, failed or cancelled.
    Done,
}

/// Where a ticket is; one asking or held stays where it stopped, so a
/// critic's question waits in review.
pub fn room(ticket: &Ticket) -> Room {
    let plan = ticket.kind == TicketKind::Plan;
    let stopped = matches!(ticket.state.kind(), "asking" | "held");
    let kind = match &ticket.held_from {
        Some(from) if stopped => from.kind(),
        _ => ticket.state.kind(),
    };
    match kind {
        "queued" => Room::UpNext,
        "checking" => Room::Checks,
        "reviewing" => Room::Review,
        "ready" => Room::Ready,
        _ if stopped && plan => Room::Planning,
        _ if stopped => Room::Desk,
        "claimed" | "working" if plan => Room::Planning,
        "claimed" | "working" => Room::Desk,
        _ => Room::Done,
    }
}

/// Free desks: run slots no run holds. Checks, review and planning hold a
/// slot too; a held ticket keeps its desk but holds no slot.
pub fn free_desks(status: &client::Status) -> usize {
    usize::try_from(status.max_runs.saturating_sub(status.running)).unwrap_or(0)
}

/// Roles that sit at the front desk rather than in the lounge.
pub const FRONT: [&str; 2] = ["approver", "router"];
/// Up next shows this many, then "+n".
const QUEUE_SHOWN: usize = 2;
const SHIPPED_SHOWN: usize = 5;

/// A plan ticket and the tickets it filed.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    pub plan: Ticket,
    /// Filed tickets, cancelled ones left out.
    pub filed: Vec<Ticket>,
    pub merged: bool,
}

impl Goal {
    pub fn landed(&self) -> usize {
        self.filed
            .iter()
            .filter(|t| t.state.kind() == "landed")
            .count()
    }

    /// The goal's first line.
    pub fn name(&self) -> String {
        self.plan
            .goal
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// As `ui::look` reads a goal.
    pub fn state(&self) -> &'static str {
        ui::goal_state(&self.plan, &self.filed, self.merged)
    }

    /// A ticket on the goal waits for the owner, or failed.
    pub fn needs_you(&self) -> bool {
        self.state() == "needs attention"
    }
}

/// Every goal still open: not failed or cancelled, and not merged.
pub fn goals(office: &Office) -> Vec<Goal> {
    office
        .tickets
        .iter()
        .filter(|t| t.kind == TicketKind::Plan)
        .filter(|t| !matches!(t.state.kind(), "failed" | "cancelled"))
        .map(|plan| {
            let family: Vec<&Ticket> = office
                .tickets
                .iter()
                .filter(|t| t.parent.as_ref() == Some(&plan.id))
                .collect();
            Goal {
                plan: plan.clone(),
                filed: family
                    .iter()
                    .filter(|t| t.state.kind() != "cancelled")
                    .map(|t| (*t).clone())
                    .collect(),
                merged: family
                    .iter()
                    .any(|t| office.merged.contains(&t.id.to_string())),
            }
        })
        .filter(|g| g.state() != "merged")
        .collect()
}

/// Why a ticket is held, short enough for a desk.
fn held_short(state: &TicketState) -> &'static str {
    match state {
        TicketState::Held { reason } => match reason {
            HeldReason::Budget => "budget",
            HeldReason::Quota => "quota",
            HeldReason::Auth => "login",
            HeldReason::Conflict => "conflict",
            HeldReason::Paused => "by you",
        },
        _ => "",
    }
}

/// The project checks a run log names since `since` (epoch ms): each
/// command and when it started, oldest first.
pub fn checks_since(log: &str, since: u64) -> Vec<(String, u64)> {
    log.lines()
        .filter_map(|raw| {
            let entry: Value = serde_json::from_str(raw).ok()?;
            let at = entry["at"].as_u64().unwrap_or(0);
            let command = entry["note"].as_str()?.strip_prefix("check: ")?;
            (at >= since).then(|| (command.to_string(), at))
        })
        .collect()
}

/// The exact call an ask is about, for a tooltip.
pub fn ask_call(ask: &AskView) -> String {
    match ask.kind.as_str() {
        "question" => questions(&ask.input)
            .into_iter()
            .next()
            .map_or_else(|| "A question".into(), |(text, _)| text),
        _ => tool_line(&ask.tool, &ask.input, 400),
    }
}

/// When a hire's last run of its own ended, if the window shows it: its
/// latest ticket going to checks, being held, failing or being cancelled,
/// or its plan landing.
pub fn idle_since(office: &Office, hire: &str) -> Option<u64> {
    // Whether each of the hire's tickets is a plan.
    let own: HashMap<String, bool> = office
        .tickets
        .iter()
        .filter(|t| t.hire.to_string() == hire)
        .map(|t| (t.id.to_string(), t.kind == TicketKind::Plan))
        .collect();
    office
        .recent
        .iter()
        .rev()
        .find(|e| {
            own.get(&e.ticket_id).is_some_and(|plan| match e.event {
                TicketEvent::ChecksStarted
                | TicketEvent::Held { .. }
                | TicketEvent::Failed { .. }
                | TicketEvent::Cancelled => true,
                TicketEvent::Landed => *plan,
                _ => false,
            })
        })
        .map(|e| e.at_ms)
}

/// Tickets that finished in the window, newest first, with when.
pub fn shipped(office: &Office) -> Vec<(Ticket, u64)> {
    finished(&office.recent, &office.tickets)
}

/// Hires with nothing running: everyone but the front desk who is not at a
/// desk, the planning table or the review table.
pub fn resting(office: &Office) -> Vec<client::Member> {
    let busy: Vec<String> = office
        .tickets
        .iter()
        .filter(|t| matches!(room(t), Room::Desk | Room::Planning))
        .map(|t| t.hire.to_string())
        .chain(
            office
                .tickets
                .iter()
                .filter(|t| room(t) == Room::Review)
                .filter_map(|t| Some(office.judges.get(&t.id.to_string())?.hire.clone())),
        )
        .collect();
    office
        .status
        .team
        .iter()
        .filter(|m| !FRONT.contains(&m.role.as_str()) && !busy.contains(&m.id))
        .cloned()
        .collect()
}

pub fn front_desk(office: &Office) -> Vec<client::Member> {
    FRONT
        .iter()
        .filter_map(|role| office.status.team.iter().find(|m| m.role == *role))
        .cloned()
        .collect()
}

fn in_room(office: &Office, which: Room) -> Vec<Ticket> {
    office
        .tickets
        .iter()
        .filter(|t| room(t) == which)
        .cloned()
        .collect()
}

// Class strings; literal so Tailwind finds them.
const ROOM: &str =
    "flex min-w-0 flex-col gap-3 rounded-[14px] border border-line-2 bg-bg px-3.5 pb-3.5 pt-3";
const PICKED: &str = "aria-pressed:border-line-2 aria-pressed:bg-bg-2";
const STATION: &str = "relative flex min-w-0 cursor-pointer flex-col items-center gap-[5px] rounded-[10px] border border-transparent px-1.5 pb-2.5 pt-2 text-center transition-colors hover:border-line-2 hover:bg-bg-2";
const SITTER: &str = "flex min-w-0 cursor-pointer flex-col items-center gap-1.5 rounded-[10px] border border-transparent px-2 py-1.5 transition-colors hover:border-line-2 hover:bg-bg-2";
const CARD: &str = "flex w-full min-w-0 cursor-pointer flex-col gap-1.5 rounded-[9px] border bg-bg px-3 py-2.5 text-left text-[12.5px] transition-colors hover:bg-bg-2";
const TID: &str = "mt-[3px] font-mono text-[11px] font-medium leading-[14px] text-fg-3";
const TITLE: &str =
    "line-clamp-2 max-w-full text-[12.5px] font-medium leading-4 text-fg [text-wrap:balance]";
const SLOT: &str = "grid min-h-10 place-items-center rounded-[9px] border border-dashed border-line-2 text-[12px] text-fg-3";

fn area(name: &str) -> &'static str {
    match name {
        "work" => "[grid-area:work]",
        "checks" => "[grid-area:checks]",
        "plan" => "[grid-area:plan]",
        "review" => "[grid-area:review]",
        "ready" => "[grid-area:ready]",
        "lounge" => "[grid-area:lounge]",
        "front" => "[grid-area:front]",
        _ => "[grid-area:shipped]",
    }
}

/// `"true"` or `"false"`, for the first paint of `aria-pressed`.
fn pressed(sel: &str, key: &str) -> &'static str {
    if sel == key { "true" } else { "false" }
}

#[component]
async fn room_box(
    name: &'static str,
    label: &'static str,
    #[into] count: String,
    #[into]
    #[default]
    hint: String,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <section class=(format!("{ROOM} {}", area(name))) aria-label=(label)>
            <div class="flex items-baseline justify-between gap-2">
                <h2 class="text-[10.5px] font-semibold uppercase leading-[14px] tracking-[0.08em] text-fg-3">(label)</h2>
                <span class="truncate text-[12px] text-fg-3" title=(hint)>(count)</span>
            </div>
            (child)
        </section>
    })
}

#[component]
pub async fn floor(office: Office, sel: String) -> topcoat::Result<impl View> {
    let seated = in_room(&office, Room::Desk);
    let free = free_desks(&office.status);
    let checks = in_room(&office, Room::Checks);
    let review = in_room(&office, Room::Review);
    let ready = in_room(&office, Room::Ready);
    let queued = in_room(&office, Room::UpNext);
    let goals = goals(&office);
    let resting = resting(&office);
    let front = front_desk(&office);
    let shipped = shipped(&office);
    let now = office.now;

    let at_desks = format!("{} at their desks", seated.len());
    let paused = office.status.max_runs == 0;
    let running = format!(
        "{} running",
        checks
            .iter()
            .filter(|t| t.state.kind() == "checking")
            .count()
    );
    let reviewing = format!("{} reviewing", review.len());
    let open_goals = match goals.len() {
        1 => "1 goal open".to_string(),
        n => format!("{n} goals open"),
    };
    let ready_count = match ready.len() {
        1 => "1 ticket".to_string(),
        n => format!("{n} tickets"),
    };
    let resting_count = format!("{} resting", resting.len());
    let no_hires = !office
        .status
        .team
        .iter()
        .any(|m| !FRONT.contains(&m.role.as_str()));
    let landed = shipped
        .iter()
        .filter(|(t, _)| t.state.kind() == "landed")
        .count();
    let failed = shipped
        .iter()
        .filter(|(t, _)| t.state.kind() == "failed")
        .count();
    let shipped_count = if failed > 0 {
        format!("{landed} landed · {failed} failed")
    } else {
        format!("{landed} landed")
    };
    let window = office.window();
    let shipped_hint = format!("Landed, failed or cancelled in {window}");
    let nothing_shipped = format!("Nothing finished in {window}");
    let more_shipped = shipped.len().saturating_sub(SHIPPED_SHOWN);
    let shipped: Vec<(Ticket, f64)> = shipped
        .into_iter()
        .take(SHIPPED_SHOWN)
        .map(|(t, _)| {
            let spent = office.spent(&t.id.to_string());
            (t, spent)
        })
        .collect();
    let more_queued = queued.len().saturating_sub(QUEUE_SHOWN);
    let queued: Vec<Ticket> = queued.into_iter().take(QUEUE_SHOWN).collect();
    let lit = checks.len().min(3);
    let nothing_ready = if ready.is_empty() {
        "Nothing ready to land"
    } else {
        "Nothing else ready"
    };
    let seated: Vec<Seat> = seated.into_iter().map(|t| Seat::of(&office, t)).collect();
    let checks: Vec<(Ticket, Option<String>, Option<u64>)> = checks
        .into_iter()
        .map(|t| {
            let (command, since) = running_check(&office, &t.id.to_string());
            (t, command, since)
        })
        .collect();
    let review: Vec<(Ticket, Option<Judge>, Option<String>)> = review
        .into_iter()
        .map(|t| {
            let id = t.id.to_string();
            let judge = office.judges.get(&id).cloned();
            let doing = judge
                .as_ref()
                .and_then(|j| j.action(office.log(&id), 60))
                .map(|a| words::action_text(&a));
            (t, judge, doing)
        })
        .collect();
    let ready: Vec<_> = ready
        .into_iter()
        .map(|t| {
            let id = t.id.to_string();
            (t, office.sizes.get(&id).copied(), office.spent(&id))
        })
        .collect();
    let goals: Vec<(Goal, bool)> = goals
        .into_iter()
        .map(|g| {
            let asking = office
                .asks
                .iter()
                .any(|a| a.ticket == g.plan.id.to_string());
            (g, asking)
        })
        .collect();
    let resting: Vec<(client::Member, Option<String>)> = resting
        .into_iter()
        .map(|m| {
            let idle = idle_since(&office, &m.id).map(|at| ago(now.saturating_sub(at)));
            (m, idle)
        })
        .collect();

    Ok(view! {
        <div id="office-floor" class="floor" role="group" aria-label="The office floor: pick anyone to see their work">
            room_box(name: "work", label: "Workspace", count: at_desks,
                if paused && seated.is_empty() {
                    <div class=(SLOT)>"Paused: max_runs is 0, so nothing new starts"</div>
                } else {
                    <div class="desks">
                        for seat in seated {
                            desk(seat: seat, sel: sel.clone())
                        }
                        for _ in 0..free {
                            <div class="flex min-w-0 flex-col items-center gap-[5px] px-1.5 pb-2.5 pt-2" aria-label="Free desk" role="img">
                                <span class="mb-1 h-6 shrink-0"></span>
                                <span class="seat"></span>
                                <span class="workdesk free"><i></i></span>
                                <span class="text-[11.5px] leading-4 text-fg-3">"Free desk"</span>
                            </div>
                        }
                    </div>
                }
            )
            room_box(name: "checks", label: "Checks", count: running,
                <div class="grid grid-cols-3 gap-2" aria-hidden="true">
                    for i in 0..3 {
                        <span class="rack">
                            for j in 0..8 {
                                <i class=(rack_light(i < lit, i + j))></i>
                            }
                        </span>
                    }
                </div>
                for (ticket, command, since) in checks {
                    checks_card(ticket: ticket, command: command, since: since, now: now, sel: sel.clone())
                }
            )
            room_box(name: "plan", label: "Planning", count: open_goals,
                if !goals.is_empty() {
                    <div class="flex flex-col gap-0.5 rounded-[9px] border border-line-2 bg-bg-2 p-1.5">
                        for (goal, asking) in goals {
                            goal_row(goal: goal, asking: asking, sel: sel.clone())
                        }
                    </div>
                } else {
                    <div class=(SLOT)>"No goals open"</div>
                }
            )
            room_box(name: "review", label: "Review", count: reviewing,
                if review.is_empty() {
                    <div class=(format!("{SLOT} my-auto"))>"Nobody in review"</div>
                } else {
                    <div class="my-auto flex flex-wrap justify-center gap-2">
                        for (ticket, judge, doing) in review {
                            review_seat(ticket: ticket, judge: judge, doing: doing, sel: sel.clone())
                        }
                    </div>
                }
            )
            room_box(name: "ready", label: "Ready to land", count: ready_count,
                for (ticket, size, spent) in ready {
                    ready_card(ticket: ticket, size: size, spent: spent, sel: sel.clone())
                }
                <div class=(SLOT)>(nothing_ready)</div>
            )
            room_box(name: "lounge", label: "Lounge", count: resting_count,
                <div class="flex items-end justify-between gap-3">
                    if no_hires {
                        <div class=(format!("{SLOT} flex-1"))>"No hires yet"</div>
                    } else if resting.is_empty() {
                        <div class=(format!("{SLOT} flex-1"))>"Everyone is at work"</div>
                    } else {
                        <div class="sofa flex flex-wrap gap-1.5">
                            for (member, idle) in resting {
                                rester(member: member, idle: idle, sel: sel.clone())
                            }
                        </div>
                    }
                    <div class="flex gap-2" aria-hidden="true">
                        <span class="prop">ui::icon(name: "coffee")</span>
                        <span class="prop plant">ui::icon(name: "sprout")</span>
                    </div>
                </div>
            )
            room_box(name: "front", label: "Front desk", count: "Calls and routing",
                if !front.is_empty() {
                    <div class="frontdesk">
                        for member in front {
                            front_hire(day: office.today.get(&member.id).cloned(), member: member, sel: sel.clone())
                        }
                    </div>
                }
                <div class="flex min-w-0 flex-col gap-1.5 text-[12px]">
                    if queued.is_empty() {
                        <span class="text-fg-3">"Up next: nothing queued"</span>
                    } else {
                        <span class="text-fg-3">"Up next"</span>
                        // One line: the chips that fit, then how many more.
                        <div class="flex min-w-0 items-center gap-2">
                            <span class="flex min-w-0 gap-2 overflow-hidden">
                                for ticket in queued {
                                    queued_card(ticket: ticket, sel: sel.clone())
                                }
                            </span>
                            if more_queued > 0 {
                                <a href="/board#up-next" class="shrink-0 text-fg-3 hover:text-fg" title="See everything queued on the board">"+" (more_queued)</a>
                            }
                        </div>
                    }
                </div>
            )
            room_box(name: "shipped", label: "Shipped", count: shipped_count, hint: shipped_hint,
                if shipped.is_empty() {
                    <div class=(SLOT)>(nothing_shipped)</div>
                } else {
                    <div class="@container grid grid-cols-[auto_auto_minmax(0,1fr)_auto] gap-x-2">
                        for (ticket, spent) in shipped {
                            shipped_row(ticket: ticket, spent: spent)
                        }
                        if more_shipped > 0 {
                            <a href="/board#done" class="col-span-full px-1.5 pt-1 text-[12px] text-fg-3 hover:text-fg">"+" (more_shipped) " more"</a>
                        }
                    </div>
                }
            )
        </div>
    })
}

/// The command a ticket's checks are running now and since when, else when
/// its checks started.
fn running_check(office: &Office, id: &str) -> (Option<String>, Option<u64>) {
    let started = office.last(id, |e| matches!(e, TicketEvent::ChecksStarted));
    let current = started.and_then(|since| checks_since(office.log(id), since).pop());
    match current {
        Some((command, at)) => (Some(command), Some(at)),
        None => (None, started),
    }
}

/// A rack light: on for a rack in use, with every other light out of step.
fn rack_light(on: bool, n: usize) -> &'static str {
    match (on, n % 3) {
        (false, _) => "",
        (true, 0) => "on",
        (true, 1) => "on b",
        (true, _) => "",
    }
}

/// How a person looks: the ring's colour class and how full it is. Work
/// held for money or quota starts again by itself, so it isn't the owner's.
fn person_look(state: &TicketState) -> (&'static str, u8) {
    match state.kind() {
        "asking" | "held" if ui::needs_owner(state) => ("attn", 25),
        "held" => ("held", 25),
        "reviewing" => ("review", 85),
        kind => ("run", ui::look(kind).ring),
    }
}

/// A hire's face in its ring; 28px when `small`.
#[component]
async fn person(
    #[into] hire: String,
    tone: &'static str,
    ring: u8,
    #[default] badge: bool,
    #[default] small: bool,
) -> topcoat::Result<impl View> {
    let class = format!(
        "person {tone} {}{}",
        ui::hue(&hire),
        if small { " sm" } else { "" }
    );
    Ok(view! {
        <span class=(class) style=(format!("--p:{ring}"))>
            <span class="face">(ui::initials(&hire))</span>
            if badge {
                <span class="badge" aria-hidden="true">"!"</span>
            }
        </span>
    })
}

/// What a desk shows of a seated ticket.
#[derive(Clone, Debug)]
struct Seat {
    ticket: Ticket,
    ask: Option<AskView>,
    /// How long its ask has waited.
    waited: Option<u64>,
}

impl Seat {
    fn of(office: &Office, ticket: Ticket) -> Seat {
        let id = ticket.id.to_string();
        let ask = office.asks.iter().find(|a| a.ticket == id).cloned();
        let waited = ask
            .as_ref()
            .and_then(opened)
            .map(|at| office.now.saturating_sub(at));
        Seat {
            ticket,
            ask,
            waited,
        }
    }
}

#[component]
async fn desk(seat: Seat, sel: String) -> topcoat::Result<impl View> {
    let Seat {
        ticket,
        ask,
        waited,
    } = seat;
    let id = ticket.id.to_string();
    let hire = ticket.hire.to_string();
    let state = ticket.state.kind();
    let k = key("tickets", &id);
    let (tone, ring) = person_look(&ticket.state);
    let (status, said, full) = match state {
        "asking" => (
            match waited {
                Some(w) => format!("Needs you · {}", ago(w)),
                None => "Needs you".to_string(),
            },
            format!("{hire} on {id} needs you"),
            String::new(),
        ),
        "held" => (
            format!("Held · {}", held_short(&ticket.state)),
            format!("{id} is held"),
            format!(
                "Held: {}",
                ui::state_detail(&ticket.state).unwrap_or_default()
            ),
        ),
        "claimed" => (
            "Starting".to_string(),
            format!("{hire} starting {id}"),
            String::new(),
        ),
        _ => (
            format!("Working · try {}", ticket.attempts + 1),
            format!("{hire} working on {id}"),
            String::new(),
        ),
    };
    let ts = match tone {
        "attn" => "text-attn-fg",
        "held" => "text-fg-3",
        _ => "text-run-fg",
    };
    Ok(view! {
        <button type="button" class=(format!("{STATION} {PICKED}")) aria-label=(said)
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(super::select(&k))>
            if let Some(ask) = ask.clone() {
                <span class="bubble" title=(ask_call(&ask))><span>(words::ask_title(&ask))</span></span>
            } else {
                <span class="mb-1 h-6 shrink-0"></span>
            }
            person(hire: hire, tone: tone, ring: ring, badge: ask.is_some())
            <span class="workdesk"><i class=(tone)></i></span>
            <span class=(TID)>(id)</span>
            <span class=(TITLE) title=(ticket.title.clone())>(ticket.title.clone())</span>
            if full.is_empty() {
                <span class=(format!("max-w-full truncate text-[11.5px] leading-4 {ts}"))>(status)</span>
            } else {
                <span class=(format!("max-w-full truncate text-[11.5px] leading-4 {ts}")) title=(full)>(status)</span>
            }
        </button>
    })
}

/// What a ticket stopped in checks or review shows: "Needs you" or
/// "Held · budget", and the class that colours it.
fn stopped(state: &TicketState) -> Option<(String, &'static str)> {
    let tone = if ui::needs_owner(state) {
        "text-attn-fg"
    } else {
        "text-fg-3"
    };
    match state.kind() {
        "asking" => Some(("Needs you".into(), tone)),
        "held" => Some((format!("Held · {}", held_short(state)), tone)),
        _ => None,
    }
}

#[component]
async fn checks_card(
    ticket: Ticket,
    command: Option<String>,
    since: Option<u64>,
    now: u64,
    sel: String,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let k = key("tickets", &id);
    let stop = stopped(&ticket.state);
    let elapsed = since
        .filter(|_| stop.is_none())
        .map(|at| ui::elapsed(now.saturating_sub(at)));
    Ok(view! {
        <button type="button" class=(format!("{CARD} {PICKED} border-run-line")) aria-label=(format!("{id} in checks"))
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
            <span class="flex items-center justify-between gap-2">
                <span class="font-mono text-[11.5px] text-fg-3">(id.clone())</span>
                if let Some(elapsed) = elapsed {
                    <span class="tabular-nums text-fg-3">(elapsed)</span>
                }
            </span>
            if let Some((words, tone)) = stop {
                <span class="truncate text-fg">(ticket.title.clone())</span>
                <span class=(tone)>(words)</span>
            } else if let Some(command) = command {
                <span class="truncate font-mono text-[12px] text-fg" title=(command.clone())>(command)</span>
            } else {
                <span class="truncate text-fg">(ticket.title.clone())</span>
                <span class="text-fg-3">"Running the project's checks"</span>
            }
        </button>
    })
}

/// A goal on the whiteboard: its tickets landed so far, or, while its
/// producer still plans it, the producer at work.
#[component]
async fn goal_row(goal: Goal, asking: bool, sel: String) -> topcoat::Result<impl View> {
    let k = key("goals", &goal.plan.id.to_string());
    let landed = goal.landed();
    let total = goal.filed.len();
    let attn = goal.needs_you();
    let name = goal.name();
    let planning = room(&goal.plan) == Room::Planning;
    let plan_id = goal.plan.id.to_string();
    let plan_key = key("tickets", &plan_id);
    let producer = goal.plan.hire.to_string();
    let plan_state = goal.plan.state.kind();
    let (tone, ring) = person_look(&goal.plan.state);
    let doing = if tone == "attn" {
        format!("Needs you, by {producer}")
    } else {
        format!("Planning, by {producer}")
    };
    let label = if planning {
        format!("{name}: {producer} is planning it")
    } else if attn {
        format!("{name}: {landed} of {total} landed, needs you")
    } else {
        format!("{name}: {landed} of {total} landed")
    };
    let row = "flex w-full min-w-0 cursor-pointer flex-col gap-1.5 rounded-md border border-transparent px-2 py-[7px] text-left hover:bg-bg-3 aria-pressed:border-line-2 aria-pressed:bg-bg-3";
    Ok(view! {
        if planning {
            <div class="flex min-w-0 items-center gap-1">
                <button type="button" class=(format!("{row} flex-1"))
                    aria-label=(label) aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
                    <span class="truncate text-[12.5px]" title=(goal.plan.goal.clone())>(name)</span>
                </button>
                <button type="button" class="flex shrink-0 cursor-pointer items-center rounded-md border border-transparent p-0.5 hover:bg-bg-3 aria-pressed:border-line-2 aria-pressed:bg-bg-3"
                    title=(doing) aria-label=(format!("{producer} planning {plan_id}")) aria-pressed=(pressed(&sel, &plan_key)) data-attr:aria-pressed=(pressed_when(&plan_key)) data-on:click=(select(&plan_key))>
                    person(hire: producer, tone: tone, ring: ring, badge: asking, small: true)
                </button>
            </div>
        } else {
            <button type="button" class=(row)
                aria-label=(label) aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
                <span class="flex justify-between gap-2 text-[12.5px]">
                    <span class="truncate" title=(goal.plan.goal.clone())>(name)</span>
                    if total > 0 {
                        <span class=(if attn { "flex shrink-0 items-center gap-1 tabular-nums text-attn-fg" } else { "shrink-0 tabular-nums text-fg-3" })>
                            if attn {
                                <svg class="st attn sm" aria-hidden="true"><use href="#s-attn"></use></svg>
                            }
                            (landed) "/" (total)
                        </span>
                    } else {
                        <span class="shrink-0 text-fg-3">(ui::look(plan_state).label)</span>
                    }
                </span>
                if total > 0 {
                    ui::meter(value: landed as f64, max: total as f64, tone: if attn { "attn" } else { "ok" })
                }
            </button>
        }
    })
}

#[component]
async fn review_seat(
    ticket: Ticket,
    judge: Option<Judge>,
    doing: Option<String>,
    sel: String,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let k = key("tickets", &id);
    let doing = judge.as_ref().map(|j| {
        doing.unwrap_or_else(|| match &j.gate {
            Some(gate) => format!("The {gate} critic is reading"),
            None => format!("{} is reading", j.hire),
        })
    });
    let who = judge.as_ref().map(|j| j.hire.clone());
    let label = match &who {
        Some(hire) => format!("{hire} reviewing {id}"),
        None => format!("{id} in review"),
    };
    let stop = stopped(&ticket.state);
    let (tone, ring) = match &stop {
        Some(_) => person_look(&ticket.state),
        None => ("review", 85),
    };
    let (status, colour) = match stop {
        Some((words, tone)) => (words, tone),
        None => (
            doing.unwrap_or_else(|| "In review".to_string()),
            "text-[var(--h-violet)]",
        ),
    };
    Ok(view! {
        <button type="button" class=(format!("{STATION} {PICKED} max-w-[220px]")) aria-label=(label)
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
            if let Some(hire) = who {
                person(hire: hire, tone: tone, ring: ring, badge: ticket.state.kind() == "asking")
            } else {
                <span class="seat"></span>
            }
            <span class="roundtable"><i></i></span>
            <span class=(TID)>(id)</span>
            <span class=(TITLE) title=(ticket.title.clone())>(ticket.title.clone())</span>
            <span class=(format!("max-w-full truncate text-[11.5px] leading-4 {colour}")) title=(status.clone())>
                (status)
            </span>
        </button>
    })
}

#[component]
async fn ready_card(
    ticket: Ticket,
    size: Option<(u64, u64)>,
    spent: f64,
    sel: String,
) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let k = key("tickets", &id);
    let spent = ui::usd(spent);
    let land = format!("@post('/tickets/{id}/land')");
    Ok(view! {
        <div class="flex min-w-0 flex-col gap-1.5 rounded-[9px] border border-ok-line bg-bg px-3 py-2.5 text-[12.5px] transition-colors has-[[aria-pressed=true]]:border-ok has-[[aria-pressed=true]]:bg-bg-2">
            <button type="button" class="flex min-w-0 cursor-pointer flex-col gap-1.5 text-left"
                aria-label=(format!("{id} ready to land")) aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
                <span class="flex items-center justify-between gap-2">
                    <span class="font-mono text-[11.5px] text-fg-3">(id.clone())</span>
                    if let Some((added, removed)) = size {
                        ui::diff_size(added: added, removed: removed)
                    }
                </span>
                <span class="truncate font-medium" title=(ticket.title.clone())>(ticket.title.clone())</span>
            </button>
            <span class="flex items-center justify-between gap-2">
                <span class="tabular-nums text-fg-3">(spent)</span>
                <button type="button" class=(ui::PRIMARY_SM) data-on:click=(land) aria-label=(format!("Land {id}"))>
                    ui::icon(name: "git-merge", small: true)
                    "Land"
                </button>
            </span>
        </div>
    })
}

#[component]
async fn rester(
    member: client::Member,
    idle: Option<String>,
    sel: String,
) -> topcoat::Result<impl View> {
    let k = key("hires", &member.id);
    let shown = match &idle {
        Some(idle) => format!("{} · {idle}", member.id),
        None => member.id.clone(),
    };
    let label = match &idle {
        Some(idle) => format!("{}, resting for {idle}", member.id),
        None => format!("{}, resting", member.id),
    };
    Ok(view! {
        <button type="button" class=(format!("{SITTER} {PICKED}")) aria-label=(label)
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
            person(hire: member.id.clone(), tone: "idle", ring: 0)
            <span class="max-w-full truncate text-[11.5px] leading-4 text-fg-3">(shown)</span>
        </button>
    })
}

#[component]
async fn front_hire(
    member: client::Member,
    sel: String,
    day: Option<HireDay>,
) -> topcoat::Result<impl View> {
    let k = key("hires", &member.id);
    // What it did today, when studiod says: calls decided and sent to the
    // owner, or tickets routed.
    let calls = day
        .as_ref()
        .map(|d| (d.allowed + d.denied + d.to_owner, d.to_owner));
    let routed = day.as_ref().map(|d| d.routed);
    let caption = "max-w-full truncate text-[11.5px] leading-4 text-fg-3";
    Ok(view! {
        <button type="button" class=(format!("{SITTER} {PICKED}")) aria-label=(format!("{}, the {}", member.id, member.role))
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
            person(hire: member.id.clone(), tone: "plain", ring: 0)
            match (member.role.as_str(), calls, routed) {
                ("approver", Some((decided, sent)), _) => {
                    <span class="flex max-w-full flex-col items-center" title=(member.id.clone())>
                        <span class=(caption)><b class="font-medium text-fg tabular-nums">(decided)</b> " decided"</span>
                        <span class=(caption)><b class="font-medium text-attn-fg tabular-nums">(sent)</b> " to you"</span>
                    </span>
                }
                ("router", _, Some(routed)) => {
                    <span class=(caption) title=(member.id.clone())>
                        <b class="font-medium text-fg tabular-nums">(routed)</b> " routed"
                    </span>
                }
                _ => <span class=(caption)>(member.id)</span>,
            }
        </button>
    })
}

#[component]
async fn queued_card(ticket: Ticket, sel: String) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let k = key("tickets", &id);
    Ok(view! {
        <button type="button" class=(format!("h-7 shrink-0 cursor-pointer rounded-[7px] border border-dashed border-line-3 px-2.5 font-mono text-[11.5px] font-medium leading-none text-fg-2 hover:border-solid hover:bg-bg-2 hover:text-fg aria-pressed:border-solid aria-pressed:text-fg {PICKED}"))
            title=(ticket.title.clone()) aria-label=(format!("{id}, up next: {}", truncate(&ticket.title, 80)))
            aria-pressed=(pressed(&sel, &k)) data-attr:aria-pressed=(pressed_when(&k)) data-on:click=(select(&k))>
            (id)
        </button>
    })
}

/// A finished ticket, on a row of the Shipped room's grid. The cost drops
/// out where the room is narrow; the ticket's page has it.
#[component]
async fn shipped_row(ticket: Ticket, spent: f64) -> topcoat::Result<impl View> {
    let id = ticket.id.to_string();
    let state = ticket.state.kind();
    let quiet = if state == "cancelled" {
        "truncate text-fg-3"
    } else {
        "truncate"
    };
    Ok(view! {
        <a href=(format!("/tickets/{id}")) class="col-span-full grid min-h-[30px] grid-cols-subgrid items-center rounded-md px-1.5 text-[12.5px] hover:bg-bg-2"
            aria-label=(format!("{id} {}: {}", ui::look(state).label.to_lowercase(), ticket.title))>
            ui::state_icon(state: state)
            <span class="font-mono text-[11.5px] text-fg-3">(id.clone())</span>
            <span class=(quiet) title=(ticket.title.clone())>(ticket.title.clone())</span>
            <span class="tabular-nums text-fg-3 @max-[200px]:hidden">(ui::usd(spent))</span>
        </a>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn ticket(id: &str, state: Value, extra: Value) -> Ticket {
        let mut t = json!({
            "id": id, "project": "game", "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
            t.extend(extra.clone());
        }
        serde_json::from_value(t).unwrap()
    }

    #[test]
    fn each_state_has_its_room() {
        let work = |state: Value| room(&ticket("G-1", state, json!({})));
        assert_eq!(work(json!("queued")), Room::UpNext);
        assert_eq!(work(json!("claimed")), Room::Desk);
        assert_eq!(work(json!("working")), Room::Desk);
        assert_eq!(work(json!({"asking": {"ask": "a1"}})), Room::Desk);
        assert_eq!(work(json!({"held": {"reason": "budget"}})), Room::Desk);
        assert_eq!(work(json!("checking")), Room::Checks);
        assert_eq!(work(json!("reviewing")), Room::Review);
        assert_eq!(work(json!("ready")), Room::Ready);
        assert_eq!(work(json!("landed")), Room::Done);
        assert_eq!(work(json!("cancelled")), Room::Done);
        assert_eq!(
            work(json!({"failed": {"class": "checks", "detail": "x"}})),
            Room::Done
        );
        // A producer plans at the table, not at a desk.
        let plan = |state: Value| room(&ticket("G-2", state, json!({"kind": "plan"})));
        assert_eq!(plan(json!("working")), Room::Planning);
        assert_eq!(plan(json!({"asking": {"ask": "a2"}})), Room::Planning);
        assert_eq!(plan(json!("queued")), Room::UpNext);
        // Asking or held, a ticket stays where it stopped.
        let stopped =
            |state: Value, from: &str| room(&ticket("G-3", state, json!({"held_from": from})));
        assert_eq!(
            stopped(json!({"asking": {"ask": "a3"}}), "reviewing"),
            Room::Review
        );
        assert_eq!(
            stopped(json!({"held": {"reason": "budget"}}), "checking"),
            Room::Checks
        );
        assert_eq!(
            stopped(json!({"held": {"reason": "paused"}}), "working"),
            Room::Desk
        );
    }

    fn status(max_runs: u64, running: u64) -> client::Status {
        serde_json::from_value(json!({"max_runs": max_runs, "running": running})).unwrap()
    }

    #[test]
    fn a_desk_is_free_for_each_slot_no_run_holds() {
        assert_eq!(free_desks(&status(6, 3)), 3);
        // More runs than slots, as when the limit was just lowered.
        assert_eq!(free_desks(&status(2, 3)), 0);
        assert_eq!(free_desks(&status(0, 0)), 0);
    }

    async fn render(office: Office, sel: &str) -> String {
        let cx = topcoat::context::Cx::default();
        ui::html(
            &cx,
            view! { cx => floor(office: office, sel: sel.to_string()) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn checks_and_review_hold_their_slots() {
        // Both slots are taken by runs that are not at a desk.
        let office = Office {
            status: status(2, 2),
            tickets: vec![
                ticket("G-1", json!("checking"), json!({})),
                ticket("G-2", json!("reviewing"), json!({})),
                ticket("G-3", json!("queued"), json!({})),
            ],
            ..Office::default()
        };
        let html = render(office, "").await;
        assert!(!html.contains(">Free desk<"), "{html}");
        // A held ticket keeps its desk beside the free one; it holds no slot.
        let office = Office {
            status: status(2, 1),
            tickets: vec![
                ticket("G-1", json!("working"), json!({})),
                ticket("G-2", json!({"held": {"reason": "budget"}}), json!({})),
            ],
            ..Office::default()
        };
        let html = render(office, "").await;
        assert_eq!(html.matches(">Free desk<").count(), 1, "{html}");
        assert!(html.contains(">Held · budget<"), "{html}");
        assert!(html.contains(r#"title="Held: out of budget""#), "{html}");
    }

    #[tokio::test]
    async fn the_floor_has_a_desk_per_slot_and_escapes_what_it_shows() {
        let office = Office {
            status: status(4, 3),
            tickets: vec![
                ticket(
                    "G-1",
                    json!({"asking": {"ask": "a1"}}),
                    json!({"title": "Waves <i>now</i>"}),
                ),
                ticket("G-2", json!("working"), json!({})),
                ticket("G-3", json!("checking"), json!({})),
            ],
            asks: vec![AskView {
                id: "a1".into(),
                ticket: "G-1".into(),
                hire: "dev".into(),
                kind: "permission".into(),
                tool: "Bash".into(),
                input: json!({"command": "rm -rf \"<x>\""}),
                reasons: Vec::new(),
                at_ms: 0,
            }],
            ..Office::default()
        };
        let html = render(office, "tickets/G-2").await;
        assert_eq!(html.matches(">Free desk<").count(), 1, "{html}");
        assert!(html.contains("Waves &lt;i&gt;now&lt;/i&gt;"), "{html}");
        // Quotes can't end the tooltip early.
        assert!(
            html.contains("title=\"Bash: rm -rf &quot;<x>&quot;\""),
            "{html}"
        );
        assert!(!html.contains(">Waves <i>"), "{html}");
        // The picked desk is pressed; the others follow the signal.
        assert_eq!(html.matches(r#"aria-pressed="true""#).count(), 1, "{html}");
        assert!(html.contains("$sel == 'tickets/G-1'"), "{html}");
        assert!(html.contains("G-3 in checks"), "{html}");
    }

    #[test]
    fn checks_are_read_from_the_current_run() {
        let log = [
            r#"{"at":10,"note":"check: cargo fmt --check"}"#,
            r#"{"at":50,"note":"check: cargo fmt --check"}"#,
            r#"{"at":54,"note":"check: cargo test"}"#,
        ]
        .join("\n");
        assert_eq!(
            checks_since(&log, 40),
            [
                ("cargo fmt --check".to_string(), 50),
                ("cargo test".to_string(), 54)
            ]
        );
    }

    #[test]
    fn asks_name_their_kind_and_the_exact_call() {
        let ask = |kind: &str, tool: &str, input: Value| AskView {
            id: "a1".into(),
            ticket: "G-1".into(),
            hire: "dev".into(),
            kind: kind.into(),
            tool: tool.into(),
            input,
            reasons: Vec::new(),
            at_ms: 0,
        };
        let run = ask(
            "permission",
            "Bash",
            json!({"command": "cargo run --example waves"}),
        );
        assert_eq!(words::ask_title(&run), "Run a command");
        assert_eq!(ask_call(&run), "Bash: cargo run --example waves");
        let edit = ask("permission", "Edit", json!({"file_path": "src/a.rs"}));
        assert_eq!(words::ask_title(&edit), "Edit a file");
        let question = ask(
            "question",
            "AskUserQuestion",
            json!([{"question": "Which palette?", "options": ["a"]}]),
        );
        assert_eq!(
            (words::ask_title(&question), ask_call(&question)),
            ("Ask a question".to_string(), "Which palette?".to_string())
        );
    }
}
