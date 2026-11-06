//! A direct conversation with one hire, beside their current and recent work.

use studio_core::ticket::Ticket;
use studiod::api::{ConversationView, Request, TalkFrom, TalkMessage};
use topcoat::view::{Child, View, component, view};

use super::shell::{self, Here};
use super::talk::fill;
use super::talk::thread::Person;
use super::{Team, clock, faces, person_href};
use crate::pages::office::Office;
use crate::{AppConfig, client, ui};

const CHIPS: [&str; 3] = [
    "What are you working on?",
    "What's blocking you?",
    "What did you finish most recently?",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Parts {
    pub person: Person,
    pub role: String,
    pub model: String,
    pub tint: usize,
    pub now: u64,
    pub conversation: ConversationView,
    pub current: Vec<Ticket>,
    pub recent: Vec<Ticket>,
}

pub async fn read(config: &AppConfig, hire: &str) -> ConversationView {
    client::call(
        &config.socket,
        Request::HireConversation {
            hire: hire.to_string(),
            after: None,
        },
    )
    .await
    .unwrap_or_default()
}

pub fn parts(office: &Office, hire: &str, conversation: ConversationView) -> Option<Parts> {
    let member = office.member(hire)?;
    let person = Person::of(office, hire);
    let projects = office.project_names();
    let mut tasks: Vec<Ticket> = office
        .tickets
        .iter()
        .filter(|ticket| ticket.hire.to_string() == hire)
        .cloned()
        .collect();
    tasks.sort_by_key(|ticket| ticket.created_at);
    tasks.reverse();
    let project = tasks
        .iter()
        .find(|ticket| !matches!(ticket.state.kind(), "landed" | "failed" | "cancelled"))
        .or_else(|| tasks.first())
        .map(|ticket| ticket.project.to_string())
        .or_else(|| {
            office
                .projects
                .iter()
                .find(|project| project.lead.as_deref() == Some(hire))
                .map(|project| project.name.clone())
        });
    let tint = project
        .as_ref()
        .and_then(|name| projects.iter().position(|project| project == name))
        .unwrap_or(0);
    let mut current = Vec::new();
    let mut recent = Vec::new();
    for task in tasks {
        if matches!(task.state.kind(), "landed" | "failed" | "cancelled") {
            recent.push(task);
        } else {
            current.push(task);
        }
    }
    current.truncate(6);
    recent.truncate(6);
    Some(Parts {
        person,
        role: member.role.clone(),
        model: member.model.clone(),
        tint,
        now: office.now,
        conversation,
        current,
        recent,
    })
}

fn composer_send(hire: &str) -> String {
    let target = serde_json::to_string(&format!("{}/talk", person_href(hire))).unwrap_or_default();
    format!("if ($_draft.trim()) {{ @post({target}, {{contentType: 'form'}}); $_draft = '' }}")
}

#[component]
pub async fn person_view(team: Team, parts: Parts) -> topcoat::Result<impl View> {
    let Parts {
        person: who,
        role,
        model,
        tint,
        now,
        conversation,
        current,
        recent,
    } = parts;
    let hire = who.hire.clone();
    let name = who.name.clone();
    let rail_name = name.clone();
    let status = if current
        .iter()
        .any(|task| ui::run_under_way(task.state.kind()))
    {
        "Working"
    } else if current.is_empty() {
        "Free"
    } else {
        "Assigned"
    };
    let send = composer_send(&hire);
    Ok(view! {
        shell::page(team: team, here: Here::Person(hire.clone()),
            rail: Child::new(view! {
                work_rail(current: current, recent: recent, name: rail_name)
            }),
            <div id="person-page" class="talk-page" data-signals="{_draft: '', _said: '', _bad: false}">
                <header id="person-head" class="flex flex-wrap items-center gap-x-3.5 gap-y-2 border-b border-line px-7 py-3.5 max-[760px]:px-4">
                    faces::face(seed: who.hire.clone(), role: who.role, tint: tint, size: 48)
                    <div class="flex flex-col">
                        <h1 class="text-[17px] font-semibold leading-6">"Talk to " (name.clone())</h1>
                        <span class="text-[13px] capitalize text-fg-3">(role) " · " (model)</span>
                    </div>
                    ui::pill(tone: if status == "Working" { "run" } else { "neutral" }, label: status)
                </header>
                <div id="talk-thread" class="talk-thread" aria-label=(format!("The conversation with {name}"))>
                    <div class="mx-auto flex w-full max-w-[780px] flex-col gap-5 px-7 py-6 max-[760px]:px-4">
                        if conversation.messages.is_empty() {
                            <div class="rounded-xl border border-line bg-bg-2 px-4 py-3 text-[14px] text-fg-2">
                                "Ask " (name.clone()) " about their work. This conversation is read-only; use an active task's message control to steer its run."
                            </div>
                        }
                        for message in conversation.messages {
                            message_row(message: message, person: who.clone(), tint: tint, now: now)
                        }
                        if conversation.thinking {
                            <div id="talk-replying" class="flex items-center gap-3 text-fg-3" role="status">
                                faces::face(seed: who.hire.clone(), role: who.role, tint: tint, size: 30)
                                <span class="talk-replying">(name.clone()) " is replying…"</span>
                            </div>
                        }
                    </div>
                </div>
                <form id="composer" class="flex flex-col gap-2.5 border-t border-line bg-bg px-7 pb-4 pt-3 max-[760px]:px-4" data-on:submit=(send)>
                    <div class="mx-auto flex w-full max-w-[780px] flex-wrap gap-1.5">
                        for chip in CHIPS {
                            <button type="button" class="h-7 rounded-full border border-line-2 px-3 text-[13px] text-fg-2 hover:bg-bg-3 hover:text-fg" data-on:click=(fill(chip))>(chip)</button>
                        }
                    </div>
                    <label class="mx-auto flex w-full max-w-[780px] items-center gap-2 rounded-xl border border-line-2 bg-bg-2 py-1.5 pl-3.5 pr-1.5 focus-within:border-ring">
                        <span class="sr-only">"Message " (name.clone())</span>
                        <input name="text" data-bind:_draft="" autocomplete="off" placeholder=(format!("Message {name}")) class="h-8 min-w-0 flex-1 bg-transparent text-[14px] outline-none placeholder:text-fg-3">
                        <button type="submit" class=(ui::PRIMARY) aria-label="Send">ui::icon(name: "send")</button>
                    </label>
                </form>
            </div>
        )
    })
}

#[component]
async fn message_row(
    message: TalkMessage,
    person: Person,
    tint: usize,
    now: u64,
) -> topcoat::Result<impl View> {
    let owner = message.from == TalkFrom::Owner;
    let studio = message.from == TalkFrom::Studio;
    let name = match &message.from {
        TalkFrom::Owner => "You".to_string(),
        TalkFrom::Studio => "Studio".to_string(),
        TalkFrom::Lead { .. } => person.name.clone(),
    };
    Ok(view! {
        if owner {
            <div id=(format!("m-{}", message.id)) class="flex flex-col items-end gap-1">
                <p class="max-w-[70%] whitespace-pre-wrap rounded-2xl rounded-br-sm border border-line-2 bg-bg-4 px-3.5 py-2.5 [overflow-wrap:anywhere] max-[760px]:max-w-[88%]">(message.text)</p>
                <span class="text-[12.5px] text-fg-3">"You · " (clock::said(message.at, now))</span>
            </div>
        } else {
            <div id=(format!("m-{}", message.id)) class="flex items-start gap-3">
                if studio {
                    <span class="grid size-[30px] shrink-0 place-items-center rounded-full bg-primary text-on-primary" aria-hidden="true"><svg class="size-4"><use href="#logo"></use></svg></span>
                } else {
                    faces::face(seed: person.hire, role: person.role, tint: tint, size: 30)
                }
                <div class="flex min-w-0 flex-1 flex-col gap-1.5">
                    <span class="flex items-baseline gap-2"><b class="font-semibold">(name)</b><span class="text-[12.5px] text-fg-3">(clock::said(message.at, now))</span></span>
                    <p class="whitespace-pre-wrap [overflow-wrap:anywhere]">ui::prose(text: message.text)</p>
                </div>
            </div>
        }
    })
}

#[component]
async fn work_rail(
    current: Vec<Ticket>,
    recent: Vec<Ticket>,
    name: String,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="person-rail" class="flex flex-col gap-6 px-5 py-5" aria-label=(format!("{name}'s work"))>
            <section class="flex flex-col gap-3">
                <h2 class="text-[12px] font-medium uppercase text-fg-3">"Current work"</h2>
                if current.is_empty() {
                    <p class="text-[13px] text-fg-3">"No open assignment right now."</p>
                } else {
                    <ul class="flex flex-col gap-3">
                        for task in current { <li>task_row(task: task, name: name.clone(), live: true)</li> }
                    </ul>
                }
            </section>
            if !recent.is_empty() {
                <section class="flex flex-col gap-3 border-t border-line pt-5">
                    <h2 class="text-[12px] font-medium uppercase text-fg-3">"Recent work"</h2>
                    <ul class="flex flex-col gap-3">
                        for task in recent { <li>task_row(task: task, name: name.clone(), live: false)</li> }
                    </ul>
                </section>
            }
        </div>
    })
}

#[component]
async fn task_row(task: Ticket, name: String, live: bool) -> topcoat::Result<impl View> {
    let id = task.id.to_string();
    let running = live && matches!(task.state.kind(), "claimed" | "working");
    let tell = format!("@post('/tasks/{id}/tell', {{contentType: 'form'}}); el.reset()");
    let href = format!("/tickets/{id}");
    let line = Child::new(view! {
        <a href=(href) class="min-w-0 flex-1">
            <span class="block [overflow-wrap:anywhere]">(task.title)</span>
            <span class="block text-[12.5px] text-fg-3">(task.project.to_string()) " · " (ui::shown(&task.state))</span>
        </a>
    });
    Ok(view! {
        if running {
            <details id=(format!("person-tell-{id}")) data-preserve-attr="open" class="group">
                <summary class="flex cursor-pointer list-none items-start gap-2 [&::-webkit-details-marker]:hidden" title=(format!("Message the active run of {name}"))>
                    (line)
                    <span class="mt-1 text-fg-3 group-hover:text-fg group-open:text-fg">ui::icon(name: "message-square-text", small: true)</span>
                </summary>
                <form class="mt-2 flex items-center gap-2" data-on:submit=(tell)>
                    <input name="text" required="" autocomplete="off" aria-label=(format!("Tell the active run of {name}")) placeholder="Tell the active run" class=(format!("{} text-[13px]", ui::INPUT))>
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

    #[test]
    fn a_person_gets_their_assignments_and_current_projects_colour() {
        let office = Office {
            status: serde_json::from_value(json!({
                "projects": ["shop", "api"],
                "team": [{"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"}],
            }))
            .unwrap(),
            tickets: vec![
                serde_json::from_value(json!({
                    "id": "S-1", "project": "shop", "title": "Old", "goal": "", "hire": "dev",
                    "state": "landed", "attempts": 1, "base_branch": "main", "created_at": 1,
                }))
                .unwrap(),
                serde_json::from_value(json!({
                    "id": "A-1", "project": "api", "title": "Current", "goal": "", "hire": "dev",
                    "state": "working", "attempts": 0, "base_branch": "main", "created_at": 2,
                }))
                .unwrap(),
            ],
            ..Office::default()
        };
        let model = parts(&office, "dev", ConversationView::default()).unwrap();
        assert_eq!(model.person.name, "Dev");
        assert_eq!(model.tint, 1);
        assert_eq!(model.current[0].id.to_string(), "A-1");
        assert_eq!(model.recent[0].id.to_string(), "S-1");
        assert!(parts(&office, "nobody", ConversationView::default()).is_none());
    }

    #[test]
    fn the_composer_escapes_and_quotes_the_employee_address() {
        let send = composer_send("a'b");
        assert!(send.contains("\"/people/a%27b/talk\""), "{send}");
        assert!(!send.contains("/people/a'b/talk"), "{send}");
    }
}
