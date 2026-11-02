//! The panels of what the company floor draws: the owner's office, the
//! lounge, the free seat, review, the checks racks and the spot for a new
//! project.

use studiod::api::ProjectCheck;
use topcoat::context::Cx;
use topcoat::view::{BoxView, View, ViewExt, component, view};

use super::company::hire_view;
use super::needs::needs_pane;
use super::queues::{lounge_view, review_view};
use super::{Extra, line, quiet, section, tint};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::inspector::{self};
use crate::pages::office::model::Element;
use crate::pages::office::{Office, select};
use crate::pages::timeline::relative;
use crate::ui;

/// An element's view, under a header with its name and a way back.
pub(super) fn element_view(cx: &Cx, element: Element, office: Office, extra: Extra) -> BoxView<'_> {
    let (title, icon) = match element {
        Element::Owner => ("For you", "inbox"),
        Element::Lounge => ("Lounge", "coffee"),
        Element::Hire => ("Hire someone", "users"),
        Element::Review => ("Review", "eye"),
        Element::Racks => ("Checks", "flask-conical"),
        Element::AddProject => ("Add a project", "plus"),
    };
    let body: BoxView = match element {
        Element::Owner => view! { cx => needs_pane(office: office) }.boxed(),
        Element::Lounge => view! { cx => lounge_view(office: office) }.boxed(),
        Element::Hire => view! { cx => hire_view() }.boxed(),
        Element::Review => view! { cx => review_view(office: office) }.boxed(),
        Element::Racks => view! { cx => racks_view(office: office, checks: extra.checks) }.boxed(),
        Element::AddProject => view! { cx => add_project_view(office: office) }.boxed(),
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

/// The form that adds a project, and the projects there are now.
#[component]
async fn add_project_view(office: Office) -> topcoat::Result<impl View> {
    let projects: Vec<(String, usize)> = office
        .status
        .projects
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), facts::wrap(i)))
        .collect();
    let note = match projects.len() {
        0 => String::new(),
        n => n.to_string(),
    };
    Ok(view! {
        // Coming back to this panel starts from the form, not from a check
        // made earlier.
        <span hidden="" data-init="$_found.path = ''"></span>
        quiet(text: "Pick the folder of a git repository on this computer. It gets a rack of its own in the checks lab.")
        // The first submit only checks. What studiod says would be added
        // fills `_found`, which swaps the form for the card below; the card's
        // button then posts the same form again to add it.
        <form id="add-project" class="flex flex-col gap-3" data-show="!$_found.path" data-on:submit="@post('/office/projects', {contentType: 'form'})">
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
        <div id="add-project-found" class="flex flex-col gap-3" style="display: none" data-show="!!$_found.path" aria-live="polite">
            <p class="m-0 text-[13px] leading-5 text-fg-2">
                "This adds " <b class="font-medium text-fg" data-text="$_found.name"></b> " to the studio."
            </p>
            <dl class="m-0 flex flex-col gap-2 text-[13px]">
                <div>
                    <dt class="text-[12px] text-fg-3">"Folder"</dt>
                    <dd class="m-0 break-all font-mono text-[12.5px]" data-text="$_found.path"></dd>
                </div>
                <div>
                    <dt class="text-[12px] text-fg-3">"Work starts from the branch"</dt>
                    <dd class="m-0 font-mono text-[12.5px]" data-text="$_found.branch"></dd>
                </div>
                <div class="text-fg-2" data-text="$_found.checks"></div>
            </dl>
            <div class=(inspector::ACTIONS)>
                <button type="button" class=(ui::PRIMARY) data-on:click="@post('/office/projects?add=yes', {contentType: 'form', selector: '#add-project'}); $_found.path = ''">
                    ui::icon(name: "plus")
                    "Add it"
                </button>
                <button type="button" class=(ui::BUTTON) data-on:click="$_found.path = ''">"Cancel"</button>
            </div>
        </div>
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

#[cfg(test)]
mod tests {
    use crate::pages::office::panel::fixtures::{extra, html_of, office};
    use crate::probe::text_of;

    #[tokio::test]
    async fn the_owners_office_is_what_waits_for_you() {
        let html = html_of(office(), extra(), "elements/owner").await;
        let text = text_of(&html);
        assert!(text.contains("Publish shop v1.2?"), "{text}");
        assert!(html.contains("@post('/asks/a1'"), "{html}");
        assert!(html.contains("@post('/tickets/SHOP-5/land')"), "{html}");
        assert!(html.contains("$sel = ''; @get('/office/panel')"), "{html}");
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
    async fn the_free_seat_hires_and_the_empty_rack_adds_a_project() {
        let hire = html_of(office(), extra(), "elements/hire").await;
        assert!(hire.contains("@post('/office/hires'"), "{hire}");
        assert!(hire.contains("href=\"/hires?new\""), "{hire}");
        let add = html_of(office(), extra(), "elements/add-project").await;
        assert!(add.contains("@post('/office/projects'"), "{add}");
        assert!(add.contains("Projects now"), "{add}");
    }
}
