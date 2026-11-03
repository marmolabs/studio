//! The thread's things to read: the owner's bubbles, the lead's lines, and the
//! cards under them.

use topcoat::view::{View, ViewExt, component, view};

use super::thread::{Acted, Card, Item, Plan, Question, Release, Ship, Stats, Voice};
use crate::pages::team::{clock, faces, lead_href, new, version};
use crate::pages::timeline;
use crate::ui;

/// The id of the field the owner writes in. The form around it is `#composer`
/// and the field is named `text`, which is what `fill` looks for.
pub const DRAFT: &str = "draft";

/// The expression that puts `text` in the composer, with the cursor after it,
/// and moves to it. The field tells its binding that it changed, so what the
/// owner sends is what it shows.
pub fn fill(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_default();
    format!(
        "const c = document.querySelector('#composer [name=text]'); \
         if (c) {{ c.value = {quoted}; c.dispatchEvent(new Event('input', {{bubbles: true}})); \
         c.focus(); c.setSelectionRange(c.value.length, c.value.length) }}"
    )
}

/// An address that opens the sheet of `look` over `project`'s conversation.
pub fn look_href(project: &str, look: &str) -> String {
    format!("{}?look={look}", lead_href(project))
}

const CARD: &str = "flex min-w-0 flex-col gap-3 rounded-xl border p-3.5";
const WHAT: &str = "flex min-w-0 flex-1 items-center gap-3";
const ICON_TILE: &str = "grid size-7 shrink-0 place-items-center rounded-lg";

/// When it was said, as the owner's clock has it: "9:15" for today, the date
/// before that. Its tooltip says how long ago and when exactly.
#[component]
async fn stamp(at: u64, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <time class="text-[12.5px] text-fg-3" datetime=(timeline::exact(at)) title=(clock::in_full(at, now))>
            (clock::said(at, now))
        </time>
    })
}

/// One thing in the thread: the owner's words as a bubble on the right, the
/// lead's (or the studio's) as a line with a face and what shows under it.
#[component]
pub async fn entry(
    item: Item,
    project: String,
    tint: usize,
    now: u64,
) -> topcoat::Result<impl View> {
    let Item {
        id,
        at,
        stamped,
        voice,
        text,
        cards,
        follows,
    } = item;
    // What the label says: when it was said, not where the thread puts it.
    let at = stamped.unwrap_or(at);
    Ok(view! {
        if matches!(voice, Voice::Owner) {
            <div id=(id) class="flex flex-col items-end gap-1">
                <p class="max-w-[70%] whitespace-pre-wrap rounded-2xl rounded-br-sm border border-line-2 bg-bg-4 px-3.5 py-2.5 [overflow-wrap:anywhere] max-[760px]:max-w-[88%]">(text)</p>
                <span class="text-[12.5px] text-fg-3">"You · " stamp(at: at, now: now)</span>
            </div>
        } else {
            <div id=(id) class=(if follows { "-mt-2 flex items-start gap-3" } else { "flex items-start gap-3" })>
                if follows {
                    <span class="w-[30px] shrink-0"></span>
                } else {
                    speaker(voice: voice.clone(), tint: tint)
                }
                <div class="flex min-w-0 flex-1 flex-col gap-2.5">
                    if !follows {
                        <span class="flex items-baseline gap-2">
                            <b class="font-semibold">(name(&voice))</b>
                            stamp(at: at, now: now)
                        </span>
                    }
                    if !text.is_empty() {
                        <p class="whitespace-pre-wrap [overflow-wrap:anywhere]">ui::prose(text: text)</p>
                    }
                    for card in cards {
                        shown(card: card, project: project.clone(), tint: tint)
                    }
                </div>
            </div>
        }
    })
}

/// What a speaker is called in the thread.
fn name(voice: &Voice) -> String {
    match voice {
        Voice::Owner => "You".to_string(),
        Voice::Lead(person) => person.name.clone(),
        Voice::Studio => "Studio".to_string(),
    }
}

/// The face of whoever speaks; the studio has its mark.
#[component]
async fn speaker(voice: Voice, tint: usize) -> topcoat::Result<impl View> {
    Ok(view! {
        if let Voice::Lead(person) = voice {
            faces::face(seed: person.hire, role: person.role, tint: tint, size: 30)
        } else {
            <span class="grid size-[30px] shrink-0 place-items-center rounded-full bg-primary text-on-primary" aria-hidden="true">
                <svg class="size-4"><use href="#logo"></use></svg>
            </span>
        }
    })
}

/// The card under a message. Each kind is drawn from a box of its own, so the
/// page holds only the card that is there, not room for every kind.
#[component]
async fn shown(card: Card, project: String, tint: usize) -> topcoat::Result<impl View> {
    Ok(match card {
        Card::Plan(plan) => view! { plan_card(plan: plan, project: project, tint: tint) }.boxed(),
        Card::Acted(acted) => view! { acted_card(acted: acted) }.boxed(),
        Card::Setup { view, lead } => {
            view! { new::setup_card(view: *view, lead_name: lead) }.boxed()
        }
        Card::Ask(question) => view! { ask_card(question: question) }.boxed(),
        Card::Ship(ship) => view! { ship_card(ship: ship, project: project) }.boxed(),
        Card::Release(release) => view! { release_card(release: release) }.boxed(),
        Card::Stats(stats) => view! { stats_card(stats: stats) }.boxed(),
    })
}

/// A goal the lead planned: its tasks with who has them, and what the owner
/// can say to it.
#[component]
async fn plan_card(plan: Plan, project: String, tint: usize) -> topcoat::Result<impl View> {
    let Plan {
        title,
        version: milestone,
        tasks,
    } = plan;
    let count = tasks.len();
    let approve = format!(
        "@post('{}/talk', {{contentType: 'form'}})",
        lead_href(&project)
    );
    let name = match milestone {
        Some(v) => format!("{} {title}", version(&v)),
        None => title,
    };
    Ok(view! {
        <section class=(format!("{CARD} border-line bg-bg-2")) aria-label="A plan">
            <div class="flex items-center gap-3">
                <span class=(format!("{ICON_TILE} bg-run-bg text-run-fg"))>ui::icon(name: "list-checks")</span>
                <span class="min-w-0">
                    <b class="block font-semibold">(name)</b>
                    <span class="text-[12.5px] text-fg-3">(count) if count == 1 { " task" } else { " tasks" }</span>
                </span>
            </div>
            <ul class="flex flex-col gap-2 pl-10">
                for task in tasks {
                    <li class="flex items-center gap-2.5">
                        if let Some(who) = task.who {
                            faces::face(seed: who.hire, role: who.role, tint: tint, size: 24)
                        }
                        <span class="min-w-0 flex-1">
                            <span class="block truncate">(task.title)</span>
                            if !task.result.is_empty() {
                                <span class="block truncate text-[12.5px] text-fg-3" title=(task.result.clone())>(task.result.clone())</span>
                            }
                        </span>
                        <span class="text-[12.5px] text-fg-3">(task.status)</span>
                    </li>
                }
            </ul>
            <form class="flex flex-wrap items-center gap-2 pl-10" data-on:submit=(approve)>
                <input type="hidden" name="text" value="Looks good.">
                <button type="submit" class=(ui::BUTTON_SM)>"Looks good"</button>
                <button type="button" class=(ui::GHOST_SM) data-on:click=(fill("Change the plan: "))>"Change it"</button>
            </form>
        </section>
    })
}

/// One line on what the lead did to a task: all the thread says of it.
#[component]
async fn acted_card(acted: Acted) -> topcoat::Result<impl View> {
    let Acted {
        title,
        action,
        note,
    } = acted;
    Ok(view! {
        <p class="flex min-w-0 items-center gap-2 text-fg-2">
            ui::icon(name: "corner-down-right", small: true)
            <span class="min-w-0 truncate">
                if action == "told" {
                    "Told the agent on " <b class="font-medium text-fg">(title)</b>
                } else {
                    <b class="font-medium text-fg">(title)</b> " was " (action)
                }
                if !note.is_empty() { ": " (note) }
            </span>
        </p>
    })
}

/// Something that waits for the owner's answer.
#[component]
async fn ask_card(question: Question) -> topcoat::Result<impl View> {
    let Question {
        ask,
        ticket,
        title,
        line,
        plain,
    } = question;
    let answer = format!("@post('/asks/{ask}', {{contentType: 'form'}})");
    Ok(view! {
        <section id=(format!("ask-{ask}")) class=(format!("{CARD} border-attn-line bg-attn-bg sm:flex-row sm:items-center")) aria-label="Waiting for you">
            <div class=(WHAT)>
                <span class=(format!("{ICON_TILE} bg-attn text-on-primary"))>ui::icon(name: "circle-alert")</span>
                <span class="min-w-0">
                    <b class="block font-semibold">(title)</b>
                    <span class="block truncate text-[12.5px] text-fg-3">(line)</span>
                </span>
            </div>
            if plain {
                <form class="flex shrink-0 gap-2" data-on:submit=(answer)>
                    <button type="submit" name=(crate::pages::ask_card::DECISION) value="approve" class=(ui::PRIMARY_SM)>"Allow"</button>
                    <button type="submit" name=(crate::pages::ask_card::DECISION) value="deny" class=(ui::BUTTON_SM)>"Deny"</button>
                </form>
            } else {
                <a href=(format!("/tickets/{ticket}")) class=(ui::PRIMARY_SM)>"Answer"</a>
            }
        </section>
    })
}

/// Work that is ready to ship: ship it, or look closer first.
#[component]
async fn ship_card(ship: Ship, project: String) -> topcoat::Result<impl View> {
    let notes = ship.notes();
    let failed = notes
        .first()
        .is_some_and(|n| n.starts_with("checks failed"));
    let post = format!("@post('{}')", ship.post);
    let Ship {
        look,
        title,
        lines,
        result,
        ..
    } = ship;
    Ok(view! {
        <section id=(format!("ship-{look}")) class=(format!("{CARD} border-ok-line bg-ok-bg sm:flex-row sm:items-center")) aria-label="Ready to ship">
            <div class=(WHAT)>
                <span class=(format!("{ICON_TILE} bg-ok text-on-primary"))>ui::icon(name: "git-merge")</span>
                <span class="min-w-0">
                    <b class="block font-semibold">"Ready to ship"</b>
                    if !result.is_empty() {
                        <span class="block truncate text-[12.5px] text-fg-2" title=(result.clone())>(result.clone())</span>
                    }
                    <span class="block text-[12.5px] text-fg-3">
                        <span class="truncate">(title)</span>
                        if let Some((added, removed)) = lines {
                            " · " <span class=(ui::ADD)>"+" (added)</span> " " <span class=(ui::DEL)>"−" (removed)</span>
                        }
                        if !notes.is_empty() {
                            " · " <span class=(if failed { "text-bad-fg" } else { "" })>(notes.join(" · "))</span>
                        }
                    </span>
                </span>
            </div>
            <div class="flex shrink-0 gap-2">
                <button type="button" class=(ui::PRIMARY_SM) data-on:click=(post)>
                    if failed { "Ship anyway" } else { "Ship" }
                </button>
                <a href=(look_href(&project, &look)) class=(ui::BUTTON_SM)>"Look closer"</a>
            </div>
        </section>
    })
}

/// A release that waits for the owner's go-ahead.
#[component]
async fn release_card(release: Release) -> topcoat::Result<impl View> {
    let Release {
        project,
        version: v,
    } = release;
    let publish = "@post('/office/releases/approve', {contentType: 'form'})";
    let hold = "@post('/office/releases/cancel', {contentType: 'form'})";
    Ok(view! {
        <section id=(format!("release-{project}")) class=(format!("{CARD} border-attn-line bg-attn-bg sm:flex-row sm:items-center")) aria-label="A release waits for you">
            <div class=(WHAT)>
                <span class=(format!("{ICON_TILE} bg-attn text-on-primary"))>ui::icon(name: "circle-alert")</span>
                <span class="min-w-0">
                    <b class="block font-semibold">"Publish " (version(&v)) "?"</b>
                    <span class="block text-[12.5px] text-fg-3">"The final checks passed. It goes out when you say."</span>
                </span>
            </div>
            <div class="flex shrink-0 gap-2">
                <form data-on:submit=(publish)>
                    <input type="hidden" name="project" value=(project.clone())>
                    <input type="hidden" name="version" value=(v.clone())>
                    <button type="submit" class=(ui::PRIMARY_SM)>"Publish"</button>
                </form>
                <form data-on:submit=(hold)>
                    <input type="hidden" name="project" value=(project)>
                    <button type="submit" class=(ui::BUTTON_SM)>"Not yet"</button>
                </form>
            </div>
        </section>
    })
}

/// The day's four numbers.
#[component]
async fn stats_card(stats: Stats) -> topcoat::Result<impl View> {
    let Stats {
        done,
        working,
        sent_back,
        for_you,
    } = stats;
    Ok(view! {
        <div class="grid grid-cols-4 gap-2 max-[760px]:grid-cols-2" aria-label="Today">
            tile(n: done, words: "done", tone: "text-ok-fg")
            tile(n: working, words: "working", tone: "text-run-fg")
            tile(n: sent_back, words: "sent back", tone: "text-[var(--rev)]")
            tile(n: for_you, words: "for you", tone: "text-attn-fg")
        </div>
    })
}

#[component]
async fn tile(n: usize, words: &'static str, tone: &'static str) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="flex flex-col gap-0.5 rounded-xl border border-line bg-bg-2 px-3.5 py-3">
            <b class=(format!("text-[22px] font-semibold leading-7 tabular-nums {tone}"))>(n)</b>
            <span class="text-[12.5px] text-fg-3">(words)</span>
        </span>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{named, text_of};
    use topcoat::context::Cx;

    fn said(text: &str) -> Item {
        Item {
            follows: false,
            stamped: None,
            id: "m-1".to_string(),
            at: 1_000,
            voice: Voice::Studio,
            text: text.to_string(),
            cards: Vec::new(),
        }
    }

    async fn drawn(item: Item) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => entry(item: item, project: "shop".to_string(), tint: 0, now: 2_000) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_lead_names_a_file_or_a_command_and_it_reads_as_code() {
        let html = drawn(said("The test is in `test.sh`; run `sh ./test.sh`.")).await;
        let code: Vec<String> = named(&html, "code").iter().map(|c| c.text()).collect();
        assert_eq!(code, ["test.sh", "sh ./test.sh"]);
        // No backtick is left in what the owner reads, and the words around the
        // code run on without a gap.
        assert_eq!(text_of(&html).matches('`').count(), 0, "{html}");
        assert!(html.contains("The test is in <code"), "{html}");
        assert!(html.contains("</code>; run <code"), "{html}");
        assert!(html.contains("</code>.</p>"), "{html}");
    }
}
