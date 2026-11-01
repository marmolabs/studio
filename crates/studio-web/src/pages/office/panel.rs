//! The office's side panel: what needs you, every project, the team, money,
//! rules and the log as tabs, and the panel of whatever the owner picked on
//! the floor.

use serde::Deserialize;
use std::collections::BTreeMap;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use studiod::api::{DaySpend, HireDay, ProjectCheck, Request, RuleFile, Trophy};
use topcoat::context::Cx;
use topcoat::view::{BoxView, Child, View, ViewExt, component, view};

use super::floor::{self, Room, room};
use super::inspector::{self, Detail};
use super::model::{Element, Role, TINTS};
use super::{Office, ago, key, safe, select};
use crate::pages::timeline::relative;
use crate::summary::truncate;
use crate::{AppConfig, client, ui};

/// The most desks the office can have.
const MOST_DESKS: u64 = studiod::api::MAX_RUNS_LIMIT;
/// The Log tab lists this many events.
const LOG_SHOWN: usize = 50;
/// Landed tickets that make a hire reach level 2, 3 and so on up to 10.
const LEVELS: [usize; 9] = [2, 5, 10, 20, 35, 55, 80, 110, 150];

/// What the panel reads from studiod beyond what the office gathers. Each
/// read that fails is left empty, and the panel then shows nothing for it.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    pub budget: Option<Budget>,
    pub rules: Vec<RuleFile>,
    pub trophies: Vec<Trophy>,
    pub checks: Vec<ProjectCheck>,
    /// Spend on each of the last 14 days, oldest first.
    pub days: Vec<DaySpend>,
}

/// The parts of `Request::Budget` the panel shows.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub spent_24h: Spent,
    /// What live runs hold, by ticket.
    #[serde(default)]
    pub reserved: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Limits {
    /// What one ticket may spend.
    pub ticket_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Spent {
    #[serde(default)]
    pub by_hire: BTreeMap<String, f64>,
    #[serde(default)]
    pub by_ticket: BTreeMap<String, f64>,
}

impl Budget {
    /// What live runs hold.
    pub fn held(&self) -> f64 {
        self.reserved.values().sum()
    }
}

impl Extra {
    /// Reads it; anything studiod can't say right now is left empty.
    pub async fn gather(config: &AppConfig) -> Extra {
        let socket = &config.socket;
        let (budget, rules, trophies, checks, days) = tokio::join!(
            client::call::<Budget>(socket, Request::Budget),
            client::call::<Vec<RuleFile>>(socket, Request::RuleFiles),
            client::call::<Vec<Trophy>>(socket, Request::Trophies),
            client::call::<Vec<ProjectCheck>>(socket, Request::ProjectChecks),
            client::call::<Vec<DaySpend>>(socket, Request::SpendByDay { days: 14 }),
        );
        Extra {
            budget: budget.ok(),
            rules: rules.unwrap_or_default(),
            trophies: trophies.unwrap_or_default(),
            checks: checks.unwrap_or_default(),
            days: days.unwrap_or_default(),
        }
    }
}

/// The panel: the tabs when `sel` is empty, with `tab` shown, else the
/// picked thing's panel.
#[component]
pub async fn panel(
    office: Office,
    detail: Detail,
    extra: Extra,
    sel: String,
    tab: String,
) -> topcoat::Result<impl View> {
    let element = sel.strip_prefix("elements/").and_then(Element::from_key);
    // Boxed, so the panels' futures live on the heap, not the stack.
    let body: BoxView = if let Some(element) = element {
        let inner = element_view(__cx, element, office, extra);
        view! { <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">(inner)</div> }
            .boxed()
    } else if sel.is_empty() || !inspector::finds(&office, &sel) {
        view! {
            <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">
                tabs(office: office, extra: extra, tab: tab)
            </div>
        }
        .boxed()
    } else {
        view! { inspector::inspector(office: office, detail: detail, sel: sel) }.boxed()
    };
    Ok(view! { (body) })
}

// ---------------------------------------------------------------- shared

/// A project's tint: its place in the list of projects.
fn tint(office: &Office, project: &str) -> usize {
    office
        .status
        .projects
        .iter()
        .position(|p| p == project)
        .unwrap_or(0)
        % TINTS
}

/// The class of a project's colour dot.
fn dot_class(index: usize) -> String {
    format!("pjd pj-{}", index % TINTS)
}

/// A skin and a hair tone, from a name, as the floor gives its figures.
fn looks(seed: &str) -> (char, char) {
    let h = seed
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let pick = |n: u32| "abcd".chars().nth((n % 4) as usize).unwrap_or('a');
    (pick(h), pick(h / 4))
}

/// A hire's classes for its portrait: shirt, skin and hair.
fn portrait_class(hire: &str, role: &str) -> String {
    let (skin, hair) = looks(hire);
    format!("{} s-{skin} h-{hair}", Role::of(role).class())
}

/// "Engineer" for `engineer`.
fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A share of a whole, in percent, from 0 to 100.
fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 && part > 0.0 {
        (part / whole * 100.0).min(100.0)
    } else {
        0.0
    }
}

/// A heading over a block of the panel, with a quiet note at its right.
#[component]
async fn section(
    label: &'static str,
    #[into]
    #[default]
    note: String,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <section class="sec">
            <div class="sec-h"><span>(label)</span><span>(note)</span></div>
            (child)
        </section>
    })
}

/// A quiet sentence.
#[component]
async fn quiet(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! { <p class="m-0 text-[13px] leading-5 text-fg-3">(text)</p> })
}

/// A ticket in a list: its project's dot, id, title and a note. Picking it
/// opens its panel.
#[component]
async fn line(
    id: String,
    title: String,
    tint: usize,
    #[into]
    #[default]
    note: String,
) -> topcoat::Result<impl View> {
    let pick = select(&key("tickets", &id));
    Ok(view! {
        <button type="button" class="grid w-full min-w-0 cursor-pointer grid-cols-[9px_58px_minmax(0,1fr)_auto] items-center gap-2 rounded-[7px] border-0 bg-transparent px-1 py-[5px] text-left text-[12.5px] hover:bg-bg-4" data-on:click=(pick)>
            <i class=(dot_class(tint))></i>
            <span class="font-mono text-[11px] text-fg-3">(id)</span>
            <span class="truncate text-fg">(title)</span>
            <span class="whitespace-nowrap text-[11.5px] text-fg-3">(note)</span>
        </button>
    })
}

// ------------------------------------------------------------------ tabs

/// The tab names and their words, in order.
pub const TABS: [(&str, &str); 6] = [
    ("needs", "Needs you"),
    ("projects", "Projects"),
    ("team", "Team"),
    ("money", "Money"),
    ("rules", "Rules"),
    ("log", "Log"),
];

/// How many things wait for the owner: open asks and tickets held for them.
fn needs_count(office: &Office) -> usize {
    office.asks.len()
        + office
            .tickets
            .iter()
            .filter(|t| t.state.kind() == "held" && ui::needs_owner(&t.state))
            .count()
}

/// `name` if it is a tab, else the first tab.
pub fn tab(name: &str) -> &'static str {
    TABS.iter()
        .map(|(tab, _)| *tab)
        .find(|tab| *tab == name)
        .unwrap_or(TABS[0].0)
}

/// The tab bar and its panes, switched by `$tab` without asking the server;
/// `tab` is the one shown before Datastar runs.
#[component]
async fn tabs(office: Office, extra: Extra, tab: String) -> topcoat::Result<impl View> {
    let shown = tab.clone();
    let hidden = move |name: &str| (name != shown).then_some("display: none");
    let needs = needs_count(&office);
    let rules = extra.rules.clone();
    let money = (extra.clone(), office.clone());
    Ok(view! {
        <nav class="tabs" role="tablist" aria-label="The studio">
            for (name, label) in TABS {
                <button type="button" class="tab" role="tab" aria-selected=(if name == tab { "true" } else { "false" })
                    data-attr:aria-selected=(format!("$tab == '{name}' ? 'true' : 'false'")) data-on:click=(format!("$tab = '{name}'"))>
                    (label)
                    if name == "needs" && needs > 0 {
                        <span class="count needs">(needs)</span>
                    }
                </button>
            }
        </nav>
        <div class="pane" role="tabpanel" style=(hidden("needs")) data-show="$tab == 'needs'">
            inspector::needs_you(office: office.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("projects")) data-show="$tab == 'projects'">
            projects_pane(office: office.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("team")) data-show="$tab == 'team'">
            team_pane(office: office.clone(), extra: extra.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("money")) data-show="$tab == 'money'">
            money_pane(office: money.1, extra: money.0)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("rules")) data-show="$tab == 'rules'">
            rules_pane(rules: rules)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("log")) data-show="$tab == 'log'">
            log_pane(office: office)
        </div>
    })
}

// -------------------------------------------------------------- projects

/// A ticket as a task row.
#[derive(Clone, Debug, PartialEq)]
struct Task {
    id: String,
    title: String,
    /// The tick's modifier class.
    tick: &'static str,
    /// The state in plain words.
    word: &'static str,
    /// The word's colour.
    tone: &'static str,
}

impl Task {
    fn of(ticket: &Ticket) -> Task {
        let (tick, word, tone) = match ticket.state.kind() {
            "landed" => ("done", "landed", "text-ok-fg"),
            "failed" => ("bad", "failed", "text-bad-fg"),
            "cancelled" => ("held", "cancelled", "text-fg-3"),
            "asking" => ("needs", "needs you", "text-attn-fg"),
            "held" if ui::needs_owner(&ticket.state) => ("needs", "needs you", "text-attn-fg"),
            "held" => ("held", "on hold", "text-fg-3"),
            "claimed" | "working" => ("now", "working", "text-run-fg"),
            "checking" => ("now", "checking", "text-run-fg"),
            "reviewing" => ("now", "in review", "text-run-fg"),
            "ready" => ("ready", "ready to land", "text-ok-fg"),
            _ => ("", "up next", "text-fg-3"),
        };
        Task {
            id: ticket.id.to_string(),
            title: ticket.title.clone(),
            tick,
            word,
            tone,
        }
    }
}

/// An open goal in a project.
#[derive(Clone, Debug, PartialEq)]
struct GoalRow {
    key: String,
    name: String,
    landed: usize,
    total: usize,
    done: bool,
    /// The merge request and the branch it merges into, for a done goal.
    merge: Option<(String, String)>,
    tasks: Vec<Task>,
}

#[derive(Clone, Debug, PartialEq)]
struct ProjectRow {
    name: String,
    tint: usize,
    needs: usize,
    working: usize,
    goals: Vec<GoalRow>,
    /// Tickets without a goal that are up next, in progress or finished
    /// lately.
    loose: Vec<Task>,
}

/// Every project, in the order of the status.
fn project_rows(office: &Office) -> Vec<ProjectRow> {
    let goals = floor::goals(office);
    let shipped = floor::shipped(office);
    office
        .status
        .projects
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let own: Vec<&Ticket> = office
                .tickets
                .iter()
                .filter(|t| t.project.to_string() == *name)
                .collect();
            let needs = own
                .iter()
                .filter(|t| {
                    t.state.kind() == "asking"
                        || (t.state.kind() == "held" && ui::needs_owner(&t.state))
                })
                .count();
            let working = own
                .iter()
                .filter(|t| {
                    matches!(
                        t.state.kind(),
                        "claimed" | "working" | "checking" | "reviewing"
                    )
                })
                .count();
            let rows = goals
                .iter()
                .filter(|g| g.plan.project.to_string() == *name)
                .map(|g| {
                    let done = g.state() == "done";
                    let merge = done
                        .then(|| g.filed.iter().find(|t| t.state.kind() == "landed"))
                        .flatten()
                        .map(|t| {
                            (
                                format!("@post('/tickets/{}/merge')", t.id),
                                t.base_branch.clone(),
                            )
                        });
                    GoalRow {
                        key: key("goals", &g.plan.id.to_string()),
                        name: g.name(),
                        landed: g.landed(),
                        total: g.filed.len(),
                        done,
                        merge,
                        tasks: g.filed.iter().map(Task::of).collect(),
                    }
                })
                .collect();
            let loose = own
                .iter()
                .filter(|t| t.kind == TicketKind::Work && t.parent.is_none())
                .filter(|t| {
                    let kind = t.state.kind();
                    !matches!(kind, "landed" | "failed" | "cancelled")
                        || shipped.iter().any(|(s, _)| s.id == t.id)
                })
                .map(|t| Task::of(t))
                .collect();
            ProjectRow {
                name: name.clone(),
                tint: i % TINTS,
                needs,
                working,
                goals: rows,
                loose,
            }
        })
        .collect()
}

#[component]
async fn projects_pane(office: Office) -> topcoat::Result<impl View> {
    let rows = project_rows(&office);
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "No projects yet.")
        }
        for row in rows {
            project_block(row: row)
        }
    })
}

#[component]
async fn project_block(row: ProjectRow) -> topcoat::Result<impl View> {
    let id = format!("proj-{}", safe(&row.name));
    let nothing = row.goals.is_empty() && row.loose.is_empty();
    Ok(view! {
        <details id=(id) class="proj" data-preserve-attr="open" open="">
            <summary>
                <i class=(dot_class(row.tint))></i>
                <span class="nm"><b>(row.name.clone())</b></span>
                <span class="pcounts">
                    if row.needs > 0 {
                        <span class="flag needs" title="Need you">(row.needs)</span>
                    }
                    if row.working > 0 {
                        <span class="flag run" title="Working">(row.working)</span>
                    }
                </span>
            </summary>
            <div class="proj-body">
                if nothing {
                    quiet(text: "Nothing in progress.")
                }
                for goal in row.goals {
                    goal_block(goal: goal)
                }
                if !row.loose.is_empty() {
                    <div class="goal loose">
                        <div class="g-h"><b>"Without a goal"</b></div>
                        <ul class="tasks m-0 list-none p-0">
                            for task in row.loose {
                                task_row(task: task)
                            }
                        </ul>
                    </div>
                }
            </div>
        </details>
    })
}

#[component]
async fn goal_block(goal: GoalRow) -> topcoat::Result<impl View> {
    let pick = select(&goal.key);
    let width = share(goal.landed as f64, goal.total as f64);
    let empty = goal.tasks.is_empty();
    Ok(view! {
        <div class="goal">
            <div class="g-h">
                <b><button type="button" class="max-w-full cursor-pointer truncate border-0 bg-transparent p-0 text-left font-[inherit] text-[13px] font-semibold text-fg hover:underline" data-on:click=(pick)>(goal.name.clone())</button></b>
                <span>(goal.landed) " of " (goal.total)</span>
            </div>
            <div class=(if goal.done { "xp done" } else { "xp" })><i style=(format!("width: {width:.0}%"))></i></div>
            if goal.done {
                <div class="btns">
                    <span class="chip ok">"Done"</span>
                    if let Some((post, base)) = goal.merge {
                        <button type="button" class=(format!("{} push", ui::BUTTON_SM)) data-on:click=(post)>
                            ui::icon(name: "git-merge", small: true)
                            "Merge into " (base)
                        </button>
                    }
                </div>
            } else if empty {
                <p class="why">"The producer has not filed tickets yet."</p>
            } else {
                <ul class="tasks m-0 list-none p-0">
                    for task in goal.tasks {
                        task_row(task: task)
                    }
                </ul>
            }
        </div>
    })
}

#[component]
async fn task_row(task: Task) -> topcoat::Result<impl View> {
    let pick = select(&key("tickets", &task.id));
    Ok(view! {
        <li>
            <button type="button" class="task w-full border-0 bg-transparent text-left" data-on:click=(pick)>
                <span class=(format!("tick {}", task.tick))></span>
                <span class="mono font-mono">(task.id.clone())</span>
                <span class="t">(task.title.clone())</span>
                <span class=(format!("whitespace-nowrap text-[11.5px] {}", task.tone))>(task.word)</span>
            </button>
        </li>
    })
}

// ------------------------------------------------------------------ team

/// The level for `landed` tickets, and how far through it, as (landed since
/// this level began, landed this level takes); `None` at the top level.
fn level(landed: usize) -> (usize, Option<(usize, usize)>) {
    let level = 1 + LEVELS.iter().filter(|&&need| landed >= need).count();
    let from = if level == 1 { 0 } else { LEVELS[level - 2] };
    let next = LEVELS.get(level - 1).map(|&to| (landed - from, to - from));
    (level, next)
}

/// One hire in the team list.
#[derive(Clone, Debug, PartialEq)]
struct Member {
    id: String,
    look: String,
    /// Level and the share of the way to the next, for those who level up.
    level: Option<(usize, f64)>,
    line: String,
    /// Today's figures: the number and what it counts.
    figures: Vec<(String, &'static str)>,
}

/// What a hire is doing: the ticket at its desk or table, or the one it is
/// judging in review.
fn doing_now(office: &Office, hire: &str) -> Option<String> {
    let own = office
        .tickets
        .iter()
        .filter(|t| t.hire.to_string() == hire)
        .find(|t| matches!(room(t), Room::Desk | Room::Planning));
    if let Some(t) = own {
        let id = t.id.to_string();
        return Some(match t.state.kind() {
            "asking" => format!("waiting for you on {id}"),
            "held" => format!("{id} on hold"),
            _ if room(t) == Room::Planning => format!("planning {id}"),
            _ => format!("on {id}"),
        });
    }
    office
        .tickets
        .iter()
        .filter(|t| room(t) == Room::Review)
        .find_map(|t| {
            let judge = office.judges.get(&t.id.to_string())?;
            (judge.hire == hire).then(|| {
                let verb = if judge.gate.is_some() {
                    "judging"
                } else {
                    "reading"
                };
                format!("{verb} {}", t.id)
            })
        })
}

/// Today's numbers for a hire, by what its role does.
fn figures(role: &str, day: &HireDay, spent: Option<f64>) -> Vec<(String, &'static str)> {
    let money = |spent: Option<f64>| spent.map(|usd| (ui::usd(usd), "spent"));
    let decided = day.allowed + day.denied + day.to_owner;
    let mut out: Vec<(String, &'static str)> = match role {
        "reviewer" => vec![(day.reviews.to_string(), "reviewed")],
        "critic" => vec![(day.critiques.to_string(), "judged")],
        "producer" => vec![(day.planned.to_string(), "planned")],
        "approver" => vec![
            (decided.to_string(), "decided"),
            (day.to_owner.to_string(), "to you"),
        ],
        "router" => vec![(day.routed.to_string(), "routed")],
        _ => vec![(day.done.to_string(), "done")],
    };
    if role != "approver" {
        out.extend(money(spent));
    }
    out
}

/// The team in three groups: at work, resting and at the front desk.
fn members(office: &Office, extra: &Extra) -> [Vec<Member>; 3] {
    let mut groups: [Vec<Member>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for m in &office.status.team {
        let front = floor::FRONT.contains(&m.role.as_str());
        // Reviewers and critics judge work rather than land it, so a level
        // from landed tickets would say nothing about them.
        let levelled = !front && !matches!(m.role.as_str(), "reviewer" | "critic");
        let landed = office
            .tickets
            .iter()
            .filter(|t| t.hire.to_string() == m.id && t.state.kind() == "landed")
            .count();
        let (lv, next) = level(landed);
        let now = doing_now(office, &m.id);
        let doing = match (&now, front) {
            (Some(now), _) => now.clone(),
            (None, true) => String::new(),
            (None, false) => match floor::idle_since(office, &m.id) {
                Some(at) => format!("resting {}", ago(office.now.saturating_sub(at))),
                None => "resting".into(),
            },
        };
        let about = [capital(&m.role), m.model.clone(), doing]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let spent = extra
            .budget
            .as_ref()
            .map(|b| b.spent_24h.by_hire.get(&m.id).copied().unwrap_or(0.0));
        let day = office.today.get(&m.id).cloned().unwrap_or_default();
        let member = Member {
            id: m.id.clone(),
            look: portrait_class(&m.id, &m.role),
            level: levelled.then(|| {
                (
                    lv,
                    next.map_or(100.0, |(have, need)| share(have as f64, need as f64)),
                )
            }),
            line: about,
            figures: figures(&m.role, &day, spent),
        };
        let group = if front {
            2
        } else if now.is_some() {
            0
        } else {
            1
        };
        groups[group].push(member);
    }
    groups
}

#[component]
async fn team_pane(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let [work, resting, front] = members(&office, &extra);
    let none = work.is_empty() && resting.is_empty() && front.is_empty();
    Ok(view! {
        if none {
            quiet(text: "No one is hired yet.")
        }
        if !work.is_empty() {
            section(label: "At work", note: "today",
                <div class="card">
                    for member in work { member_row(member: member) }
                </div>
            )
        }
        if !resting.is_empty() {
            section(label: "Resting",
                <div class="card">
                    for member in resting { member_row(member: member) }
                </div>
            )
        }
        if !front.is_empty() {
            section(label: "Front desk",
                <div class="card">
                    for member in front { member_row(member: member) }
                </div>
            )
        }
        if !none {
            <p class="why m-0">"Levels come from the tickets a hire has landed."</p>
        }
        hire_link()
    })
}

#[component]
async fn hire_link() -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <a href="/hires?new" class=(ui::BUTTON)>
                ui::icon(name: "plus")
                "Hire someone"
            </a>
        </div>
    })
}

#[component]
async fn member_row(member: Member) -> topcoat::Result<impl View> {
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
                if let Some((_, percent)) = member.level {
                    <div class="xp"><i style=(format!("width: {percent:.0}%"))></i></div>
                }
            </div>
            <div class="nums">
                for (number, what) in member.figures {
                    <div><b>(number)</b><small>(what)</small></div>
                }
            </div>
        </button>
    })
}

// ----------------------------------------------------------------- money

/// "Oct 19" for "2026-10-19".
fn short_date(date: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, rest) = date.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    let month = MONTHS.get(month.parse::<usize>().ok()?.checked_sub(1)?)?;
    Some(format!("{month} {}", day.parse::<u32>().ok()?))
}

/// One day's bar.
#[derive(Clone, Debug, PartialEq)]
struct Bar {
    label: String,
    usd: f64,
    /// Its height in percent of the chart.
    height: f64,
}

/// The days as bars, and where the daily limit sits in the chart. The chart
/// is as tall as the biggest day or the limit.
fn bars(days: &[DaySpend], daily: f64) -> (Vec<Bar>, Option<f64>) {
    let top = days.iter().map(|d| d.usd).fold(daily.max(0.0), f64::max);
    let last = days.len().saturating_sub(1);
    let bars = days
        .iter()
        .enumerate()
        .map(|(i, d)| Bar {
            label: if i == last {
                "Today".into()
            } else {
                short_date(&d.date).unwrap_or_default()
            },
            usd: d.usd,
            height: share(d.usd, top),
        })
        .collect();
    (bars, (daily > 0.0).then(|| share(daily, top)))
}

/// The spend by project, biggest first, from what each ticket cost.
fn by_project(office: &Office, budget: &Budget) -> Vec<(String, usize, f64)> {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for (id, usd) in &budget.spent_24h.by_ticket {
        if let Some(ticket) = office.ticket(id) {
            *sums.entry(ticket.project.to_string()).or_default() += usd;
        }
    }
    let mut rows: Vec<(String, usize, f64)> = sums
        .into_iter()
        .filter(|(_, usd)| *usd > 0.0)
        .map(|(name, usd)| (tint(office, &name), name, usd))
        .map(|(i, name, usd)| (name, i, usd))
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    rows
}

/// The spend by hire, biggest first.
fn by_hire(budget: &Budget) -> Vec<(String, f64)> {
    let mut rows: Vec<(String, f64)> = budget
        .spent_24h
        .by_hire
        .iter()
        .filter(|(_, usd)| **usd > 0.0)
        .map(|(hire, usd)| (hire.clone(), *usd))
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    rows
}

/// What was spent over the last 24 hours against the daily limit, with what
/// live runs hold and what is left when the budget says.
#[component]
async fn spend_summary(spent: f64, daily: f64, held: Option<f64>) -> topcoat::Result<impl View> {
    let left = held.map(|held| (daily - spent - held).max(0.0));
    let tone = if daily > 0.0 && spent / daily >= 0.9 {
        "bad"
    } else if daily > 0.0 && spent / daily >= 0.7 {
        "attn"
    } else {
        ""
    };
    Ok(view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
                <b class="text-[28px] font-bold leading-none tabular-nums">(ui::usd(spent))</b>
                <span class="text-[12.5px] text-fg-3">
                    "spent of " (ui::limit(daily))
                    if let Some(held) = held {
                        " · " (ui::usd(held)) " held by live runs"
                    }
                    if let Some(left) = left {
                        " · " (ui::usd(left)) " left"
                    }
                </span>
            </div>
            ui::meter(value: spent, max: daily, tone: tone)
        </div>
    })
}

/// A row of a split: a name, its share as a bar and the amount.
#[component]
async fn split_row(
    #[into] name: String,
    #[into] fill: String,
    percent: f64,
    #[into] amount: String,
    #[default] tint: Option<usize>,
    #[into]
    #[default]
    shirt: String,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class=(format!("grid grid-cols-[104px_minmax(0,1fr)_56px] items-center gap-2.5 text-[13px] {shirt}"))>
            <span class="flex min-w-0 items-center gap-1.5">
                if let Some(tint) = tint {
                    <i class=(dot_class(tint))></i>
                }
                <span class="truncate">(name)</span>
            </span>
            <span class="h-1.5 overflow-hidden rounded-full bg-bg-4">
                <i class="block h-full rounded-full" style=(format!("width: {percent:.1}%; background: {fill}"))></i>
            </span>
            <b class="text-right font-semibold tabular-nums">(amount)</b>
        </div>
    })
}

/// The limits, to change: the daily one and what one ticket may spend.
#[component]
async fn limits_form(daily: f64, ticket: Option<f64>) -> topcoat::Result<impl View> {
    let ticket = ticket.map(|usd| usd.to_string()).unwrap_or_default();
    Ok(view! {
        <form class="flex flex-col gap-3" data-on:submit="@post('/office/limits', {contentType: 'form'})">
            <div class="grid grid-cols-2 gap-3">
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Daily limit, USD"</span>
                    <input name="daily_usd" type="number" step="0.01" min="0.01" required="" value=(daily.to_string()) class=(ui::INPUT)>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"One ticket, USD"</span>
                    <input name="ticket_usd" type="number" step="0.01" min="0.01" value=(ticket) placeholder="Leave as is" class=(ui::INPUT)>
                </label>
            </div>
            <div><button type="submit" class=(ui::BUTTON)>"Save limits"</button></div>
        </form>
    })
}

#[component]
async fn money_pane(office: Office, extra: Extra) -> topcoat::Result<impl View> {
    let status = &office.status;
    let (spent, daily) = (status.spent_24h, status.daily_usd);
    let held = extra.budget.as_ref().map(Budget::held);
    let ticket_usd = extra.budget.as_ref().and_then(|b| b.limits.ticket_usd);
    let projects = extra
        .budget
        .as_ref()
        .map(|b| by_project(&office, b))
        .unwrap_or_default();
    let project_total: f64 = projects.iter().map(|(_, _, usd)| usd).sum();
    let hires = extra.budget.as_ref().map(by_hire).unwrap_or_default();
    let hire_total: f64 = hires.iter().map(|(_, usd)| usd).sum();
    let hires: Vec<(String, String, f64)> = hires
        .into_iter()
        .map(|(hire, usd)| {
            let role = office.member(&hire).map_or("", |m| m.role.as_str());
            (hire.clone(), Role::of(role).class().to_string(), usd)
        })
        .collect();
    let (days, limit_at) = bars(&extra.days, daily);
    let first = days.first().map(|b| b.label.clone()).unwrap_or_default();
    let limit_note = format!("daily limit {}", ui::limit(daily));
    Ok(view! {
        section(label: "Last 24 hours", note: limit_note,
            spend_summary(spent: spent, daily: daily, held: held)
        )
        if !projects.is_empty() {
            section(label: "By project",
                <div class="flex flex-col gap-2">
                    for (name, index, usd) in projects {
                        split_row(name: name, fill: format!("var(--pj-{index})"), percent: share(usd, project_total), amount: ui::usd(usd), tint: Some(index))
                    }
                </div>
            )
        }
        if !days.is_empty() {
            section(label: "14 days", note: "dashed: daily limit",
                <div class="relative flex h-24 items-end gap-1 border-b border-line">
                    if let Some(at) = limit_at {
                        <span class="pointer-events-none absolute inset-x-0 border-t border-dashed border-attn" style=(format!("bottom: {at:.1}%"))></span>
                    }
                    for bar in days {
                        <i class=(if bar.label == "Today" { "block min-h-px flex-1 rounded-t-sm bg-run" } else { "block min-h-px flex-1 rounded-t-sm bg-fg-3" })
                            style=(format!("height: {:.1}%", bar.height)) title=(format!("{} · {}", bar.label, ui::usd(bar.usd)))></i>
                    }
                </div>
                <div class="flex justify-between text-[11.5px] text-fg-3"><span>(first)</span><span>"Today"</span></div>
            )
        }
        if !hires.is_empty() {
            section(label: "By hire",
                <div class="flex flex-col gap-2">
                    for (hire, class, usd) in hires {
                        split_row(name: hire, fill: "var(--shirt)", percent: share(usd, hire_total), amount: ui::usd(usd), shirt: class)
                    }
                </div>
            )
        }
        section(label: "Limits",
            limits_form(daily: daily, ticket: ticket_usd)
        )
    })
}

// ----------------------------------------------------------------- rules

/// A rule's kind and level, from its front matter.
#[derive(Debug, Default, Deserialize, PartialEq)]
struct RuleHead {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    level: String,
}

fn rule_head(text: &str) -> RuleHead {
    studio_config::frontmatter::split(text)
        .ok()
        .flatten()
        .and_then(|(yaml, _)| serde_yaml_ng::from_str(yaml).ok())
        .unwrap_or_default()
}

/// The first paragraph of a rule after its front matter, as one line.
fn rule_about(text: &str) -> String {
    let body = studio_config::frontmatter::split(text)
        .ok()
        .flatten()
        .map_or(text, |(_, body)| body);
    let paragraph: Vec<&str> = body
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect();
    truncate(&paragraph.join(" "), 140)
}

/// The link to a rule in its editor.
fn rule_href(rule: &RuleFile) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("file", &rule.name);
    if rule.origin != "studio" {
        query.append_pair("from", &format!("{}/{}", rule.origin, rule.owner));
    }
    format!("/rules?{}", query.finish())
}

/// The chip colour for a rule's level.
fn level_tone(level: &str) -> &'static str {
    match level {
        "allow" => "chip ok",
        "ask" | "warn" => "chip needs",
        "block" => "chip bad",
        _ => "chip",
    }
}

#[component]
async fn rules_pane(rules: Vec<RuleFile>) -> topcoat::Result<impl View> {
    Ok(view! {
        section(label: "House rules", note: "agents follow these",
            rule_cards(rules: rules)
        )
        new_rule()
    })
}

#[component]
async fn new_rule() -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <a href="/rules?new" class=(ui::BUTTON)>
                ui::icon(name: "plus")
                "New rule"
            </a>
        </div>
    })
}

#[component]
async fn rule_cards(rules: Vec<RuleFile>) -> topcoat::Result<impl View> {
    let rows: Vec<(String, RuleHead, String, String, String, bool)> = rules
        .iter()
        .map(|r| {
            let owner = match r.origin.as_str() {
                "studio" => String::new(),
                origin => format!("{origin} {}", r.owner),
            };
            (
                r.name.clone(),
                rule_head(&r.text),
                rule_about(&r.text),
                rule_href(r),
                owner,
                r.disabled,
            )
        })
        .collect();
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "No rules yet.")
        }
        for (name, head, about, href, owner, off) in rows {
            <a href=(href) class="card text-fg no-underline hover:border-line-2">
                <div class="flex flex-wrap items-center gap-1.5">
                    <b class="mr-auto min-w-0 truncate text-[13px] font-semibold">(name)</b>
                    if !head.kind.is_empty() {
                        <span class="chip">(head.kind.clone())</span>
                    }
                    if !head.level.is_empty() {
                        <span class=(level_tone(&head.level))>(head.level.clone())</span>
                    }
                    if off {
                        <span class="chip held">"off"</span>
                    }
                </div>
                if !owner.is_empty() {
                    <span class="text-[12px] text-fg-3">(owner)</span>
                }
                if !about.is_empty() {
                    <p class="m-0 text-[12.5px] leading-[18px] text-fg-2">(about)</p>
                }
            </a>
        }
    })
}

// ------------------------------------------------------------------- log

/// The dot colour for a state in the log.
fn dot_tone(state: &str) -> &'static str {
    match state {
        "working" | "checking" => "dot run",
        "ready" | "landed" => "dot ok",
        "asking" => "dot needs",
        "failed" | "cancelled" => "dot bad",
        _ => "dot held",
    }
}

#[component]
async fn log_pane(office: Office) -> topcoat::Result<impl View> {
    let now = office.now;
    let rows: Vec<(String, &'static str, Option<usize>, String, String)> = office
        .recent
        .iter()
        .rev()
        .filter(|e| !matches!(e.event, TicketEvent::RunStarted { .. }))
        .take(LOG_SHOWN)
        .map(|e| {
            let (state, said) = inspector::event_words(&e.event, &office.asks);
            let project = office
                .ticket(&e.ticket_id)
                .map(|t| tint(&office, &t.project.to_string()));
            (
                ago(now.saturating_sub(e.at_ms)),
                dot_tone(state),
                project,
                e.ticket_id.clone(),
                said,
            )
        })
        .collect();
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "Nothing has happened lately.")
        }
        <ul class="m-0 flex list-none flex-col gap-2 p-0 text-[13px] text-fg-2">
            for (when, dot, project, id, said) in rows {
                <li class="grid grid-cols-[28px_8px_9px_minmax(0,1fr)] items-baseline gap-2">
                    <time class="text-[12px] tabular-nums text-fg-3">(when)</time>
                    <span class=(dot)></span>
                    if let Some(index) = project {
                        <i class=(dot_class(index))></i>
                    } else {
                        <i></i>
                    }
                    <span class="min-w-0">
                        <a href=(format!("/tickets/{id}")) class=(inspector::TICKET_LINK)>(id.clone())</a>
                        " " (said)
                    </span>
                </li>
            }
        </ul>
    })
}

// -------------------------------------------------------------- elements

/// An element's view, under a header with its name and a way back.
fn element_view(cx: &Cx, element: Element, office: Office, extra: Extra) -> BoxView<'_> {
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

/// The ticket a judge is reading: its id, title and project tint.
type Reading = (String, String, usize);

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
            (!own.is_empty()).then(|| (name.clone(), i % TINTS, own))
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
        .map(|(i, name)| (name.clone(), i % TINTS))
        .collect();
    let note = match projects.len() {
        0 => String::new(),
        n => n.to_string(),
    };
    Ok(view! {
        quiet(text: "Pick the folder of a git repository on this computer. It gets a rack of its own in the checks lab.")
        <form class="flex flex-col gap-3" data-on:submit="@post('/office/projects', {contentType: 'form'})">
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
                        for role in super::ROLES { <option value=(role)>(capital(role))</option> }
                    </select>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Model"</span>
                    <select name="model" class=(ui::INPUT)>
                        for model in super::MODELS { <option value=(model)>(model)</option> }
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

/// Reviewers and critics, and what each is reading now.
#[component]
async fn review_view(office: Office) -> topcoat::Result<impl View> {
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
async fn crates_view(office: Office) -> topcoat::Result<impl View> {
    let window = office.window();
    let now = office.now;
    let landed: Vec<(String, String, usize, String)> = floor::shipped(&office)
        .into_iter()
        .filter(|(t, _)| t.state.kind() == "landed")
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
async fn returns_view(office: Office) -> topcoat::Result<impl View> {
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
async fn counter_view(office: Office) -> topcoat::Result<impl View> {
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
async fn queue_view(office: Office) -> topcoat::Result<impl View> {
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
async fn lounge_view(office: Office) -> topcoat::Result<impl View> {
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
    use super::*;
    use crate::summary::Judge;
    use serde_json::{Value, json};
    use studiod::api::{AskView, CheckResult};

    fn ticket(id: &str, project: &str, state: Value, extra: Value) -> Ticket {
        let mut t = json!({
            "id": id, "project": project, "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
            t.extend(extra.clone());
        }
        serde_json::from_value(t).unwrap()
    }

    const HOUR: u64 = 3_600_000;
    const NOW: u64 = 100 * HOUR;

    /// Two projects, a goal with a landed and a working ticket, a queued
    /// ticket, a failed one, a ready one, a held one and a loose one.
    fn office() -> Office {
        let status: client::Status = serde_json::from_value(json!({
            "asks": 1, "running": 2, "projects": ["shop", "api"],
            "tickets": {"working": 1, "ready": 1},
            "daily_usd": 10.0, "spent_24h": 6.4, "max_runs": 3,
            "team": [
                {"id": "dev", "role": "engineer", "model": "sonnet"},
                {"id": "reviewer", "role": "reviewer", "model": "opus"},
                {"id": "approver", "role": "approver", "model": "haiku"},
                {"id": "router", "role": "router", "model": "haiku"},
            ],
        }))
        .unwrap();
        let tickets = vec![
            ticket(
                "SHOP-1",
                "shop",
                json!("landed"),
                json!({"kind": "plan", "goal": "Checkout flow"}),
            ),
            ticket(
                "SHOP-2",
                "shop",
                json!("landed"),
                json!({"parent": "SHOP-1"}),
            ),
            ticket(
                "SHOP-3",
                "shop",
                json!("working"),
                json!({"parent": "SHOP-1"}),
            ),
            ticket(
                "SHOP-4",
                "shop",
                json!("queued"),
                json!({"parent": "SHOP-1"}),
            ),
            ticket("SHOP-5", "shop", json!("ready"), json!({})),
            ticket(
                "API-1",
                "api",
                json!({"failed": {"class": "checks", "detail": "clippy"}}),
                json!({}),
            ),
            ticket(
                "API-2",
                "api",
                json!({"held": {"reason": "paused"}}),
                json!({}),
            ),
            ticket("API-3", "api", json!("queued"), json!({})),
        ];
        let at = |ticket: &str, event: Value, ago: u64| -> client::Happened {
            serde_json::from_value(json!({
                "ticket_id": ticket, "at_ms": NOW - ago, "event": event,
            }))
            .unwrap()
        };
        let mut today = BTreeMap::new();
        today.insert(
            "approver".to_string(),
            HireDay {
                allowed: 7,
                denied: 2,
                to_owner: 1,
                ..HireDay::default()
            },
        );
        let mut spend = BTreeMap::new();
        spend.insert("SHOP-3".to_string(), 1.5);
        let ask = AskView {
            id: "a1".into(),
            ticket: "SHOP-3".into(),
            hire: "dev".into(),
            kind: "permission".into(),
            tool: "Bash".into(),
            input: json!({"command": "cargo run -- --port 8080"}),
            reasons: vec![],
            at_ms: NOW - 60_000,
        };
        Office {
            status,
            tickets,
            asks: vec![ask],
            recent: vec![
                at("SHOP-2", json!({"type": "landed"}), 2 * HOUR),
                at(
                    "API-1",
                    json!({"type": "failed", "class": "checks", "detail": "clippy"}),
                    HOUR,
                ),
            ],
            today,
            spend,
            now: NOW,
            ..Office::default()
        }
    }

    fn budget() -> Budget {
        serde_json::from_value(json!({
            "limits": {"daily_usd": 10.0, "ticket_usd": 2.5, "run_usd": null},
            "spent_24h": {
                "total": 6.4,
                "by_hire": {"dev": 4.4, "reviewer": 0.5},
                "by_ticket": {"SHOP-3": 3.9, "API-1": 1.1},
            },
            "reserved": {"SHOP-3": 0.72},
        }))
        .unwrap()
    }

    fn extra() -> Extra {
        let days = (0..14)
            .map(|i| DaySpend {
                date: format!("2026-10-{:02}", 19 + i),
                start_ms: 0,
                usd: if i == 13 { 6.4 } else { f64::from(i) * 0.5 },
            })
            .collect();
        Extra {
            budget: Some(budget()),
            rules: vec![RuleFile {
                origin: "studio".into(),
                owner: String::new(),
                name: "no-push".into(),
                path: "/rules/no-push.md".into(),
                text: "---\nkind: gate\nlevel: block\n---\nAgents never push.\n".into(),
                problem: None,
                disabled: false,
            }],
            trophies: vec![
                Trophy {
                    id: "first-goal-merged".into(),
                    title: "First goal merged".into(),
                    detail: "A whole goal landed".into(),
                    earned_at: Some(NOW - 3 * HOUR),
                    progress: None,
                },
                Trophy {
                    id: "landed-25".into(),
                    title: "25 tickets landed".into(),
                    detail: "Land 25 tickets".into(),
                    earned_at: None,
                    progress: Some((12, 25)),
                },
            ],
            checks: vec![ProjectCheck {
                project: "shop".into(),
                verify: vec![vec!["cargo".into(), "test".into()]],
                running: vec!["SHOP-3".into()],
                latest: Some(CheckResult {
                    ticket: "SHOP-2".into(),
                    passed: true,
                    at_ms: NOW - 5 * 60_000,
                }),
            }],
            days,
        }
    }

    async fn html_of(office: Office, extra: Extra, sel: &str) -> String {
        html_on(office, extra, sel, "needs").await
    }

    async fn html_on(office: Office, extra: Extra, sel: &str, tab: &str) -> String {
        let cx = Cx::default();
        let (sel, tab) = (sel.to_string(), tab.to_string());
        ui::html(
            &cx,
            view! { cx => panel(office: office, detail: Detail::default(), extra: extra, sel: sel, tab: tab) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_link_can_open_another_tab() {
        let html = html_on(office(), extra(), "", tab("money")).await;
        assert!(
            html.contains(r#"<div class="pane" role="tabpanel" data-show="$tab == 'money'">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"style="display: none" data-show="$tab == 'needs'""#),
            "{html}"
        );
        assert_eq!(tab("nonsense"), "needs");
    }

    #[tokio::test]
    async fn the_tabs_switch_without_the_server() {
        let html = html_of(office(), extra(), "").await;
        for (name, label) in TABS {
            assert!(html.contains(label), "{label}: {html}");
            assert!(html.contains(&format!("$tab = '{name}'")), "{name}: {html}");
            assert!(
                html.contains(&format!("data-show=\"$tab == '{name}'\"")),
                "{name}: {html}"
            );
        }
        // The one that needs the owner shows first, with how many.
        assert!(
            html.contains(r#"<span class="count needs">2</span>"#),
            "{html}"
        );
    }

    #[tokio::test]
    async fn needs_you_answers_an_ask_in_place() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("@post('/asks/a1'"), "{html}");
        for word in [
            "Allow",
            "Deny",
            "Always allow",
            "Note for the agent",
            "Ready to land",
        ] {
            assert!(html.contains(word), "{word}: {html}");
        }
        assert!(html.contains("@post('/tickets/SHOP-5/land')"), "{html}");
        assert!(html.contains("@post('/tickets/API-2/resume')"), "{html}");
    }

    #[tokio::test]
    async fn projects_list_goals_tasks_and_loose_tickets() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("pjd pj-0"), "{html}");
        assert!(html.contains("pjd pj-1"), "{html}");
        // The goal: one of three landed.
        assert!(html.contains("Checkout flow"), "{html}");
        assert!(html.contains("1 of 3"), "{html}");
        for word in [
            "landed",
            "working",
            "up next",
            "needs you",
            "failed",
            "ready to land",
        ] {
            assert!(html.contains(&format!(">{word}<")), "{word}: {html}");
        }
        assert!(
            html.contains("$sel = 'tickets/SHOP-3'; @get('/office/panel')"),
            "{html}"
        );
        // The held, ready, failed and queued ones have no goal, and are
        // listed apart, the queued one as up next.
        assert!(html.contains("Without a goal"), "{html}");
        assert!(html.contains("tickets/API-3'"), "{html}");
    }

    #[tokio::test]
    async fn a_done_goal_waits_for_its_merge_in_two_places() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets.remove(3);
        let html = html_of(office, extra(), "").await;
        assert!(html.contains("Goals done"), "{html}");
        assert!(html.contains("Merge into"), "{html}");
        // Once under Needs you and once in its project.
        assert_eq!(
            html.matches("@post('/tickets/SHOP-2/merge')").count(),
            2,
            "{html}"
        );
    }

    #[test]
    fn a_done_goal_waits_for_its_merge() {
        let mut office = office();
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets.remove(3);
        let shop = &project_rows(&office)[0];
        assert!(shop.goals[0].done);
        assert_eq!(
            shop.goals[0].merge,
            Some(("@post('/tickets/SHOP-2/merge')".into(), "main".into()))
        );
    }

    #[test]
    fn levels_come_from_landed_tickets() {
        assert_eq!(level(0), (1, Some((0, 2))));
        assert_eq!(level(1), (1, Some((1, 2))));
        assert_eq!(level(2), (2, Some((0, 3))));
        assert_eq!(level(4), (2, Some((2, 3))));
        assert_eq!(level(5), (3, Some((0, 5))));
        assert_eq!(level(149).0, 9);
        assert_eq!(level(150), (10, None));
        assert_eq!(level(900), (10, None));
    }

    #[tokio::test]
    async fn the_team_shows_levels_numbers_and_the_front_desk() {
        let html = html_of(office(), extra(), "").await;
        // dev landed two tickets: level 2.
        assert!(html.contains("Lv 2"), "{html}");
        assert!(html.contains("Engineer · sonnet · on SHOP-3"), "{html}");
        assert!(html.contains("$4.40"), "{html}");
        assert!(
            html.contains("Levels come from the tickets a hire has landed."),
            "{html}"
        );
        assert!(html.contains("href=\"/hires?new\""), "{html}");
        // The approver: ten decided today, one sent to the owner.
        assert!(html.contains("<b>10</b><small>decided</small>"), "{html}");
        assert!(html.contains("<small>to you</small>"), "{html}");
        assert!(html.contains("$sel = 'hires/dev'"), "{html}");
        // The front desk has no levels.
        let approver = html.split("<b>approver</b>").next().unwrap_or_default();
        assert!(!approver.ends_with("Lv 1"), "{html}");
    }

    #[tokio::test]
    async fn money_adds_up_what_studiod_says() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("$6.40"), "{html}");
        assert!(html.contains("$0.72 held by live runs"), "{html}");
        assert!(html.contains("$2.88 left"), "{html}");
        // By project: SHOP-3's $3.90 is the shop's; API-1's $1.10 the api's.
        assert!(html.contains("$3.90"), "{html}");
        assert!(html.contains("$1.10"), "{html}");
        assert!(html.contains("Oct 19"), "{html}");
        assert!(html.contains("daily limit $10"), "{html}");
        assert!(html.contains("@post('/office/limits'"), "{html}");
        assert!(html.contains("name=\"daily_usd\""), "{html}");
        assert!(html.contains("name=\"ticket_usd\""), "{html}");
        assert!(html.contains("value=\"2.5\""), "{html}");
    }

    #[test]
    fn the_chart_is_as_tall_as_the_biggest_day_or_the_limit() {
        let day = |usd| DaySpend {
            date: "2026-10-19".into(),
            start_ms: 0,
            usd,
        };
        let (bars, limit) = bars(&[day(2.0), day(5.0)], 10.0);
        assert_eq!(bars[1].height, 50.0);
        assert_eq!(limit, Some(100.0));
        assert_eq!(bars[1].label, "Today");
        assert_eq!(bars[0].label, "Oct 19");
        let (bars, limit) = super::bars(&[day(20.0)], 10.0);
        assert_eq!((bars[0].height, limit), (100.0, Some(50.0)));
        assert_eq!(super::bars(&[], 10.0).0.len(), 0);
    }

    #[tokio::test]
    async fn rules_link_to_their_editor() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("href=\"/rules?file=no-push\""), "{html}");
        assert!(html.contains("<span class=\"chip\">gate</span>"), "{html}");
        assert!(html.contains("chip bad\">block<"), "{html}");
        assert!(html.contains("Agents never push."), "{html}");
        assert!(html.contains("href=\"/rules?new\""), "{html}");
    }

    #[tokio::test]
    async fn the_log_is_newest_first_with_project_dots() {
        let html = html_of(office(), extra(), "").await;
        let log = &html[html.find(r#"data-show="$tab == 'log'""#).unwrap()..];
        let api = log.find(">API-1<").unwrap();
        let shop = log.find(">SHOP-2<").unwrap();
        assert!(api < shop, "{log}");
        assert!(log.contains("failed"), "{log}");
        assert!(log.contains(">1h<") && log.contains(">2h<"), "{log}");
    }

    #[tokio::test]
    async fn a_picked_ticket_gets_its_panel_with_a_way_back() {
        let html = html_of(office(), extra(), "tickets/SHOP-5").await;
        assert!(html.contains("$sel = ''; @get('/office/panel')"), "{html}");
        assert!(!html.contains("role=\"tablist\""), "{html}");
    }

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

    #[tokio::test]
    async fn without_extra_nothing_is_made_up() {
        let empty = Extra::default();
        let all = html_of(office(), empty.clone(), "").await;
        // Only what the office knows: no held, left, per-hire or per-day
        // figures, no rules.
        for made_up in [
            "held by live runs",
            " left",
            "By hire",
            "By project",
            "14 days",
            "$4.40",
        ] {
            assert!(!all.contains(made_up), "{made_up}: {all}");
        }
        assert!(all.contains("$6.40"), "{all}");
        assert!(all.contains("No rules yet."), "{all}");
        for element in Element::ALL {
            let html = html_of(
                office(),
                empty.clone(),
                &format!("elements/{}", element.key()),
            )
            .await;
            assert!(
                html.contains("$sel = ''; @get('/office/panel')"),
                "{}: {html}",
                element.key()
            );
            for made_up in ["held by live runs", "Passed", "3h ago", "25 tickets"] {
                assert!(
                    !html.contains(made_up),
                    "{}: {made_up}: {html}",
                    element.key()
                );
            }
        }
        let trophies = html_of(office(), empty.clone(), "elements/trophies").await;
        assert!(trophies.contains("No trophies to show yet."), "{trophies}");
        let racks = html_of(office(), empty, "elements/racks").await;
        assert!(racks.contains("No project checks to show."), "{racks}");
    }

    #[tokio::test]
    async fn a_name_cannot_break_out_of_the_page() {
        let mut office = office();
        office.status.team[0].id = "d<b>ev".into();
        let html = html_of(office, extra(), "").await;
        assert!(!html.contains("<b>ev"), "{html}");
    }
}
