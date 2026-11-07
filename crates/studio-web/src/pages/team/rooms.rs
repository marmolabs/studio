use studiod::api::{Request, RoomSummary, RoomView, TalkFrom, TalkMessage};
use topcoat::view::{View, component, view};

use super::shell::{self, Here};
use super::talk::thread::Person;
use super::{Team, clock, faces, person_href, room_href};
use crate::pages::office::Office;
use crate::{AppConfig, client, ui};

pub async fn read(config: &AppConfig, room: &str, before: Option<u64>) -> RoomView {
    let mut view: RoomView = client::call(
        &config.socket,
        Request::Room {
            room: room.to_string(),
            after: None,
            before,
            limit: None,
        },
    )
    .await
    .unwrap_or_default();
    if view.total == 0 {
        view.total = view.messages.len() as u64;
    }
    view
}

#[component]
async fn room_link(room: RoomSummary, current: bool, now: u64) -> topcoat::Result<impl View> {
    let latest = room.last.as_ref().map(|message| {
        let text = message.text.trim();
        let head = text.chars().take(70).collect::<String>();
        if text.chars().count() > 70 {
            format!("{head}…")
        } else {
            head
        }
    });
    Ok(view! {
        <a href=(room_href(&room.id)) aria-current=(current.then_some("page")) class="block rounded-xl px-3 py-2.5 hover:bg-bg-3 aria-[current=page]:bg-bg-4">
            <span class="flex items-center gap-2">
                ui::icon(name: if room.project.is_some() { "users" } else { "building-2" }, small: true)
                <b class="min-w-0 flex-1 truncate text-[13px] font-semibold">(room.name)</b>
                if room.thinking { <i class="size-[7px] rounded-full bg-run" aria-label="Someone is replying"></i> }
            </span>
            <span class="mt-0.5 block truncate text-[12px] text-fg-3">
                if let Some(latest) = latest { (latest) } else { "No messages yet" }
            </span>
            if let Some(last) = room.last {
                <span class="mt-0.5 block text-[11px] text-fg-3">(clock::said(last.at, now))</span>
            }
        </a>
    })
}

#[component]
async fn room_message(
    message: TalkMessage,
    office: Office,
    now: u64,
) -> topcoat::Result<impl View> {
    let (name, person) = match &message.from {
        TalkFrom::Owner => ("You".to_string(), None),
        TalkFrom::Studio => ("Studio".to_string(), None),
        TalkFrom::Lead { hire } => (office.name_of(hire), Some(Person::of(&office, hire))),
    };
    let owner = message.from == TalkFrom::Owner;
    Ok(view! {
        <article class=(format!("flex gap-3 {}", if owner { "flex-row-reverse" } else { "" }))>
            if let Some(person) = person {
                <a href=(person_href(&person.hire)) aria-label=(format!("Talk to {}", person.name))>
                    faces::face(seed: person.hire.clone(), role: person.role, tint: 0, size: 32)
                </a>
            } else {
                <span class=(format!("grid size-8 shrink-0 place-items-center rounded-lg {}", if owner { "bg-primary text-on-primary" } else { "bg-bg-3 text-fg-3" }))>
                    ui::icon(name: if owner { "user-round" } else { "building-2" }, small: true)
                </span>
            }
            <div class=(format!("max-w-[76%] {}", if owner { "text-right" } else { "" }))>
                <span class="text-[12px] text-fg-3">(name) " · " (clock::said(message.at, now))</span>
                <div class=(format!("mt-1 rounded-xl px-3.5 py-2.5 text-left text-[14px] leading-5 [overflow-wrap:anywhere] {}", if owner { "bg-primary text-on-primary" } else { "border border-line bg-bg-2" }))>
                    ui::prose(text: message.text)
                </div>
            </div>
        </article>
    })
}

#[component]
pub async fn room_view(
    team: Team,
    office: Office,
    room: String,
    conversation: RoomView,
    #[default] before: Option<u64>,
) -> topcoat::Result<impl View> {
    let Some(summary) = office
        .rooms
        .iter()
        .find(|summary| summary.id == room)
        .cloned()
    else {
        return Err(client::error("no such room"));
    };
    let rooms = office.rooms.clone();
    // An example for the line under the box, from someone who can be asked.
    let example = office
        .status
        .team
        .iter()
        .find(|member| {
            !matches!(
                member.role.as_str(),
                "lead" | "approver" | "router" | "reviewer"
            )
        })
        .or_else(|| office.status.team.first())
        .map_or_else(|| "dev".to_string(), |member| member.id.clone());
    let now = office.now;
    let title = summary.name.clone();
    let project = summary.project.clone();
    let post = format!(
        "@post('{}/talk', {{contentType: 'form'}}); $_room_draft = ''",
        room_href(&room)
    );
    let earlier = if conversation.has_more {
        conversation
            .messages
            .first()
            .map(|message| format!("{}?before={}", room_href(&room), message.id))
    } else {
        None
    };
    Ok(view! {
        shell::page(team: team, here: Here::Rooms, flush: true,
            <div id="room-page" class="flex min-h-0 flex-1" data-signals="{_room_draft: ''}">
                <nav class="w-[210px] shrink-0 overflow-y-auto border-r border-line bg-bg-2 p-2 max-[760px]:hidden" aria-label="Rooms">
                    <h2 class="px-2 pb-2 pt-1 text-[11px] font-semibold uppercase tracking-[.07em] text-fg-3">"Rooms"</h2>
                    <div class="flex flex-col gap-1">
                        for room in rooms.iter() { room_link(room: room.clone(), current: room.id == summary.id, now: now) }
                    </div>
                </nav>
                <section class="flex min-h-0 min-w-0 flex-1 flex-col" aria-label=(title.clone())>
                    <nav class="hidden gap-2 overflow-x-auto [scrollbar-width:none] border-b border-line px-4 py-2.5 max-[760px]:flex" aria-label="Rooms">
                        for other in rooms.iter() {
                            <a href=(room_href(&other.id)) aria-current=((other.id == summary.id).then_some("page")) class="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full border border-line-2 px-3 text-[13px] text-fg-2 aria-[current=page]:border-primary aria-[current=page]:bg-bg-4 aria-[current=page]:text-fg">
                                ui::icon(name: if other.project.is_some() { "users" } else { "building-2" }, small: true)
                                (other.name.clone())
                            </a>
                        }
                    </nav>
                    <header class="flex flex-wrap items-center gap-3 border-b border-line px-5 py-3.5">
                        <span class="grid size-9 place-items-center rounded-xl bg-bg-3 text-fg-2">ui::icon(name: if project.is_some() { "users" } else { "building-2" })</span>
                        <div class="min-w-0 flex-1">
                            <h1 class="truncate text-[16px] font-semibold">(title)</h1>
                            <p class="m-0 truncate text-[12.5px] text-fg-3">
                                (if let Some(project) = project { format!("Project room · {project}") } else { "Everyone across the company".to_string() })
                            </p>
                        </div>
                        <span class="text-[12px] text-fg-3">(conversation.total) if conversation.total == 1 { " message" } else { " messages" }</span>
                    </header>
                    <div id="room-thread" class="talk-thread">
                        <div class="mx-auto flex w-full max-w-[780px] flex-col gap-5 px-5 py-6">
                            if let Some(earlier) = earlier {
                                <a href=(earlier) class=(ui::BUTTON_SM)>"Earlier messages"</a>
                            }
                            if conversation.messages.is_empty() {
                                <div class="py-12 text-center">
                                    <b class="block text-[15px] font-semibold">"Start the conversation"</b>
                                    <p class="m-0 mt-1 text-[13px] text-fg-3">"Share an announcement, or write @ and someone's id to ask them to reply."</p>
                                </div>
                            }
                            for item in conversation.messages { room_message(message: item, office: office.clone(), now: now) }
                            if before.is_some() {
                                <a href=(room_href(&room)) class=(ui::BUTTON_SM)>"Back to the latest messages"</a>
                            }
                            if conversation.thinking {
                                <div id="room-replying" class="flex items-center gap-2 text-[13px] text-fg-3" role="status">
                                    <i class="size-2 animate-pulse rounded-full bg-run"></i> "A mentioned employee is replying…"
                                </div>
                            }
                        </div>
                    </div>
                    <form id="room-composer" class="border-t border-line px-5 pb-4 pt-3" data-on:submit=(format!("if ($_room_draft.trim()) {{ {post} }}"))>
                        <div class="mx-auto w-full max-w-[780px]">
                            <label class="flex items-center gap-2 rounded-xl border border-line-2 bg-bg-2 py-1.5 pl-3.5 pr-1.5 focus-within:border-ring">
                                <span class="sr-only">"Message the room"</span>
                                <input name="text" data-bind:_room_draft="" required="" autocomplete="off" placeholder="Write to the room" class="h-8 min-w-0 flex-1 bg-transparent text-[14px] outline-none placeholder:text-fg-3">
                                <button type="submit" class=(ui::PRIMARY) aria-label="Send">ui::icon(name: "send")</button>
                            </label>
                            <p class="m-0 mt-1.5 text-[11.5px] text-fg-3">
                                "To ask someone to reply, write @ and their id, like @" (example) ". An announcement alone starts no work."
                            </p>
                        </div>
                    </form>
                </section>
            </div>
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_read_through_the_persistent_room_api() {
        let request = Request::Room {
            room: "my shop".into(),
            after: None,
            before: Some(10),
            limit: Some(50),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"cmd":"room","room":"my shop","after":null,"before":10,"limit":50})
        );
    }
}
