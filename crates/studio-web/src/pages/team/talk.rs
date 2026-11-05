//! A conversation with a project's lead: the thread of what the owner and the
//! lead said, the cards that wait for the owner, a box to write in, and a rail
//! of who is working on the project. `thread` merges what was said with what is
//! true now, `cards` draws it, `rail` works out the rail. A project without a
//! lead has a card to give it one in place of the message box, and one with a
//! lead has a menu in its header to change it (`lead`). The page draws
//! itself whole, like every team page, and the one stream of the team pages
//! (`stream`) draws it again as it changes; what the owner has typed or
//! opened stays under `data-ignore-morph` or `data-preserve-attr`, or in a
//! field that the page gives no value of its own (the message box).

pub(super) mod asked;
mod cards;
pub(super) use cards::{fill, look_href};
mod lead;
mod rail;
pub(super) mod thread;

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use studiod::api::{ConversationView, Request, TalkFrom};
use topcoat::view::{Child, View, component, view};

use super::new::Hiring;
use super::shell::{self, Here};
use super::{Team, faces, lead_href, sheet};
use crate::pages::office::Office;
use crate::pages::office::model::Phase;
use crate::{AppConfig, client, diffstat, ui};
use cards::{DRAFT, entry};
use rail::{Money, Rail};
use thread::{Item, Person, Sizes};

/// What the owner can say with one click.
const CHIPS: [&str; 3] = ["How's it going?", "What's next?", "Plan something new"];

/// How long what was read of the budget is kept before it is read again: it
/// changes slowly.
const BUDGET_FRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// How long what was read of a goal's size is kept before it is read again:
/// the size moves when more work lands on the goal's branch.
const SIZES_FRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// What the page reads from studiod besides the office.
#[derive(Clone, Debug, Default)]
pub struct Reads {
    pub convo: ConversationView,
    pub sizes: Sizes,
    pub budget: Value,
}

/// How far a conversation had come when it was read: what a stream looks at to
/// know a message came, which the office alone may not show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Said {
    messages: usize,
    last: Option<u64>,
    thinking: bool,
}

impl Said {
    pub fn of(conversation: &ConversationView) -> Said {
        Said {
            messages: conversation.messages.len(),
            last: conversation.messages.last().map(|message| message.id),
            thinking: conversation.thinking,
        }
    }
}

impl Reads {
    pub fn said(&self) -> Said {
        Said::of(&self.convo)
    }
}

/// What a stream keeps from one read to the next: the sizes of the goals that
/// are ready to ship, which move only when more work lands on a goal, and the
/// budget, which moves slowly.
#[derive(Default)]
pub struct Memo {
    /// What studiod last said of the lines added and removed by each piece of
    /// work asked about, and when; none when it could not tell.
    sizes: HashMap<String, (Instant, Option<(u64, u64)>)>,
    budget: Option<(Instant, Value)>,
}

impl Memo {
    /// The lines added and removed by each of the goals `wanted` (by their plan
    /// tickets) that studiod can tell, as it said less than `SIZES_FRESH` ago,
    /// else as it says now.
    pub async fn sizes(&mut self, config: &AppConfig, wanted: Vec<String>) -> Sizes {
        let stale: Vec<String> = wanted
            .iter()
            .filter(|id| {
                self.sizes
                    .get(*id)
                    .is_none_or(|(at, _)| at.elapsed() >= SIZES_FRESH)
            })
            .cloned()
            .collect();
        if !stale.is_empty() {
            let said = client::call::<BTreeMap<String, Option<String>>>(
                &config.socket,
                Request::Diffstats {
                    tickets: stale.clone(),
                },
            )
            .await;
            let now = Instant::now();
            for id in stale {
                let lines = match &said {
                    Ok(stats) => stats
                        .get(&id)
                        .and_then(|stat| stat.as_deref())
                        .and_then(diffstat::totals),
                    // Studiod could not be asked: what was known stays.
                    Err(_) => self.sizes.get(&id).and_then(|(_, lines)| *lines),
                };
                self.sizes.insert(id, (now, lines));
            }
        }
        self.sizes.retain(|id, _| wanted.contains(id));
        wanted
            .into_iter()
            .filter_map(|id| {
                let (_, lines) = self.sizes.get(&id)?;
                Some((id, (*lines)?))
            })
            .collect()
    }

    /// What studiod says of the budget, as it said it less than `BUDGET_FRESH`
    /// ago, else as it says now.
    pub async fn budget(&mut self, config: &AppConfig) -> Value {
        match &self.budget {
            Some((at, budget)) if at.elapsed() < BUDGET_FRESH => budget.clone(),
            _ => {
                let budget = client::call::<Value>(&config.socket, Request::Budget)
                    .await
                    .unwrap_or_default();
                self.budget = Some((Instant::now(), budget.clone()));
                budget
            }
        }
    }
}

/// Reads the conversation of `project`, the sizes of the work ready to ship
/// and the budget.
pub async fn read(config: &AppConfig, office: &Office, project: &str, memo: &mut Memo) -> Reads {
    let socket = &config.socket;
    let request = Request::Conversation {
        project: project.to_string(),
        after: None,
    };
    let convo = client::call::<ConversationView>(socket, request)
        .await
        .unwrap_or_default();
    let sizes = memo
        .sizes(config, thread::wanted_sizes(office, project))
        .await;
    let budget = memo.budget(config).await;
    Reads {
        convo,
        sizes,
        budget,
    }
}

/// The sizes of the work that is ready to ship in every project of the studio,
/// for the page that says what waits.
pub async fn sizes_of_all(config: &AppConfig, office: &Office, memo: &mut Memo) -> Sizes {
    let wanted: Vec<String> = office
        .project_names()
        .iter()
        .flat_map(|project| thread::wanted_sizes(office, project))
        .collect();
    memo.sizes(config, wanted).await
}

/// What the header says.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub name: String,
    pub seed: String,
    pub project: String,
    pub has_lead: bool,
    pub phase: Phase,
    pub phase_words: String,
    pub progress: f64,
    /// "v0.4 · 1 of 3", when the project works towards a milestone.
    pub milestone: Option<String>,
}

/// Everything a conversation draws, worked out from the office and what was
/// read.
#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub project: String,
    /// The project's colour.
    pub tint: usize,
    pub now: u64,
    pub head: Head,
    pub items: Vec<Item>,
    /// The lead is working on a reply.
    pub thinking: Option<Person>,
    pub rail: Rail,
    /// Who the project's lead can be: a new hire or one the studio has.
    pub hiring: Hiring,
    /// The lead has not looked at how the project runs, and has said nothing
    /// yet, so the message box offers to have it look.
    pub offer: bool,
}

/// What the conversation of `project` shows, if the studio has it.
pub fn parts(team: &Team, office: &Office, project: &str, reads: &Reads) -> Option<Parts> {
    let lead = team.lead(project)?;
    let rail = rail::rail(office, project, rail::money(office, project, &reads.budget));
    // "v0.4 · 1 of 3"; a goal that has filed no task yet has no count to give.
    let milestone = rail.milestone.as_ref().and_then(|m| {
        let of = format!("{} of {}", m.done, m.total);
        match (&m.version, m.total) {
            (Some(v), 0) => Some(super::version(v)),
            (None, 0) => None,
            (Some(v), _) => Some(format!("{} · {of}", super::version(v))),
            (None, _) => Some(of),
        }
    });
    // A lead that nobody has set the project up with, and that has not said a
    // word here, is a new one: it offers to look at how the project runs.
    let offer = lead.lead.as_ref().is_some_and(|hire| {
        !office.setups.contains_key(project)
            && !reads
                .convo
                .messages
                .iter()
                .any(|m| matches!(&m.from, TalkFrom::Lead { hire: said } if said == hire))
    });
    Some(Parts {
        project: project.to_string(),
        tint: lead.tint,
        now: office.now,
        head: Head {
            name: lead.name.clone(),
            seed: lead.lead.clone().unwrap_or_else(|| project.to_string()),
            project: project.to_string(),
            has_lead: lead.lead.is_some(),
            phase: lead.phase,
            phase_words: lead.phase_words.clone(),
            progress: lead.progress,
            milestone,
        },
        items: thread::thread(office, project, &reads.convo, &reads.sizes),
        thinking: reads
            .convo
            .thinking
            .then(|| thread::lead_of(office, project))
            .flatten(),
        rail,
        hiring: Hiring::of(office),
        offer,
    })
}

/// The whole page, frame included: the conversation, its rail and, when
/// `sheet` is given, the sheet of the work to look closer at.
#[component]
pub async fn talk(
    team: Team,
    parts: Parts,
    sheet: Option<sheet::Sheet>,
) -> topcoat::Result<impl View> {
    let Parts {
        project,
        tint,
        now,
        head,
        items,
        thinking,
        rail,
        hiring,
        offer,
    } = parts;
    let (name, has_lead) = (head.name.clone(), head.has_lead);
    let back = lead_href(&project);
    let rail_project = project.clone();
    let send = format!(
        "if ($_draft.trim()) {{ @post('{back}/talk', {{contentType: 'form'}}); $_draft = '' }}"
    );
    // It is asked once: the chip goes as it is clicked, and stays gone while the
    // page still offers it.
    let look = format!("el.style.display = 'none'; @post('{back}/setup/start')");
    Ok(view! {
        shell::page(team: team, here: Here::Lead(project.clone()),
            rail: Child::new(view! { talk_rail(rail: rail, project: rail_project, tint: tint, now: now) }),
            <div id="talk-page" class="talk-page" data-signals="{_draft: '', _said: '', _bad: false}">
                talk_head(head: head, tint: tint, hiring: hiring.clone())
                talk_thread(items: items, project: project.clone(), tint: tint, now: now, thinking: thinking)
                if has_lead {
                    // What is typed in it is the owner's, and a patch of the page
                    // leaves it as typed: the field has no value of the page's own,
                    // and Datastar keeps a field's value while the page does not give
                    // it one. The rest of the form names the lead and offers what the
                    // lead may, so it follows the lead as the page is patched.
                    <form id="composer" class="flex flex-col gap-2.5 border-t border-line bg-bg px-7 pb-4 pt-3 max-[760px]:px-4" data-on:submit=(send)>
                        <div class="mx-auto flex w-full max-w-[780px] flex-wrap gap-1.5">
                            for chip in CHIPS {
                                <button type="button" class="h-7 rounded-full border border-line-2 px-3 text-[13px] text-fg-2 hover:bg-bg-3 hover:text-fg" data-on:click=(fill(chip))>(chip)</button>
                            }
                            if offer {
                                <button type="button" id="look-at-project" data-preserve-attr="style" class="h-7 rounded-full border border-run-line bg-run-bg px-3 text-[13px] text-run-fg hover:border-run-fg" data-on:click=(look)>
                                    "Ask " (name.clone()) " to look at how " (project.clone()) " runs"
                                </button>
                            }
                        </div>
                        <label class="mx-auto flex w-full max-w-[780px] items-center gap-2 rounded-xl border border-line-2 bg-bg-2 py-1.5 pl-3.5 pr-1.5 focus-within:border-ring">
                            <span class="sr-only">"Message " (name.clone())</span>
                            <input id=(DRAFT) name="text" data-bind:_draft="" autocomplete="off" placeholder=(format!("Message {name}")) class="h-8 min-w-0 flex-1 bg-transparent text-[14px] outline-none placeholder:text-fg-3">
                            <button type="submit" class=(ui::PRIMARY) aria-label="Send">ui::icon(name: "send")</button>
                        </label>
                    </form>
                } else {
                    lead::no_lead(project: project.clone(), hiring: hiring)
                }
                if let Some(sheet) = sheet {
                    sheet::sheet(work: sheet)
                }
            </div>
        )
    })
}

/// A thin track filled `value` of `max` in the project's colour.
#[component]
async fn bar(value: f64, max: f64, tint: usize) -> topcoat::Result<impl View> {
    let share = if max > 0.0 && value > 0.0 {
        (value / max * 100.0).min(100.0)
    } else {
        0.0
    };
    Ok(view! {
        <span class="block h-1 overflow-hidden rounded-sm bg-bg-4" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow=(format!("{share:.0}"))>
            <i class=(format!("pj-{tint} block h-full rounded-sm bg-[var(--pj)]")) style=(format!("width:{share:.1}%"))></i>
        </span>
    })
}

/// The tone a phase's pill takes.
fn tone(phase: Phase, words: &str) -> &'static str {
    match phase {
        Phase::Planning | Phase::Building | Phase::Testing => "run",
        Phase::Releasing if words.contains("waits for you") => "attn",
        Phase::Releasing => "run",
        Phase::Live => "ok",
        Phase::Idle | Phase::Paused => "neutral",
    }
}

/// The header: the lead's face and name, where the project is, and how far
/// its milestone has come. A project that has a lead has a menu in it to
/// change the lead.
#[component]
async fn talk_head(head: Head, tint: usize, hiring: Hiring) -> topcoat::Result<impl View> {
    let Head {
        name,
        seed,
        project,
        has_lead,
        phase,
        phase_words,
        progress,
        milestone,
    } = head;
    let tone = tone(phase, &phase_words);
    // The lead's hire id is what its face is drawn from.
    let (hire, led) = (seed.clone(), project.clone());
    Ok(view! {
        <header id="talk-head" class="flex flex-wrap items-center gap-x-3.5 gap-y-2 border-b border-line px-7 py-3.5 max-[760px]:px-4">
            faces::face(seed: seed, role: crate::pages::office::model::Role::Lead, tint: tint, size: 48)
            <div class="flex flex-col">
                <h1 class="sr-only">"Talk to " (name.clone())</h1>
                <b class="text-[17px] font-semibold leading-6" aria-hidden="true">(name)</b>
                <span class="text-[13px] text-fg-3">
                    if has_lead { "Leads " (project) } else { "Nobody leads " (project) " yet" }
                </span>
            </div>
            ui::pill(tone: tone, label: phase_words)
            if let Some(milestone) = milestone {
                <span class="flex items-center gap-2 text-[12.5px] text-fg-3">
                    <span class="tabular-nums">(milestone)</span>
                    <span class="w-[90px]">bar(value: progress, max: 1.0, tint: tint)</span>
                </span>
            }
            if has_lead {
                lead::change_lead(project: led, lead: hire, hiring: hiring)
            }
        </header>
    })
}

/// The thread, newest at the bottom, and "is replying" while the lead
/// works on an answer. The box scrolls from its bottom, so it stays there as
/// the thread grows.
#[component]
async fn talk_thread(
    items: Vec<Item>,
    project: String,
    tint: usize,
    now: u64,
    thinking: Option<Person>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="talk-thread" class="talk-thread" aria-label="The conversation">
            <div class="mx-auto flex w-full max-w-[780px] flex-col gap-5 px-7 py-6 max-[760px]:px-4">
                for item in items {
                    entry(item: item, project: project.clone(), tint: tint, now: now)
                }
                if let Some(lead) = thinking {
                    <div id="talk-replying" class="flex items-center gap-3 text-fg-3" role="status">
                        faces::face(seed: lead.hire, role: lead.role, tint: tint, size: 30)
                        <span class="talk-replying">(lead.name) " is replying…"</span>
                    </div>
                }
            </div>
        </div>
    })
}

/// "In shop now": who works on the project, the milestone and the spend.
#[component]
async fn talk_rail(
    rail: Rail,
    project: String,
    tint: usize,
    now: u64,
) -> topcoat::Result<impl View> {
    let Rail {
        workers,
        free,
        milestone,
        money: Money { spent, cap },
    } = rail;
    let heading = format!("In {project} now");
    Ok(view! {
        <div id="talk-rail" class="flex flex-col gap-6 px-5 py-5" aria-label=(heading.clone())>
            <section class="flex flex-col gap-3">
                <h2 class="text-[12px] font-medium uppercase text-fg-3">(heading)</h2>
                if workers.is_empty() {
                    <p class="text-[13px] text-fg-3">"Nobody is at work on it right now."</p>
                } else {
                    <ul class="flex flex-col gap-3.5">
                        for worker in workers {
                            <li>
                                worker_block(worker: worker, tint: tint, now: now)
                            </li>
                        }
                    </ul>
                }
                if !free.is_empty() {
                    <ul class="flex flex-col gap-2.5" aria-label="Free">
                        for person in free {
                            <li class="flex items-center gap-2.5">
                                faces::face(seed: person.hire, role: person.role, tint: tint, size: 24)
                                <b class="font-semibold">(person.name)</b>
                                <i class="inline-block size-[7px] rounded-full bg-fg-3"></i>
                                <span class="text-[12.5px] text-fg-3">"Free"</span>
                            </li>
                        }
                    </ul>
                }
            </section>
            if let Some(milestone) = milestone {
                <section class="flex flex-col gap-2.5 border-t border-line pt-5">
                    <h2 class="text-[12px] font-medium uppercase text-fg-3">"Milestone"</h2>
                    <b class="font-semibold">
                        if let Some(v) = milestone.version { (super::version(&v)) " " }
                        (milestone.title)
                    </b>
                    bar(value: milestone.done as f64, max: milestone.total as f64, tint: tint)
                    <ul class="flex flex-col gap-1.5">
                        for (title, state) in milestone.tasks {
                            <li class="flex items-center gap-2 text-fg-2">
                                ui::state_icon(state: state, small: true)
                                <span class="min-w-0 truncate">(title)</span>
                            </li>
                        }
                    </ul>
                </section>
            }
            <section class="flex flex-col gap-1.5 border-t border-line pt-5" title="What this project spent over the last 24 hours">
                <span class="flex items-baseline justify-between text-[13px]">
                    <span class="text-fg-3">"Spent today"</span>
                    <span class="tabular-nums"><b class="font-semibold">(ui::usd(spent))</b> <span class="text-fg-3">" of " (ui::limit(cap))</span></span>
                </span>
                bar(value: spent, max: cap, tint: tint)
            </section>
        </div>
    })
}

/// One person at work: face and name once, and under them each task they
/// are on, with a dot and what is happening to it.
#[component]
async fn worker_block(worker: rail::Worker, tint: usize, now: u64) -> topcoat::Result<impl View> {
    let rail::Worker { person, tasks } = worker;
    let name = person.name.clone();
    Ok(view! {
        <div class="flex flex-col gap-2">
            <span class="flex items-center gap-2.5">
                faces::face(seed: person.hire, role: person.role, tint: tint, size: 30)
                <b class="font-semibold">(person.name)</b>
            </span>
            <ul class="flex flex-col gap-2 pl-10">
                for job in tasks {
                    <li>
                        job_row(job: job, name: name.clone(), now: now)
                    </li>
                }
            </ul>
        </div>
    })
}

/// One task of a person: a dot, its title and what is happening to it
/// ("working, 4m", "in checks", "waiting for you"). A running task has a
/// button that opens a field to tell its agent something.
#[component]
async fn job_row(job: rail::Job, name: String, now: u64) -> topcoat::Result<impl View> {
    let rail::Job {
        task,
        title,
        tone,
        status,
        since,
        running,
    } = job;
    // How long it has been going is told of a task being worked on.
    let how_long = since
        .filter(|_| running)
        .map(|at| ui::elapsed(now.saturating_sub(at)));
    let tell = format!("@post('/tasks/{task}/tell', {{contentType: 'form'}}); el.reset()");
    let dot = if tone == "attn" { "bg-attn" } else { "bg-run" };
    let line = Child::new(view! {
        <i class=(format!("mt-[7px] inline-block size-[7px] shrink-0 rounded-full {dot}"))></i>
        <span class="min-w-0 flex-1">
            <span class="block [overflow-wrap:anywhere]">(title)</span>
            <span class="block text-[12.5px] text-fg-3">
                (status)
                if let Some(how_long) = how_long { ", " (how_long) }
            </span>
        </span>
    });
    Ok(view! {
        if running {
            <details id=(format!("tell-{task}")) data-preserve-attr="open" class="group">
                <summary class="flex cursor-pointer list-none items-start gap-2 [&::-webkit-details-marker]:hidden" title=(format!("Message {name}"))>
                    (line)
                    <span class="mt-1 text-fg-3 group-hover:text-fg group-open:text-fg" aria-label=(format!("Message {name}"))>ui::icon(name: "message-square-text", small: true)</span>
                </summary>
                <form class="mt-2 flex items-center gap-2" data-on:submit=(tell)>
                    <input name="text" required="" autocomplete="off" aria-label=(format!("Tell {name}")) placeholder=(format!("Tell {name} something")) class=(format!("{} text-[13px]", ui::INPUT))>
                    <button type="submit" class=(ui::BUTTON_SM)>"Send"</button>
                </form>
            </details>
        } else {
            <div class="flex items-start gap-2">(line)</div>
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use studio_core::ticket::Ticket;
    use studiod::api::ProjectView;

    fn ticket(id: &str, extra: Value) -> Ticket {
        let mut fields = json!({
            "id": id, "project": "shop", "title": format!("Title {id}"),
            "goal": "Save carts", "hire": "dev", "state": "queued",
            "attempts": 0, "base_branch": "main", "created_at": 1_000,
        });
        if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            fields.extend(extra.clone());
        }
        serde_json::from_value(fields).unwrap()
    }

    /// What the header of the shop's conversation says of its milestone.
    fn milestone_of(tickets: Vec<Ticket>) -> Option<String> {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects": ["shop"],
            "team": [{"id": "nora", "name": "Nora", "role": "lead", "model": "opus"}],
        }))
        .unwrap();
        office.projects = vec![ProjectView {
            name: "shop".into(),
            lead: Some("nora".into()),
            ..ProjectView::default()
        }];
        let team = Team::of(&office);
        parts(&team, &office, "shop", &Reads::default())
            .unwrap()
            .head
            .milestone
    }

    #[test]
    fn the_header_counts_a_goals_tasks_only_once_it_has_filed_some() {
        // A goal being planned has no task to count: its version alone, or
        // nothing, never "0 of 0".
        let planning = |extra: Value| {
            let mut fields = json!({"kind": "plan", "state": "working"});
            if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
                fields.extend(extra.clone());
            }
            vec![ticket("S-1", fields)]
        };
        assert_eq!(milestone_of(planning(json!({}))), None);
        assert_eq!(
            milestone_of(planning(json!({"version": "0.4"}))).as_deref(),
            Some("v0.4")
        );
        // Once it has filed tasks, how many are done.
        let filed = vec![
            ticket(
                "S-1",
                json!({"kind": "plan", "state": "landed", "version": "0.4"}),
            ),
            ticket("S-2", json!({"parent": "S-1", "state": "landed"})),
            ticket("S-3", json!({"parent": "S-1"})),
        ];
        assert_eq!(milestone_of(filed).as_deref(), Some("v0.4 · 1 of 2"));
    }
}
