//! Look closer: a sheet that slides over a conversation from the right, with
//! the work's summary, its checks, its tasks and files, and what to do with it.

use std::collections::BTreeMap;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};
use studio_git::Repo;
use studiod::api::{CheckState, EvidenceFile, Request};
use topcoat::view::{View, component, view};

use super::lead_href;
use super::talk::thread::Person;
use crate::pages::activity::{self, Answer, Logged};
use crate::pages::changes;
use crate::pages::cost::{self, Costs};
use crate::pages::office::{Office, floor};
use crate::{AppConfig, client, diffstat, ui};

/// What a proof tile says: its tone is `ok`, `attn`, `bad` or `neutral`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub tone: &'static str,
    pub title: &'static str,
    pub words: String,
}

/// One task of the work.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub title: String,
    /// As `ui::look` knows it.
    pub state: &'static str,
    pub who: Person,
    pub lines: Option<(u64, u64)>,
}

/// What shipping does, and where it posts.
#[derive(Clone, Debug, PartialEq)]
pub struct Ship {
    pub post: String,
    /// The branch a goal is shipped to, such as "main"; a task is only added
    /// to its goal, so it has none.
    pub into: Option<String>,
    /// The checks failed: shipping still works, and the sheet says so first.
    pub failed: Option<String>,
}

/// What the sheet shows for one piece of work: a goal's plan ticket or a task.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub project: String,
    /// What `?look=` opens.
    pub id: String,
    pub title: String,
    /// The lead's sentence on it, if anyone wrote one.
    pub summary: String,
    pub lead: Option<Person>,
    pub tint: usize,
    pub checks: Tile,
    pub review: Tile,
    pub protected: Tile,
    pub tasks: Vec<Row>,
    /// What the work cost, split by kind of run.
    pub costs: Costs,
    /// Screenshots and clips of each task, grouped by ticket id.
    pub evidence: Vec<(String, Vec<EvidenceFile>)>,
    pub added: u64,
    pub removed: u64,
    /// The whole change as a unified diff.
    pub diff: String,
    pub files: usize,
    /// Where it is sent back from: a goal goes back through its lead.
    pub goal: bool,
    /// None when there is nothing to ship (not ready, or already shipped).
    pub ship: Option<Ship>,
    pub branch: String,
    /// The folder to open in an editor.
    pub folder: String,
}

/// The tasks of the work, and what each has to show.
struct Task {
    ticket: Ticket,
    /// Lines added and removed, as studiod told them: none when it could not
    /// tell, and none of either when the task changed nothing.
    lines: Option<(u64, u64)>,
    events: Vec<client::Stamped>,
    log: String,
    evidence: Vec<EvidenceFile>,
}

/// What was read for the sheet of one piece of work.
struct Read {
    title: String,
    /// The change of the work itself, as studiod reads it from git: a goal's
    /// is its branch against the base it will ship to, so it is there when its
    /// tasks' worktrees are gone. As a unified diff; empty when studiod had
    /// none.
    diff: String,
    /// Lines added and removed by the change, from `git diff --stat`, which
    /// is exact.
    lines: Option<(u64, u64)>,
    tasks: Vec<Task>,
    costs: Costs,
}

/// Reads what the sheet of `id` shows; none when the project has no such work.
pub async fn gather(
    config: &AppConfig,
    office: &Office,
    project: &str,
    id: &str,
    tint: usize,
) -> Option<Sheet> {
    let goal = floor::all_goals(office)
        .into_iter()
        .find(|g| g.plan.id.to_string() == id && g.plan.project.to_string() == project);
    let (title, tickets): (String, Vec<Ticket>) = match &goal {
        Some(goal) => (goal.name(), goal.filed.clone()),
        None => {
            let task = office
                .ticket(id)
                .filter(|t| t.project.to_string() == project && t.kind != TicketKind::Plan)?;
            (task.title.clone(), vec![task.clone()])
        }
    };
    // The work's change and the size of each task, together in one look.
    let mut asked = vec![id.to_string()];
    asked.extend(
        tickets
            .iter()
            .map(|ticket| ticket.id.to_string())
            .filter(|task| task != id),
    );
    let goal_of = |ticket: &Ticket| cost::goal_ids(&office.tickets, ticket);
    let (own_ids, goal_ids, scope) = match &goal {
        Some(_) => (asked.clone(), Vec::new(), "on this goal"),
        None => {
            let task = office.ticket(id);
            (
                vec![id.to_string()],
                task.map(goal_of).unwrap_or_default(),
                "on this task",
            )
        }
    };
    let (own, whole) = tokio::join!(
        cost::read(&config.socket, own_ids),
        cost::read(&config.socket, goal_ids),
    );
    let name_of = |hire: &str| cost::name_in(&office.status.team, hire);
    let costs = if goal.is_some() {
        cost::costs(vec![("The goal", scope, own)], &name_of)
    } else {
        cost::costs(
            vec![
                ("This task", scope, own),
                ("Its goal", "on its goal", whole),
            ],
            &name_of,
        )
    };
    let (diff, stats) = tokio::join!(
        client::call::<Option<String>>(
            &config.socket,
            Request::Diff {
                ticket: id.to_string()
            }
        ),
        client::call::<BTreeMap<String, Option<String>>>(
            &config.socket,
            Request::Diffstats { tickets: asked },
        ),
    );
    let stats = stats.unwrap_or_default();
    // A stat that is empty is a change of nothing; none is one not told.
    let lines_of = |ticket: &str| {
        let stat = stats.get(ticket)?.as_deref()?;
        Some(diffstat::totals(stat).unwrap_or((0, 0)))
    };
    let (diff, lines) = (diff.ok().flatten().unwrap_or_default(), lines_of(id));
    let tasks = futures_util::future::join_all(tickets.into_iter().map(|ticket| {
        let lines = lines_of(&ticket.id.to_string());
        async move {
            let task = ticket.id.to_string();
            let (events, log, evidence) = tokio::join!(
                client::call::<Vec<client::Stamped>>(
                    &config.socket,
                    Request::Events {
                        since: 0,
                        limit: usize::MAX,
                        ticket: Some(task.clone()),
                    },
                ),
                tokio::fs::read_to_string(
                    config.data_dir.join("runs").join(format!("{task}.jsonl"))
                ),
                client::call::<Vec<EvidenceFile>>(
                    &config.socket,
                    Request::Evidence {
                        ticket: task.clone(),
                    },
                ),
            );
            Task {
                ticket,
                lines,
                events: events.unwrap_or_default(),
                log: log.unwrap_or_default(),
                evidence: evidence.unwrap_or_default(),
            }
        }
    }))
    .await;
    let read = Read {
        title,
        diff,
        lines,
        tasks,
        costs,
    };
    Some(build(office, project, id, tint, goal.as_ref(), read))
}

/// The sheet from what was read.
fn build(
    office: &Office,
    project: &str,
    id: &str,
    tint: usize,
    goal: Option<&floor::Goal>,
    read: Read,
) -> Sheet {
    let Read {
        title,
        diff,
        lines,
        tasks,
        costs,
    } = read;
    let rows: Vec<Row> = tasks
        .iter()
        .map(|task| Row {
            title: task.ticket.title.clone(),
            state: ui::shown(&task.ticket.state),
            who: Person::of(office, &task.ticket.hire.to_string()),
            lines: task.lines,
        })
        .collect();
    let files = changes::stats(&diff);
    // The diff is cut when it is huge, so the exact count comes first.
    let (added, removed) = lines.unwrap_or_else(|| {
        files
            .iter()
            .fold((0, 0), |(a, r), (_, x, y)| (a + x, r + y))
    });
    let files = files.len();

    let (checks, failed) = match goal {
        Some(goal) => goal_checks(office, &goal.plan.id.to_string()),
        None => (task_checks(&tasks[0]), None),
    };
    let summary = match goal {
        Some(goal) => super::goal_said(office, goal),
        None => super::task_result(office, id),
    };

    let base = tasks
        .first()
        .map(|t| t.ticket.base_branch.clone())
        .unwrap_or_default();
    let ship = match goal {
        Some(goal) if goal.state() == "done" => goal
            .filed
            .iter()
            .find(|t| t.state.kind() == "landed")
            .map(|landed| Ship {
                post: format!("/tickets/{}/merge", landed.id),
                into: Some(base.clone()),
                failed: failed.clone(),
            }),
        Some(_) => None,
        None => tasks
            .first()
            .filter(|t| t.ticket.state.kind() == "ready")
            .map(|t| Ship {
                post: format!("/tickets/{}/land", t.ticket.id),
                into: None,
                failed: None,
            }),
    };
    let (branch, folder) = match goal {
        Some(goal) => (
            Repo::goal_branch_name(&goal.plan.goal),
            repo_path(office, project),
        ),
        None => (
            Repo::ticket_branch_name(&tasks[0].ticket.id),
            worktree_path(office, project, id),
        ),
    };
    let lead = super::talk::thread::lead_of(office, project);
    Sheet {
        project: project.to_string(),
        id: id.to_string(),
        title,
        summary,
        lead,
        tint,
        checks,
        review: review_tile(office, &tasks),
        protected: protected_tile(&tasks),
        tasks: rows,
        costs,
        evidence: tasks
            .iter()
            .filter(|task| !task.evidence.is_empty())
            .map(|task| (task.ticket.id.to_string(), task.evidence.clone()))
            .collect(),
        added,
        removed,
        diff,
        files,
        goal: goal.is_some(),
        ship,
        branch,
        folder,
    }
}

/// The latest check state the task itself recorded, not an assumption from
/// whether it happens to be ready to ship.
fn task_checks(task: &Task) -> Tile {
    let latest = task
        .events
        .iter()
        .rev()
        .find_map(|stored| match &stored.event {
            TicketEvent::ChecksStarted => Some(("neutral", "Checks", "running…".to_string())),
            TicketEvent::ChecksPassed => Some(("ok", "Checks", "passed".to_string())),
            TicketEvent::ChecksFailed { detail } => Some(("bad", "Checks failed", detail.clone())),
            _ => None,
        });
    let (tone, title, words) =
        latest.unwrap_or_else(|| ("neutral", "Checks", "not run yet".to_string()));
    Tile { tone, title, words }
}

/// What the checks of a goal's branch came to.
fn goal_checks(office: &Office, plan: &str) -> (Tile, Option<String>) {
    let Some(check) = office.ship_checks.get(plan) else {
        let words = "not run yet".to_string();
        let tile = Tile {
            tone: "neutral",
            title: "Checks",
            words,
        };
        return (tile, None);
    };
    match check.state {
        CheckState::Passed => (
            Tile {
                tone: "ok",
                title: "Checks",
                words: if check.summary.is_empty() {
                    "passed".to_string()
                } else {
                    check.summary.clone()
                },
            },
            None,
        ),
        CheckState::Failed => (
            Tile {
                tone: "bad",
                title: "Checks failed",
                words: check.summary.clone(),
            },
            Some(check.summary.clone()),
        ),
        CheckState::Running => (
            Tile {
                tone: "neutral",
                title: "Checks",
                words: "running…".to_string(),
            },
            None,
        ),
    }
}

/// Whether a reviewer agreed with each task: its last word on the matter.
fn review_tile(office: &Office, tasks: &[Task]) -> Tile {
    let agreed = tasks
        .iter()
        .filter(|task| {
            let id = task.ticket.id.to_string();
            let last = task.events.iter().rev().find_map(|e| match e.event {
                TicketEvent::ReviewPassed => Some(true),
                TicketEvent::ReviewFailed { .. } => Some(false),
                _ => None,
            });
            last == Some(true) && !office.unreviewed.contains(&id)
        })
        .count();
    let (tone, words) = if tasks.is_empty() {
        ("neutral", "nothing to read".to_string())
    } else if agreed == tasks.len() {
        ("ok", "Reviewer agreed".to_string())
    } else if agreed == 0 {
        ("attn", "Not reviewed by anyone else".to_string())
    } else {
        (
            "attn",
            format!("Reviewer agreed on {agreed} of {}", tasks.len()),
        )
    };
    Tile {
        tone,
        title: "Review",
        words,
    }
}

/// The edit tools whose files a task may be asked about.
const EDITS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The files the owner allowed a task to change although they were protected:
/// the edits that reached the owner and were allowed.
fn protected_paths(task: &Task) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for (_, line) in activity::parse_log(&task.log) {
        let Logged::Asked {
            tool,
            input,
            answer: Answer::Allowed,
        } = line
        else {
            continue;
        };
        if !EDITS.contains(&tool.as_str()) {
            continue;
        }
        let Some(path) = input["file_path"]
            .as_str()
            .or_else(|| input["notebook_path"].as_str())
        else {
            continue;
        };
        let path = path
            .split_once("/.studio/worktrees/")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map_or(path, |(_, inside)| inside)
            .to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn protected_tile(tasks: &[Task]) -> Tile {
    let mut paths: Vec<String> = Vec::new();
    for path in tasks.iter().flat_map(protected_paths) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Tile {
            tone: "ok",
            title: "Protected paths",
            words: "none touched".to_string(),
        };
    }
    let shown = match paths.as_slice() {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
        [] => String::new(),
    };
    Tile {
        tone: "attn",
        title: "Protected paths",
        words: format!("{shown}, you allowed it"),
    }
}

/// The project's folder.
fn repo_path(office: &Office, project: &str) -> String {
    office
        .project(project)
        .map(|p| p.path.clone())
        .unwrap_or_default()
}

/// Where the task's worktree is: its folder under the project's `.studio`.
fn worktree_path(office: &Office, project: &str, task: &str) -> String {
    let name: String = task
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let repo = repo_path(office, project);
    if repo.is_empty() {
        return repo;
    }
    format!("{}/.studio/worktrees/{name}", repo.trim_end_matches('/'))
}

/// A folder as an editor's address: each byte outside letters, digits and
/// `-_.~/` is escaped.
fn editor_href(folder: &str) -> String {
    let mut href = String::from("vscode://file");
    if !folder.starts_with('/') {
        href.push('/');
    }
    for byte in folder.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                href.push(char::from(byte));
            }
            _ => href.push_str(&format!("%{byte:02X}")),
        }
    }
    href
}

/// How a tile looks, by tone.
fn tile_class(tone: &str) -> &'static str {
    match tone {
        "attn" => "border-attn-line bg-attn-bg",
        "bad" => "border-bad-line bg-bad-bg",
        _ => "border-line bg-bg-2",
    }
}

/// The sheet of `sheet`'s work, over the conversation it was opened from.
#[component]
pub async fn sheet(work: Sheet) -> topcoat::Result<impl View> {
    let Sheet {
        project,
        id,
        title,
        summary,
        lead,
        tint,
        checks,
        review,
        protected,
        tasks,
        costs,
        evidence,
        added,
        removed,
        diff,
        files,
        goal,
        ship,
        branch,
        folder,
    } = work;
    let about = title.clone();
    let back = lead_href(&project);
    let count = tasks.len();
    let by = match tasks.first() {
        Some(first) if tasks.iter().all(|t| t.who.hire == first.who.hire) => {
            Some(first.who.name.clone())
        }
        _ => None,
    };
    let switch = format!("git switch {branch}");
    let send_back = if goal {
        format!("@post('{back}/talk', {{contentType: 'form'}})")
    } else {
        format!("@post('/tickets/{id}/changes', {{contentType: 'form'}})")
    };
    let editor = (!folder.is_empty()).then(|| editor_href(&folder));
    let copy = format!(
        "navigator.clipboard.writeText({}); $_copied = true; setTimeout(() => $_copied = false, 1600)",
        serde_json::to_string(&switch).unwrap_or_default()
    );
    let ship_post = ship.as_ref().map(|s| format!("@post('{}')", s.post));
    Ok(view! {
        <div id="sheet" class="talk-sheet" data-work=(id.clone()) data-signals="{_copied: false}">
            <a href=(back.clone()) class="talk-sheet-dim" aria-label="Close"></a>
            <aside class="talk-sheet-panel" aria-label="Look closer">
                <header class="flex items-start gap-3 border-b border-line px-5 py-4">
                    <span class="grid size-8 shrink-0 place-items-center rounded-lg bg-ok text-on-primary">ui::icon(name: "git-merge")</span>
                    <div class="min-w-0 flex-1">
                        <h2 class="truncate text-[16px] font-semibold leading-6">(title)</h2>
                        <p class="text-[12.5px] text-fg-3">
                            (project.clone()) " · " (count) if count == 1 { " task" } else { " tasks" }
                            if added + removed > 0 { " · " <span class=(ui::ADD)>"+" (added)</span> " " <span class=(ui::DEL)>"−" (removed)</span> }
                        </p>
                    </div>
                    <a href=(back.clone()) class=(ui::ICON_BUTTON_SM) aria-label="Close">ui::icon(name: "x")</a>
                </header>
                <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 py-5">
                    if !summary.is_empty() {
                        <div class="flex items-start gap-3">
                            if let Some(lead) = lead {
                                super::faces::face(seed: lead.hire, role: lead.role, tint: tint, size: 30)
                            }
                            <p class="[overflow-wrap:anywhere]">(summary)</p>
                        </div>
                    }
                    <div class="grid grid-cols-3 gap-2.5 max-[760px]:grid-cols-1">
                        proof(tile: checks)
                        proof(tile: review)
                        proof(tile: protected)
                    </div>
                    <section class="flex flex-col gap-2">
                        <h3 class="text-[12px] font-medium uppercase text-fg-3">
                            (count) if count == 1 { " task" } else { " tasks" }
                            if let Some(by) = by { ", all by " (by) }
                        </h3>
                        <ul class=(ui::LIST)>
                            for task in tasks {
                                <li class="flex items-center gap-2.5 border-t border-line px-3.5 py-2.5 first:border-t-0">
                                    ui::state_icon(state: task.state, small: true)
                                    <span class="min-w-0 flex-1 truncate">(task.title)</span>
                                    if let Some((a, r)) = task.lines {
                                        // A task that changed nothing says so, not "+0 −0".
                                        if a + r > 0 {
                                            ui::diff_size(added: a, removed: r)
                                        } else {
                                            <span class="shrink-0 text-[12.5px] text-fg-3">"No changes to show"</span>
                                        }
                                    }
                                </li>
                            }
                        </ul>
                    </section>
                    if !costs.parts.is_empty() {
                        <section class="flex flex-col gap-2" aria-label="What it cost">
                            <h3 class="text-[12px] font-medium uppercase text-fg-3">"What it cost"</h3>
                            cost::costs_view(costs: costs)
                        </section>
                    }
                    for (task, captures) in evidence {
                        <section class="flex flex-col gap-2" aria-label=(format!("Evidence for {task}"))>
                            <h3 class="text-[12px] font-medium uppercase text-fg-3">"Visual proof · " (task.clone())</h3>
                            crate::pages::evidence::screenshots(ticket: task, files: captures)
                        </section>
                    }
                    if files > 0 {
                        <section class="flex flex-col gap-2">
                            <h3 class="text-[12px] font-medium uppercase text-fg-3">(files) if files == 1 { " file" } else { " files" }</h3>
                            changes::diff_files(diff: diff, open_first: true)
                        </section>
                    }
                </div>
                <footer class="flex flex-col gap-3.5 border-t border-line px-5 py-4">
                    if let Some(ship) = ship {
                        if let Some(why) = ship.failed.clone() {
                            <p class="text-[13px] text-bad-fg">"The checks failed: " (why) ". Shipping it anyway still works."</p>
                        }
                        <div class="flex flex-wrap items-center gap-2.5">
                            <button type="button" class=(ui::PRIMARY) data-on:click=(ship_post.unwrap_or_default())>
                                ui::icon(name: "git-merge")
                                if ship.failed.is_some() { "Ship anyway" } else if let Some(into) = ship.into.clone() { "Ship to " (into) } else { "Ship" }
                            </button>
                            <details id="send-back" data-preserve-attr="open" class="group [&[open]]:basis-full">
                                <summary class=(format!("{} list-none [&::-webkit-details-marker]:hidden", ui::BUTTON))>"Send back with a note"</summary>
                                <form data-on:submit=(send_back) class="mt-3 flex flex-col gap-2.5">
                                    if goal {
                                        <input type="hidden" name="about" value=(about.clone())>
                                        <textarea name="text" required="" rows="3" aria-label="What should change" placeholder="What should change?" class=(ui::TEXTAREA)></textarea>
                                    } else {
                                        <textarea name="feedback" required="" rows="3" aria-label="What should change" placeholder="What should change?" class=(ui::TEXTAREA)></textarea>
                                    }
                                    <div><button type="submit" class=(ui::BUTTON)>"Send back"</button></div>
                                </form>
                            </details>
                        </div>
                    }
                    <div class="flex flex-wrap items-center gap-2.5">
                        <span class="text-[12.5px] text-fg-3">"Try it first"</span>
                        <code class="min-w-0 flex-1 truncate rounded-lg border border-line-2 bg-bg-2 px-3 py-1.5 font-mono text-[12.5px]">(switch.clone())</code>
                        <button type="button" class=(ui::ICON_BUTTON_SM) aria-label="Copy the command" data-on:click=(copy)>
                            <span data-show="!$_copied">ui::icon(name: "copy")</span>
                            <span data-show="$_copied" style="display:none">ui::icon(name: "check")</span>
                        </button>
                        if let Some(href) = editor {
                            <a href=(href) class=(ui::BUTTON_SM)>"Open in editor"</a>
                        }
                    </div>
                </footer>
            </aside>
        </div>
    })
}

/// One proof tile: what was checked, and what came of it.
#[component]
async fn proof(tile: Tile) -> topcoat::Result<impl View> {
    let Tile { tone, title, words } = tile;
    let icon = if tone == "ok" {
        "circle-check"
    } else {
        "circle-alert"
    };
    let colour = match tone {
        "ok" => "text-ok-fg",
        "attn" => "text-attn-fg",
        "bad" => "text-bad-fg",
        _ => "text-fg-3",
    };
    Ok(view! {
        <div class=(format!("flex flex-col gap-1 rounded-xl border p-3 {}", tile_class(tone)))>
            <span class=(colour)>ui::icon(name: icon)</span>
            <b class="font-semibold">(title)</b>
            <span class="text-[12.5px] text-fg-3 [overflow-wrap:anywhere]">(words)</span>
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use studiod::api::{ProjectView, ShipCheck};

    fn ticket(id: &str, state: serde_json::Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": "shop", "title": format!("Title {id}"), "goal": "Save carts",
            "hire": "dev", "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    fn task(id: &str, events: &[serde_json::Value], log: &[serde_json::Value]) -> Task {
        Task {
            ticket: ticket(id, json!("ready")),
            lines: None,
            events: events
                .iter()
                .map(|e| serde_json::from_value(json!({"at_ms": 1, "event": e})).unwrap())
                .collect(),
            log: log.iter().map(|l| format!("{l}\n")).collect(),
            evidence: Vec::new(),
        }
    }

    fn asked(tool: &str, path: &str) -> serde_json::Value {
        json!({"at": 1, "event": {"type": "permission", "tool": tool, "input": {"file_path": path}}})
    }

    fn went_on() -> serde_json::Value {
        json!({"at": 2, "event": {"type": "text", "text": "Going on."}})
    }

    #[test]
    fn the_check_tile_uses_the_tasks_latest_real_check_event() {
        let none = task("S-1", &[], &[]);
        assert_eq!(task_checks(&none).words, "not run yet");
        let running = task("S-1", &[json!({"type":"checks_started"})], &[]);
        assert_eq!(task_checks(&running).words, "running…");
        let failed = task(
            "S-1",
            &[
                json!({"type":"checks_passed"}),
                json!({"type":"checks_failed", "detail":"2 failed"}),
            ],
            &[],
        );
        let tile = task_checks(&failed);
        assert_eq!(
            (tile.tone, tile.title, tile.words.as_str()),
            ("bad", "Checks failed", "2 failed")
        );
        let passed = task("S-1", &[json!({"type":"checks_passed"})], &[]);
        assert_eq!(task_checks(&passed).tone, "ok");
    }

    #[test]
    fn the_review_tile_says_whether_a_reviewer_agreed_with_every_task() {
        let office = Office::default();
        let passed = json!({"type": "review_passed"});
        let failed = json!({"type": "review_failed", "detail": "no"});
        let tile = |tasks: Vec<Task>| review_tile(&office, &tasks);
        let agreed = tile(vec![task("S-1", std::slice::from_ref(&passed), &[])]);
        assert_eq!(
            (agreed.tone, agreed.words.as_str()),
            ("ok", "Reviewer agreed")
        );
        // The last word counts: sent back after it passed once.
        let back = tile(vec![task("S-1", &[passed.clone(), failed], &[])]);
        assert_eq!(back.tone, "attn");
        let half = tile(vec![task("S-1", &[passed], &[]), task("S-2", &[], &[])]);
        assert_eq!(half.words, "Reviewer agreed on 1 of 2");
        assert_eq!(
            tile(vec![task("S-1", &[], &[])]).words,
            "Not reviewed by anyone else"
        );
        // A ready task no reviewer on another model read was not reviewed.
        let unread = Office {
            unreviewed: vec!["S-1".into()],
            ..Office::default()
        };
        let tile = review_tile(
            &unread,
            &[task("S-1", &[json!({"type": "review_passed"})], &[])],
        );
        assert_eq!(tile.tone, "attn");
    }

    #[test]
    fn protected_paths_are_the_edits_the_owner_allowed() {
        let inside = "/work/shop/.studio/worktrees/shop-3/migrations/0007.sql";
        let allowed = task(
            "S-3",
            &[],
            &[
                asked("Edit", inside),
                went_on(),
                // Denied, so never touched; a command is no path; a rule or the
                // approver answered the last, so it never reached the owner.
                asked("Write", "/work/shop/.studio/worktrees/shop-3/secrets.env"),
                json!({"at": 3, "note": "denied: not that"}),
                json!({"at": 4, "event": {"type": "permission", "tool": "Bash", "input": {"command": "ls"}}}),
                went_on(),
                asked("Edit", "/work/shop/.studio/worktrees/shop-3/src/a.rs"),
                json!({"at": 5, "note": "approver allowed Edit"}),
                went_on(),
            ],
        );
        assert_eq!(protected_paths(&allowed), ["migrations/0007.sql"]);
        let tile = protected_tile(&[allowed]);
        assert_eq!(
            (tile.tone, tile.words.as_str()),
            ("attn", "migrations/0007.sql, you allowed it")
        );
        let none = protected_tile(&[task("S-4", &[], &[])]);
        assert_eq!((none.tone, none.words.as_str()), ("ok", "none touched"));
        // Many are counted, once each, across the tasks.
        let more = protected_tile(&[
            task("S-1", &[], &[asked("Edit", "a.sql"), went_on()]),
            task(
                "S-2",
                &[],
                &[
                    asked("Edit", "a.sql"),
                    went_on(),
                    asked("Edit", "b.sql"),
                    went_on(),
                ],
            ),
        ]);
        assert_eq!(more.words, "a.sql and 1 more, you allowed it");
    }

    #[test]
    fn the_checks_of_a_goal_come_from_what_the_studio_ran_on_its_branch() {
        let mut office = Office::default();
        let (tile, failed) = goal_checks(&office, "S-1");
        assert_eq!(
            (tile.tone, tile.words.as_str(), failed),
            ("neutral", "not run yet", None)
        );
        let mut check = |state, summary: &str| {
            office.ship_checks.insert(
                "S-1".into(),
                ShipCheck {
                    state,
                    summary: summary.into(),
                    at: 0,
                },
            );
            goal_checks(&office, "S-1")
        };
        let (tile, failed) = check(CheckState::Passed, "47 tests pass");
        assert_eq!(
            (tile.tone, tile.words.as_str(), failed),
            ("ok", "47 tests pass", None)
        );
        // A failure is said, and Ship asks about it first.
        let (tile, failed) = check(CheckState::Failed, "cargo test: 2 failed");
        assert_eq!((tile.tone, tile.title), ("bad", "Checks failed"));
        assert_eq!(failed.as_deref(), Some("cargo test: 2 failed"));
        let (tile, failed) = check(CheckState::Running, "");
        assert_eq!((tile.words.as_str(), failed), ("running…", None));
    }

    #[test]
    fn the_editor_opens_the_projects_folder_or_a_tasks_worktree() {
        let office = Office {
            projects: vec![ProjectView {
                name: "shop".into(),
                path: "/work/my shop/".into(),
                ..ProjectView::default()
            }],
            ..Office::default()
        };
        assert_eq!(
            editor_href(&repo_path(&office, "shop")),
            "vscode://file/work/my%20shop/"
        );
        assert_eq!(
            worktree_path(&office, "shop", "SHOP-12"),
            "/work/my shop/.studio/worktrees/shop-12"
        );
        // A project the studio can't place has no folder to open.
        assert_eq!(worktree_path(&office, "api", "API-1"), "");
        assert_eq!(editor_href("C:/work"), "vscode://file/C%3A/work");
    }
}
