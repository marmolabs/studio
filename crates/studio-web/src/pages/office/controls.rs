//! What the studio settings do: the forms for the money limits, the desks,
//! hiring and opening or closing the office, and the routes they post to.
//! The cards of the team pages post to the same routes.

use serde::Deserialize;
use studiod::api::{FileKind, MAX_RUNS_LIMIT, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{content::Form, route},
    view::{View, component, view},
};

use crate::{AppConfig, client, ui};

/// A row of buttons.
const ACTIONS: &str = "flex flex-wrap gap-2";

/// The most desks the office can have.
const MOST_DESKS: u64 = MAX_RUNS_LIMIT;

/// The parts of `Request::Budget` the settings show.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub limits: Caps,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Caps {
    /// What one ticket may spend.
    pub ticket_usd: Option<f64>,
}

/// "Engineer" for `engineer`.
fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A quiet sentence.
#[component]
async fn quiet(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! { <p class="m-0 text-[13px] leading-5 text-fg-3">(text)</p> })
}

/// How many desks there are, and a way to add or remove one.
#[component]
pub(crate) async fn desks_view(desks: u64) -> topcoat::Result<impl View> {
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
        <div class=(ACTIONS)>
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
pub(crate) async fn open_view(closed: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex items-center gap-2">
            ui::dot(state: if closed { "failed" } else { "ready" })
            <b class="text-[14px] font-semibold">(if closed { "The office is closed" } else { "The office is open" })</b>
        </div>
        quiet(text: "Closing the office stops new work from starting. Work already running carries on, and waiting tasks wait until you reopen it.")
        <div class=(ACTIONS)>
            if closed {
                <button type="button" class=(ui::PRIMARY) data-on:click="@post('/office/open')">"Reopen the office"</button>
            } else {
                <button type="button" class=(ui::BUTTON) data-on:click="@post('/office/close')">"Close the office"</button>
            }
        </div>
    })
}

/// The form that hires someone, and a way to the hires editor for more.
#[component]
pub(crate) async fn hire_view() -> topcoat::Result<impl View> {
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
                        for role in ROLES { <option value=(role)>(capital(role))</option> }
                    </select>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Model"</span>
                    <select name="model" class=(ui::INPUT)>
                        for model in MODELS { <option value=(model)>(model)</option> }
                    </select>
                </label>
            </div>
            <label class=(ui::FIELD)>
                <span class=(ui::FIELD_LABEL)>"How they work"</span>
                <input name="about" autocomplete="off" placeholder="Optional. One line on how they work." class=(ui::INPUT)>
            </label>
            <div class=(ACTIONS)>
                <button type="submit" class=(ui::PRIMARY)>
                    ui::icon(name: "users")
                    "Hire"
                </button>
                <a href="/hires?new" class=(ui::GHOST)>"Write a longer brief in the hires editor"</a>
            </div>
        </form>
    })
}

/// The limits, to change: the daily one and what one ticket may spend.
#[component]
pub(crate) async fn limits_form(daily: f64, ticket: Option<f64>) -> topcoat::Result<impl View> {
    let ticket = ticket.map(|usd| usd.to_string()).unwrap_or_default();
    Ok(view! {
        <form class="flex flex-col gap-3" data-on:submit="@post('/office/limits', {contentType: 'form'})">
            <div class="grid grid-cols-2 gap-3">
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Daily limit, USD"</span>
                    <input name="daily_usd" type="number" step="0.01" min="0.01" required="" value=(daily.to_string()) class=(ui::INPUT)>
                </label>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"One task, USD"</span>
                    <input name="ticket_usd" type="number" step="0.01" min="0.01" value=(ticket) placeholder="Leave as is" class=(ui::INPUT)>
                </label>
            </div>
            <div><button type="submit" class=(ui::BUTTON)>"Save limits"</button></div>
        </form>
    })
}

/// Closes the office: agents finish the step they are on and nothing new
/// starts.
#[route(POST "/office/close")]
async fn close_office(cx: &Cx) -> topcoat::Result<PatchElements> {
    set_open(cx, false).await
}

/// Opens the office again.
#[route(POST "/office/open")]
async fn open_office(cx: &Cx) -> topcoat::Result<PatchElements> {
    set_open(cx, true).await
}

async fn set_open(cx: &Cx, open: bool) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    match client::call::<serde_json::Value>(&config.socket, Request::SetOpen { open }).await {
        Ok(_) => {
            let text = if open {
                "The office is open. Work starts again."
            } else {
                "The office is closed. Work in progress carries on; nothing new starts."
            };
            ui::notice(cx, text.into(), false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct Desks {
    max_runs: String,
}

/// Sets how many agents may work at once.
#[route(POST "/office/desks")]
async fn set_desks(cx: &Cx, Form(form): Form<Desks>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let max_runs = match form.max_runs.trim().parse::<u64>() {
        Ok(n @ 1..=MAX_RUNS_LIMIT) => n,
        _ => {
            let words = format!("Pick between 1 and {MAX_RUNS_LIMIT} desks.");
            return ui::notice(cx, words, true).await;
        }
    };
    match client::call::<serde_json::Value>(&config.socket, Request::SetMaxRuns { max_runs }).await
    {
        Ok(_) => {
            let words = if max_runs == 1 {
                "One agent can work at a time.".to_string()
            } else {
                format!("{max_runs} agents can work at once.")
            };
            ui::notice(cx, words, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct Limits {
    #[serde(default)]
    daily_usd: String,
    #[serde(default)]
    ticket_usd: String,
}

/// A limit typed in a field: empty leaves it as it is, else a positive
/// amount in dollars, with or without the "$".
fn amount(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().trim_start_matches('$').trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(n) if n.is_finite() && n > 0.0 => Ok(Some(n)),
        _ => Err(()),
    }
}

/// Sets the daily and per-ticket spending limits; a field left empty keeps
/// its limit.
#[route(POST "/office/limits")]
async fn set_limits(cx: &Cx, Form(form): Form<Limits>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let (Ok(daily_usd), Ok(ticket_usd)) = (amount(&form.daily_usd), amount(&form.ticket_usd))
    else {
        return ui::notice(cx, "A limit is an amount above $0.".into(), true).await;
    };
    if daily_usd.is_none() && ticket_usd.is_none() {
        return ui::notice(cx, "Enter a limit to change.".into(), true).await;
    }
    let request = Request::SetLimits {
        daily_usd,
        ticket_usd,
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            let mut parts = Vec::new();
            if let Some(n) = daily_usd {
                parts.push(format!("The daily limit is {}.", ui::limit(n)));
            }
            if let Some(n) = ticket_usd {
                parts.push(format!("A ticket may cost up to {}.", ui::limit(n)));
            }
            ui::notice(cx, parts.join(" "), false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The roles the Hire form offers, and the models.
const ROLES: [&str; 9] = [
    "engineer", "designer", "writer", "producer", "lead", "reviewer", "critic", "approver",
    "router",
];
const MODELS: [&str; 3] = ["sonnet", "opus", "haiku"];
/// The longest name a hire may have.
const HIRE_NAME_MAX: usize = 40;

#[derive(Deserialize)]
struct NewHire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    about: String,
}

/// Whether `name` can be a hire's id and file name: lowercase letters,
/// digits and dashes, starting with a letter or digit.
fn hire_id_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= HIRE_NAME_MAX
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A hire's file, in the shape of the hires editor's template.
fn hire_file(name: &str, role: &str, model: &str, about: &str) -> String {
    let about = about.split_whitespace().collect::<Vec<_>>().join(" ");
    let about = if about.is_empty() {
        format!("You are a careful {role}. Describe how you work here.")
    } else {
        about
    };
    format!("---\nname: {name}\nrole: {role}\nruntime: claude-code\nmodel: {model}\n---\n{about}\n")
}

/// Hires someone: writes their file, and they join the team at once.
#[route(POST "/office/hires")]
async fn hire_someone(cx: &Cx, Form(form): Form<NewHire>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let (name, role, model) = (form.name.trim(), form.role.trim(), form.model.trim());
    if !hire_id_ok(name) {
        let words = "A name is lowercase letters, digits and dashes, like quick-dev.";
        return ui::notice(cx, words.into(), true).await;
    }
    if !ROLES.contains(&role) {
        return ui::notice(cx, "Pick a role.".into(), true).await;
    }
    if !MODELS.contains(&model) {
        return ui::notice(cx, "Pick a model.".into(), true).await;
    }
    let status: client::Status = match client::call(&config.socket, Request::Status).await {
        Ok(status) => status,
        Err(err) => return ui::notice(cx, err.to_string(), true).await,
    };
    if status.team.iter().any(|m| m.id == name) {
        let words = format!("{name} is already on the team. Pick another name.");
        return ui::notice(cx, words, true).await;
    }
    let request = Request::SaveFile {
        kind: FileKind::Hires,
        name: name.to_string(),
        text: hire_file(name, role, model, &form.about),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, format!("Hired {name} as {role}."), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{named, text_of};
    use topcoat::context::Cx;

    async fn render(view: impl View) -> String {
        ui::html(&Cx::default(), view).await.unwrap()
    }

    #[test]
    fn a_limit_is_an_amount_above_zero() {
        assert_eq!(amount(""), Ok(None));
        assert_eq!(amount(" $12.50 "), Ok(Some(12.5)));
        assert_eq!(amount("3"), Ok(Some(3.0)));
        for bad in ["0", "-1", "abc", "inf", "NaN"] {
            assert_eq!(amount(bad), Err(()), "{bad}");
        }
    }

    #[test]
    fn a_hire_is_named_in_lowercase_and_filed_like_the_editors_template() {
        for good in ["dev", "quick-dev", "dev2", "9"] {
            assert!(hire_id_ok(good), "{good}");
        }
        for bad in [
            "",
            "Dev",
            "-dev",
            "quick dev",
            "dev_2",
            "dev.md",
            "../x",
            &"a".repeat(41),
        ] {
            assert!(!hire_id_ok(bad), "{bad}");
        }
        assert_eq!(
            hire_file("quick-dev", "engineer", "opus", " Careful\r\nand\tquick "),
            "---\nname: quick-dev\nrole: engineer\nruntime: claude-code\nmodel: opus\n---\nCareful and quick\n"
        );
        assert!(
            hire_file("w", "writer", "haiku", "")
                .ends_with("You are a careful writer. Describe how you work here.\n")
        );
    }

    #[tokio::test]
    async fn desks_can_be_added_and_removed_between_the_ends() {
        let cx = Cx::default();
        let html = render(view! { cx => desks_view(desks: 3) }).await;
        assert!(
            text_of(&html).contains("Up to 3 agents work at once"),
            "{html}"
        );
        assert!(html.contains("@post('/office/desks'"), "{html}");
        assert!(html.contains("name=\"max_runs\" value=\"4\""), "{html}");
        assert!(html.contains("name=\"max_runs\" value=\"2\""), "{html}");
        let one = render(view! { cx => desks_view(desks: 1) }).await;
        assert!(!one.contains("Remove a desk"), "{one}");
        let most = render(view! { cx => desks_view(desks: MOST_DESKS) }).await;
        assert!(!most.contains("Add a desk"), "{most}");
    }

    #[tokio::test]
    async fn the_office_can_be_closed_and_reopened() {
        let cx = Cx::default();
        let open = render(view! { cx => open_view(closed: false) }).await;
        assert!(text_of(&open).contains("The office is open"), "{open}");
        assert!(open.contains("@post('/office/close')"), "{open}");
        let shut = render(view! { cx => open_view(closed: true) }).await;
        assert!(text_of(&shut).contains("The office is closed"), "{shut}");
        assert!(shut.contains("Reopen the office"), "{shut}");
        assert!(shut.contains("@post('/office/open')"), "{shut}");
        assert!(
            text_of(&shut).contains("Work already running carries on"),
            "{shut}"
        );
    }

    #[tokio::test]
    async fn the_hire_form_offers_every_role_and_model() {
        let cx = Cx::default();
        let html = render(view! { cx => hire_view() }).await;
        assert!(html.contains("@post('/office/hires'"), "{html}");
        for field in ["name", "role", "model", "about"] {
            assert!(html.contains(&format!(r#"name="{field}""#)), "{field}");
        }
        for option in ROLES.into_iter().chain(MODELS) {
            assert!(
                named(&html, "option")
                    .iter()
                    .any(|o| o.attr_is("value", option)),
                "{option}: {html}"
            );
        }
        assert!(
            html.contains(r#"<option value="lead">Lead</option>"#),
            "{html}"
        );
        assert!(html.contains(r#"href="/hires?new""#), "{html}");
    }

    #[tokio::test]
    async fn the_limits_form_starts_from_the_limits_now() {
        let cx = Cx::default();
        let html = render(view! { cx => limits_form(daily: 10.0, ticket: Some(2.5)) }).await;
        assert!(html.contains("@post('/office/limits'"), "{html}");
        assert!(html.contains(r#"value="10""#), "{html}");
        assert!(html.contains(r#"value="2.5""#), "{html}");
        let bare = render(view! { cx => limits_form(daily: 10.0, ticket: None) }).await;
        assert!(bare.contains("Leave as is"), "{bare}");
    }
}
