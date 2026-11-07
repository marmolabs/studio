//! Giving a project a lead from its conversation. A project without one has a
//! card where the message box would be, and one that has a lead has a small
//! menu in its header to change it. Both do the same three things: hire a new
//! lead, pick one the studio has, or (the menu only) leave the project with
//! none. They post to the routes below and to `/office/projects/lead`, which
//! names or clears a lead. The conversation then turns into one with the new
//! lead as the next patch of the page comes, so nothing here redraws it.
//!
//! What the owner types in either is theirs: each is under `data-ignore-morph`,
//! so a patch of the page leaves it as it is. The menu's id has its lead in
//! it, so that once the lead changes the page puts a fresh one in its place.
//! What the hire forms are told in reply is said under their name field (the
//! page's `_said` and `_bad` signals), as a message over a card at the bottom
//! of the page would cover the form.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use studiod::api::{FileKind, NewLead, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchSignals,
    router::{
        content::Form,
        response::{IntoResponse, Response},
        route,
    },
    view::{View, component, view},
};

use crate::pages::team::new::{Hiring, LEAD_MODEL};
use crate::{AppConfig, client, ui};

/// Where a form that names or clears a project's lead posts.
const SET_LEAD: &str = "/office/projects/lead";
/// Where a form that hires a lead for a project posts.
const HIRE_LEAD: &str = "/office/projects/lead/hire";

/// What a form does as it is submitted: posts its fields to `path`.
fn post(path: &str) -> String {
    format!("@post('{path}', {{contentType: 'form'}})")
}

/// The options of a choice of leads, by (id, name).
#[component]
async fn lead_options(leads: Vec<(String, String)>) -> topcoat::Result<impl View> {
    Ok(view! {
        for (id, name) in leads {
            <option value=(id)>(name)</option>
        }
    })
}

/// Where a hire form is told what came of it: the page's `_said`, red when
/// `_bad`. It is gone again as the owner changes the name.
#[component]
async fn said_here() -> topcoat::Result<impl View> {
    Ok(view! {
        <p class="m-0 text-[12.5px] leading-[18px]" style="display: none" role="status" data-show="$_said" data-text="$_said" data-class:text-bad-fg="$_bad" data-class:text-fg-3="!$_bad"></p>
    })
}

/// What stands where the message box would be for a project that has no lead:
/// what a lead is for, and two ways to give it one. A way that has nothing to
/// pick from, no lead hire in the studio, is left out.
#[component]
pub async fn no_lead(project: String, hiring: Hiring) -> topcoat::Result<impl View> {
    let Hiring { suggestion, leads } = hiring;
    let pick = !leads.is_empty();
    let columns = if pick { "min-[761px]:grid-cols-2" } else { "" };
    Ok(view! {
        <section id="lead-card" class="shrink-0 border-t border-line bg-bg px-7 py-4 max-[760px]:px-4 max-[760px]:py-3" aria-label=(format!("Give {project} a lead"))>
            <div class="mx-auto flex w-full max-w-[780px] flex-col gap-4 rounded-xl border border-line-2 bg-bg-2 p-4 max-[760px]:gap-3">
                <p class="m-0 flex items-start gap-3 text-[14px] leading-6">
                    <span class="grid size-7 shrink-0 place-items-center rounded-lg bg-run-bg text-run-fg">ui::icon(name: "users")</span>
                    <span class="min-w-0 [overflow-wrap:anywhere]">
                        <b class="font-semibold">(project.clone()) " has no lead yet."</b>
                        " A lead plans the work, hands it out and checks it."
                    </span>
                </p>
                <div class=(format!("grid gap-5 max-[760px]:gap-3 {columns}"))>
                    <form id="hire-lead" data-ignore-morph="" class="flex min-w-0 flex-col gap-3" data-on:submit=(post(HIRE_LEAD))>
                        <input type="hidden" name="project" value=(project.clone())>
                        <h2 class=(ui::SECTION_TITLE)>"Hire a lead"</h2>
                        <div class=(ui::FIELD)>
                            <label for="hire-lead-name" class=(ui::FIELD_LABEL)>"Its name"</label>
                            <input id="hire-lead-name" name="name" required="" maxlength="40" autocomplete="off" spellcheck="false" value=(suggestion) class=(ui::INPUT) data-on:input="$_said = ''">
                            <span class=(ui::HINT)>"Suggested. A new hire on a strong model (Opus)."</span>
                            said_here()
                        </div>
                        <div class="mt-auto"><button type="submit" class=(ui::PRIMARY)>"Hire"</button></div>
                    </form>
                    if pick {
                        <form id="pick-lead" data-ignore-morph="" class="flex min-w-0 flex-col gap-3 max-[760px]:border-t max-[760px]:border-line max-[760px]:pt-3 min-[761px]:border-l min-[761px]:border-line min-[761px]:pl-5" data-on:submit=(post(SET_LEAD))>
                            <input type="hidden" name="project" value=(project)>
                            <h2 class=(ui::SECTION_TITLE)>"Pick one you have"</h2>
                            <div class=(ui::FIELD)>
                                <label for="pick-lead-hire" class=(ui::FIELD_LABEL)>"Which lead"</label>
                                <select id="pick-lead-hire" name="hire" class=(ui::INPUT)>
                                    lead_options(leads: leads)
                                </select>
                            </div>
                            <div class="mt-auto"><button type="submit" class=(ui::BUTTON)>"Make it the lead"</button></div>
                        </form>
                    }
                </div>
            </div>
        </section>
    })
}

/// The menu in the header of a conversation that changes its lead: another
/// lead the studio has, a new hire, or no lead at all. It stays open, with
/// what is typed in it, as the page is patched, and closes when the owner
/// clicks elsewhere or presses Escape.
#[component]
pub async fn change_lead(
    project: String,
    lead: String,
    hiring: Hiring,
) -> topcoat::Result<impl View> {
    let Hiring { suggestion, leads } = hiring;
    let others: Vec<(String, String)> = leads.into_iter().filter(|(id, _)| *id != lead).collect();
    let swap = !others.is_empty();
    // The lead is in the id: once it changes, this menu is gone and a fresh one
    // takes its place.
    let id = format!("change-lead-{lead}");
    let leave = format!(
        "confirm({}) && {}",
        serde_json::to_string(&format!(
            "Leave {project} without a lead? Nobody will plan its work or answer for it until you give it one."
        ))
        .unwrap_or_default(),
        post(SET_LEAD)
    );
    Ok(view! {
        <details id=(id) data-ignore-morph="" class="group relative ml-auto" data-on:toggle="$_said = ''" data-on:click__outside="el.open = false" data-on:keydown__window="evt.key === 'Escape' && (el.open = false)">
            <summary class=(format!("{} list-none group-open:bg-bg-3 [&::-webkit-details-marker]:hidden", ui::BUTTON_SM))>
                ui::icon(name: "users", small: true)
                "Change lead"
            </summary>
            <div class="absolute right-0 top-[calc(100%+6px)] z-20 flex w-[min(360px,calc(100vw-32px))] flex-col gap-4 rounded-xl border border-line-2 bg-bg-2 p-4 [box-shadow:var(--pop)]">
                if swap {
                    <form id="swap-lead" class="flex flex-col gap-1.5" data-on:submit=(post(SET_LEAD))>
                        <input type="hidden" name="project" value=(project.clone())>
                        <label for="swap-lead-hire" class=(ui::FIELD_LABEL)>"Another lead"</label>
                        <div class="flex gap-2">
                            <select id="swap-lead-hire" name="hire" class=(ui::INPUT)>
                                lead_options(leads: others)
                            </select>
                            <button type="submit" class=(ui::BUTTON)>"Use"</button>
                        </div>
                    </form>
                }
                <form id="hire-another-lead" class="flex flex-col gap-1.5" data-on:submit=(post(HIRE_LEAD))>
                    <input type="hidden" name="project" value=(project.clone())>
                    <label for="hire-another-lead-name" class=(ui::FIELD_LABEL)>"A new hire"</label>
                    <div class="flex gap-2">
                        <input id="hire-another-lead-name" name="name" required="" maxlength="40" autocomplete="off" spellcheck="false" value=(suggestion) class=(ui::INPUT) data-on:input="$_said = ''">
                        <button type="submit" class=(ui::BUTTON)>"Hire"</button>
                    </div>
                    <span class=(ui::HINT)>"On a strong model (Opus)."</span>
                    said_here()
                </form>
                <form id="leave-lead" class="flex flex-col gap-1.5" data-on:submit=(leave)>
                    <input type="hidden" name="project" value=(project)>
                    <input type="hidden" name="hire" value="">
                    <span class=(ui::FIELD_LABEL)>"No lead"</span>
                    <div><button type="submit" class=(ui::DANGER)>"Leave it without a lead"</button></div>
                </form>
            </div>
        </details>
    })
}

/// What a form that hires a lead posts.
#[derive(Deserialize)]
struct HireLead {
    /// The project to give a lead.
    #[serde(default)]
    project: String,
    /// What to call the new lead.
    #[serde(default)]
    name: String,
}

/// A refusal as a sentence: its first letter in capital, and a full stop.
fn sentence(why: &str) -> String {
    let why = why.trim();
    let mut chars = why.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut said: String = first.to_uppercase().chain(chars).collect();
    if !said.ends_with(['.', '!', '?']) {
        said.push('.');
    }
    said
}

/// Hires a lead for a project and makes it the project's lead, and tells the
/// form what came of it, under its name field.
#[route(POST "/office/projects/lead/hire")]
async fn hire_lead(cx: &Cx, Form(form): Form<HireLead>) -> topcoat::Result<Response> {
    let config = app_context::<AppConfig>(cx);
    let (said, bad) = match hire(&config.socket, &form).await {
        Ok(said) => (said, false),
        Err(why) => (why, true),
    };
    PatchSignals::json(&json!({"_said": said, "_bad": bad}))?.into_response(cx)
}

/// Makes the lead `form` asks for: the hire's file first, as the settings' hire
/// form writes one, then the project's lead. A hire that can't be made its
/// project's lead is let go, so a refusal leaves nothing behind. The new lead
/// is on a strong model and has the tool it needs to plan goals, as one hired
/// from New project does. Answers what to tell the owner, or why it stopped.
async fn hire(socket: &Path, form: &HireLead) -> Result<String, String> {
    let project = form.project.trim();
    if project.is_empty() {
        return Err("Pick a project.".to_string());
    }
    let new = NewLead {
        name: form.name.trim().to_string(),
        model: LEAD_MODEL.to_string(),
    };
    let (id, text) = new.hire_file().map_err(|why| sentence(&why))?;
    let status: client::Status = client::call(socket, Request::Status)
        .await
        .map_err(|err| err.to_string())?;
    if !status.projects.iter().any(|name| name == project) {
        return Err(format!("The studio has no project {project}."));
    }
    let name = &new.name;
    let taken = status
        .team
        .iter()
        .any(|member| member.id == id || member.name.trim().to_lowercase() == name.to_lowercase());
    if taken {
        return Err(format!(
            "A hire called {name} already exists. Pick another name."
        ));
    }
    let file = Request::SaveFile {
        kind: FileKind::Hires,
        name: id.clone(),
        text,
    };
    client::call::<Value>(socket, file)
        .await
        .map_err(|err| err.to_string())?;
    let lead = Request::SetProjectLead {
        project: project.to_string(),
        hire: Some(id.clone()),
    };
    if let Err(err) = client::call::<Value>(socket, lead).await {
        let undo = Request::DeleteFile {
            kind: FileKind::Hires,
            name: id,
        };
        // Nothing more can be done if this fails too.
        let _ = client::call::<Value>(socket, undo).await;
        return Err(err.to_string());
    }
    Ok(format!("Hired {name} to lead {project}."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{by_id, named, text_of};

    fn hiring(leads: &[(&str, &str)]) -> Hiring {
        Hiring {
            suggestion: "Mira".into(),
            leads: leads
                .iter()
                .map(|(id, name)| (id.to_string(), name.to_string()))
                .collect(),
        }
    }

    async fn card(leads: &[(&str, &str)]) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => no_lead(project: "shop".to_string(), hiring: hiring(leads)) },
        )
        .await
        .unwrap()
    }

    async fn menu(lead: &str, leads: &[(&str, &str)]) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => change_lead(project: "shop".to_string(), lead: lead.to_string(), hiring: hiring(leads)) },
        )
        .await
        .unwrap()
    }

    #[test]
    fn a_refusal_reads_as_a_sentence() {
        assert_eq!(
            sentence("give the lead a short name"),
            "Give the lead a short name."
        );
        assert_eq!(sentence(" Already a sentence. "), "Already a sentence.");
        assert_eq!(sentence("Is it?"), "Is it?");
        assert_eq!(sentence(""), "");
    }

    #[tokio::test]
    async fn the_card_says_what_a_lead_is_for_and_has_a_way_to_hire_one() {
        let html = card(&[]).await;
        let text = text_of(&html);
        assert!(
            text.contains(
                "shop has no lead yet. A lead plans the work, hands it out and checks it."
            ),
            "{text}"
        );
        // With no lead in the studio to pick, hiring is the one way.
        assert!(text.contains("Hire a lead"), "{text}");
        assert!(!text.contains("Pick one you have"), "{text}");
        let form = by_id(&html, "hire-lead");
        assert!(form.has("data-ignore-morph"));
        assert!(form.attr_has("data-on:submit", "@post('/office/projects/lead/hire'"));
        let name = named(form.inner_html(), "input")
            .into_iter()
            .find(|input| input.attr_is("name", "name"))
            .unwrap();
        assert_eq!(name.attr("value").as_deref(), Some("Mira"));
        assert!(name.has("required"));
    }

    #[tokio::test]
    async fn the_card_offers_the_leads_the_studio_has_to_pick() {
        let html = card(&[("nora", "Nora"), ("theo", "Theo")]).await;
        let form = by_id(&html, "pick-lead");
        assert!(form.has("data-ignore-morph"));
        assert!(form.attr_has("data-on:submit", "@post('/office/projects/lead'"));
        let options: Vec<(String, String)> = named(form.inner_html(), "option")
            .iter()
            .map(|o| (o.attr("value").unwrap(), o.text()))
            .collect();
        assert_eq!(
            options,
            [
                ("nora".to_string(), "Nora".to_string()),
                ("theo".to_string(), "Theo".to_string())
            ]
        );
        assert!(text_of(&html).contains("Pick one you have"));
    }

    #[tokio::test]
    async fn the_menu_offers_another_lead_a_new_hire_or_none_and_is_the_owners_to_keep() {
        let html = menu("nora", &[("nora", "Nora"), ("theo", "Theo")]).await;
        // The lead is in its id, so a menu for another lead replaces it.
        let details = by_id(&html, "change-lead-nora");
        assert_eq!(details.name, "details");
        assert!(details.has("data-ignore-morph"));
        // The lead it has is not offered again.
        let swap = by_id(&html, "swap-lead");
        let options: Vec<String> = named(swap.inner_html(), "option")
            .iter()
            .map(|o| o.attr("value").unwrap())
            .collect();
        assert_eq!(options, ["theo"]);
        assert!(swap.attr_has("data-on:submit", "@post('/office/projects/lead'"));
        let hire = by_id(&html, "hire-another-lead");
        assert!(hire.attr_has("data-on:submit", "@post('/office/projects/lead/hire'"));
        // No lead: the same route with no hire, after asking.
        let leave = by_id(&html, "leave-lead");
        assert!(leave.attr_has("data-on:submit", "confirm("));
        assert!(leave.attr_has("data-on:submit", "@post('/office/projects/lead'"));
        let hire_field = named(leave.inner_html(), "input")
            .into_iter()
            .find(|input| input.attr_is("name", "hire"))
            .unwrap();
        assert_eq!(hire_field.attr("value").as_deref(), Some(""));
        let text = text_of(&html);
        for words in ["Change lead", "Another lead", "A new hire", "No lead"] {
            assert!(text.contains(words), "{words}: {text}");
        }
        // Nobody else to pick: the choice of another lead is left out.
        let alone = menu("nora", &[("nora", "Nora")]).await;
        assert!(!text_of(&alone).contains("Another lead"), "{alone}");
    }
}
