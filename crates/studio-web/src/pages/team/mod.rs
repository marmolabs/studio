//! The team pages: the studio as a company, where the owner talks to one lead
//! for each project. Every page sits in the same frame (`shell`): the team
//! sidebar, with a lead for each project, and the page's own main area.
//!
//! This file holds the routes and the model the sidebar is drawn from. Each
//! page is a file of its own and draws itself whole, frame included: `hq` (the
//! morning round-up), `talk` (a conversation, with the `sheet` that slides over
//! it), `office` (the company floor), `company` (the work map and scorecard),
//! `rooms` (company and project conversations), `operations` (manager reviews
//! and controls), `new` (hiring a lead) and `settings`. `faces` draws a person's
//! portrait, and `stream` keeps every page up to date.
//!
//! The pages live at `/`, `/leads/{project}`, `/people/{hire}`,
//! `/rooms/{room}`, `/office`, `/company`, `/new` and `/studio`. The addresses
//! the studio had before redirect to them.

mod clock;
mod company;
mod faces;
mod hq;
mod new;
mod office;
mod operations;
mod person;
mod rooms;
mod settings;
mod sheet;
mod shell;
mod stream;
mod talk;

use serde::Deserialize;
use studiod::api::{NewLead, ReleaseState, Request, SetupChoice, SetupState, SetupView};
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, PatchSignals},
    router::{
        content::Form,
        error::{not_found, redirect_permanent},
        page, path_param, query_params,
        response::{IntoResponse, Response},
        route,
    },
    view::{View, view},
};

use self::talk::thread::Sizes;
use super::office::model::{Phase, Role, Turn};
use super::office::{Office, facts, gather, hq_scene, plan, safe};
use crate::{AppConfig, client, ui};

/// What the sidebar of every team page shows, worked out from the office
/// snapshot: a lead for each project, what waits for the owner, and what has
/// been spent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Team {
    /// One for each project, in the studio's order.
    pub leads: Vec<LeadEntry>,
    /// Everything that waits for the owner, in every project.
    pub needs: usize,
    /// What was spent over the last 24 hours, in dollars: what counts
    /// against the daily limit.
    pub spent_today: f64,
    /// The most the studio may spend in a day, in dollars.
    pub daily_usd: f64,
    /// The company floor, for the sidebar's small picture of it.
    pub floor: Floor,
}

/// The company floor drawn once for the sidebar's thumbnail.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Floor {
    /// The floor's own drawing, as the office view draws it.
    pub svg: String,
    /// Its width over its height, as a CSS ratio such as "1000 / 620".
    pub ratio: String,
    /// The office is closed: the floor is drawn dimmed.
    pub closed: bool,
}

/// A project and its lead, as the sidebar lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct LeadEntry {
    pub project: String,
    /// The lead's hire id; none when the project has no lead.
    pub lead: Option<String>,
    /// What to call the lead: the name in its hire file, else its id. A
    /// project without a lead has the project's name here.
    pub name: String,
    pub role: Role,
    /// The project's colour, as the office floor has it.
    pub tint: usize,
    pub phase: Phase,
    /// The phase in a few words: "Building", "Testing", "v1.2 waits for you",
    /// "Live v1.0", "Setting up…".
    pub phase_words: String,
    /// How far the project's milestone has come, from 0 to 1.
    pub progress: f64,
    /// What waits for the owner in this project.
    pub needs: usize,
}

impl Team {
    /// The team of the studio `office` describes.
    pub fn of(office: &Office) -> Team {
        let projects = office.project_names();
        let ready: usize = projects
            .iter()
            .map(|project| hq::ships(office, project, &Sizes::new()).len())
            .sum();
        Team {
            leads: projects
                .iter()
                .map(|project| LeadEntry::of(office, &projects, project))
                .collect(),
            needs: facts::owner_needs(office) + ready + office.operations.pending(),
            spent_today: office.status.spent_24h,
            daily_usd: office.status.daily_usd,
            floor: Floor::of(office),
        }
    }

    /// The entry of `project`, if the studio has it.
    pub fn lead(&self, project: &str) -> Option<&LeadEntry> {
        self.leads.iter().find(|entry| entry.project == project)
    }
}

impl Floor {
    /// The floor of the studio `office` describes.
    fn of(office: &Office) -> Floor {
        let floor = plan::hq(office);
        let scene = hq_scene::render(&floor, Turn::default());
        Floor {
            ratio: format!("{} / {}", scene.width, scene.height),
            svg: scene.svg,
            closed: floor.closed,
        }
    }
}

impl LeadEntry {
    /// `project`'s entry; `projects` are all of them, in the studio's order,
    /// which give each its colour.
    fn of(office: &Office, projects: &[String], project: &str) -> LeadEntry {
        let lead = office.project(project).and_then(|view| view.lead.clone());
        let phase = facts::phase(office, project);
        LeadEntry {
            project: project.to_string(),
            name: lead
                .as_deref()
                .map_or_else(|| project.to_string(), |hire| office.name_of(hire)),
            lead,
            role: Role::Lead,
            tint: facts::tint(projects, project),
            phase,
            phase_words: phase_words(office, project, phase),
            progress: progress(office, project, phase),
            // What waits for the owner, and the work ready to ship, which the
            // office does not count.
            needs: facts::project_needs(office, project)
                + hq::ships(office, project, &Sizes::new()).len()
                + office
                    .operations
                    .actions
                    .iter()
                    .filter(|action| {
                        action.project == project
                            && action.state == studiod::api::OperationState::Pending
                    })
                    .count(),
        }
    }
}

/// A version as people say it: "v1.2", whether it was written with the v or
/// not.
fn version(written: &str) -> String {
    format!("v{}", written.trim_start_matches(['v', 'V']))
}

/// The most characters of what a goal's tasks came to that a card says.
const RESULT_MAX: usize = 160;

/// What the finished task `id` came to, in a line; empty when it has none.
pub(crate) fn task_result(office: &Office, id: &str) -> String {
    office
        .results
        .get(id)
        .map(|said| said.trim().to_string())
        .unwrap_or_default()
}

/// What `goal` came to: the result of its plan, if it has one, else what its
/// tasks came to, one after another.
pub(crate) fn goal_said(office: &Office, goal: &super::office::floor::Goal) -> String {
    let own = task_result(office, &goal.plan.id.to_string());
    if !own.is_empty() {
        return own;
    }
    let said: Vec<String> = goal
        .filed
        .iter()
        .map(|task| task_result(office, &task.id.to_string()))
        .filter(|said| !said.is_empty())
        .collect();
    said.join(" ")
}

/// What `goal` came to, in a line cut where a card has no more room.
pub(crate) fn goal_result(office: &Office, goal: &super::office::floor::Goal) -> String {
    let said = goal_said(office, goal);
    if said.chars().count() <= RESULT_MAX {
        return said;
    }
    let head: String = said.chars().take(RESULT_MAX - 1).collect();
    // Back to the last whole word.
    let head = head
        .rsplit_once(' ')
        .map_or(head.as_str(), |(words, _)| words);
    format!("{}…", head.trim_end_matches([',', ';', ':', '.', ' ']))
}

/// Whether the lead of `project` is still setting it up: reading it, trying
/// what it proposed, or waiting for the owner's choice.
fn setting_up(office: &Office, project: &str) -> bool {
    office.setups.get(project).is_some_and(|setup| {
        matches!(
            setup.state,
            SetupState::Reading | SetupState::Checking | SetupState::Ready
        )
    })
}

/// What `project` is doing, in the few words the sidebar has room for.
fn phase_words(office: &Office, project: &str, phase: Phase) -> String {
    if setting_up(office, project) {
        return "Setting up…".to_string();
    }
    let view = office.project(project);
    match phase {
        Phase::Idle => "Idle".to_string(),
        Phase::Planning => "Planning".to_string(),
        Phase::Building => "Building".to_string(),
        Phase::Testing => "Testing".to_string(),
        Phase::Paused => "Paused".to_string(),
        Phase::Releasing => match view.and_then(|view| view.release.as_ref()) {
            Some(release) if release.state == ReleaseState::Waiting => {
                format!("{} waits for you", version(&release.version))
            }
            Some(release) => format!("Releasing {}", version(&release.version)),
            None => "Releasing".to_string(),
        },
        Phase::Live => match view.and_then(|view| view.last_release.as_ref()) {
            Some(release) => format!("Live {}", version(&release.version)),
            None => "Live".to_string(),
        },
    }
}

/// How far `project` has come, from 0 to 1: its milestone's done tasks over
/// all of them; nearly there (0.9) while a release is under way, as it waits
/// for the owner's go-ahead or is published; all the way (1) once it is live.
fn progress(office: &Office, project: &str, phase: Phase) -> f64 {
    let releasing = office
        .project(project)
        .and_then(|view| view.release.as_ref())
        .is_some_and(facts::is_releasing);
    if releasing {
        return 0.9;
    }
    if phase == Phase::Live {
        return 1.0;
    }
    match facts::milestone(office, project) {
        Some(goal) if !goal.filed.is_empty() => goal.landed() as f64 / goal.filed.len() as f64,
        _ => 0.0,
    }
}

/// The address of a project's conversation. The project's name may hold
/// anything a folder's name can, so each byte outside letters, digits and
/// `-_.~` is escaped.
pub(crate) fn lead_href(project: &str) -> String {
    escaped_path("/leads/", project)
}

/// The address of a hire's direct conversation.
pub(crate) fn person_href(hire: &str) -> String {
    escaped_path("/people/", hire)
}

/// The address of a company or project room.
pub(crate) fn room_href(room: &str) -> String {
    escaped_path("/rooms/", room)
}

fn escaped_path(prefix: &str, id: &str) -> String {
    let mut href = String::from(prefix);
    for byte in id.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                href.push(char::from(byte));
            }
            _ => href.push_str(&format!("%{byte:02X}")),
        }
    }
    href
}

/// Whether `path` is a team page, which draws the team sidebar itself.
pub fn owns(path: &str) -> bool {
    let first = path.trim_matches('/').split('/').next().unwrap_or_default();
    matches!(
        first,
        "" | "leads" | "people" | "rooms" | "office" | "company" | "new" | "studio"
    )
}

/// Whether the layout opens the one stream of the team pages for the page at
/// `path`, which then keeps itself up to date. The office does not: it opens
/// the stream itself, again whenever the owner picks someone on the
/// floor or turns it, as the stream draws the floor as it is picked.
pub fn live(path: &str) -> bool {
    matches!(stream::Spot::of(path), Some(spot) if !matches!(spot, stream::Spot::Office { .. }))
}

/// What opens the stream of the page the browser is at.
pub use stream::OPEN as OPEN_STREAM;

path_param!(project);
path_param!(id);

/// Reads the office once for a page, and the team drawn from it.
async fn read(cx: &Cx) -> topcoat::Result<(Office, Team)> {
    let office = gather(app_context::<AppConfig>(cx)).await?;
    let team = Team::of(&office);
    Ok((office, team))
}

#[query_params(error = bad_request)]
struct Home {
    /// The tab of the old home's side panel.
    tab: Option<String>,
    /// What was picked on the old home: `projects/<name>`, `tickets/<id>`.
    sel: Option<String>,
}

impl Home {
    /// Where the old home's address, with its tab or its pick, leads now;
    /// none for plain HQ.
    fn old_address(&self, office: &Office) -> Option<String> {
        if let Some((kind, id)) = self.sel.as_deref().and_then(|sel| sel.split_once('/')) {
            match kind {
                "projects" => {
                    let projects = office.project_names();
                    if let Some(project) = projects.iter().find(|project| safe(project) == id) {
                        return Some(lead_href(project));
                    }
                }
                "tickets" if !safe(id).is_empty() => return Some(format!("/tickets/{}", safe(id))),
                // A goal is looked at from its project's conversation.
                "goals" => {
                    if let Some(ticket) = office.ticket(&safe(id)) {
                        return Some(format!(
                            "{}?look={}",
                            lead_href(&ticket.project.to_string()),
                            ticket.id
                        ));
                    }
                }
                _ => {}
            }
        }
        match self.tab.as_deref() {
            Some("money" | "company") => Some("/studio".to_string()),
            // The other tabs, and a pick that is nowhere now, are HQ without
            // the query.
            Some(_) => Some("/".to_string()),
            None => self.sel.as_ref().map(|_| "/".to_string()),
        }
    }
}

/// HQ: the morning round-up, the studio's home page. The old home's
/// addresses, with a tab or a pick in the query, lead to where those are now.
#[page("/")]
async fn hq_page(cx: &Cx) -> topcoat::Result<impl View> {
    let home = query_params::<Home>(cx)?;
    let (office, team) = read(cx).await?;
    if let Some(to) = home.old_address(&office) {
        return Err(redirect_permanent(&to).into());
    }
    let sizes = talk::sizes_of_all(
        app_context::<AppConfig>(cx),
        &office,
        &mut talk::Memo::default(),
    )
    .await;
    Ok(view! { hq::hq(team: team, office: office, sizes: sizes) })
}

/// HQ was at `/hq` while it was being made.
#[route(GET "/hq")]
async fn old_hq() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The old home was at `/office/classic`: with a tab or a pick it leads to
/// where those are now, else to the office, where its floor is.
#[route(GET "/office/classic")]
async fn old_classic(cx: &Cx) -> topcoat::Result<Response> {
    let home = query_params::<Home>(cx)?;
    let to = if home.tab.is_none() && home.sel.is_none() {
        "/office".to_string()
    } else {
        let office = gather(app_context::<AppConfig>(cx)).await?;
        home.old_address(&office).unwrap_or_else(|| "/".to_string())
    };
    Err(redirect_permanent(&to).into())
}

#[query_params(error = bad_request)]
struct Looking {
    /// The goal's plan ticket or the ready task whose sheet is open.
    look: Option<String>,
}

/// A conversation with a project's lead, with the sheet of the work picked
/// by `?look=`.
#[page("/leads/{project}")]
async fn lead_page(cx: &Cx) -> topcoat::Result<impl View> {
    let project = path_param::<Project>(cx).to_owned();
    let look = query_params::<Looking>(cx)?
        .look
        .as_deref()
        .map(safe)
        .filter(|look| !look.is_empty());
    let config = app_context::<AppConfig>(cx);
    let (office, team) = read(cx).await?;
    let reads = talk::read(config, &office, &project, &mut talk::Memo::default()).await;
    let Some(parts) = talk::parts(&team, &office, &project, &reads) else {
        return Err(not_found().into());
    };
    let sheet = match look {
        Some(id) => sheet::gather(config, &office, &project, &id, parts.tint).await,
        None => None,
    };
    Ok(view! { talk::talk(team: team, parts: parts, sheet: sheet) })
}

/// A direct conversation with one hire.
#[page("/people/{id}")]
async fn person_page(cx: &Cx) -> topcoat::Result<impl View> {
    let hire = path_param::<Id>(cx).to_owned();
    let config = app_context::<AppConfig>(cx);
    let (office, team) = read(cx).await?;
    let conversation = person::read(config, &hire).await;
    let Some(parts) = person::parts(&office, &hire, conversation) else {
        return Err(not_found().into());
    };
    Ok(view! { person::person_view(team: team, parts: parts) })
}

#[route(GET "/rooms")]
async fn rooms_home() -> topcoat::Result<Response> {
    Err(redirect_permanent("/rooms/company").into())
}

/// A persistent company or project room.
#[page("/rooms/{id}")]
async fn room_page(cx: &Cx) -> topcoat::Result<impl View> {
    let room = path_param::<Id>(cx).to_owned();
    let config = app_context::<AppConfig>(cx);
    let (office, team) = read(cx).await?;
    if !office.rooms.iter().any(|summary| summary.id == room) {
        return Err(not_found().into());
    }
    let conversation = rooms::read(config, &room).await;
    Ok(
        view! { rooms::room_view(team: team, office: office, room: room, conversation: conversation) },
    )
}

#[query_params(error = bad_request)]
struct Turned {
    /// The corner the floor is seen from, in quarter turns.
    turn: Option<i64>,
    /// What is picked on the floor, as a click writes it, such as
    /// `projects/shop`.
    pick: Option<String>,
}

/// The company's live work map and manager decision log.
#[page("/company")]
async fn company_page(cx: &Cx) -> topcoat::Result<impl View> {
    let (office, team) = read(cx).await?;
    Ok(view! { company::company(team: team, office: office) })
}

/// The company floor, seen from the corner `?turn=` names, with what `?pick=`
/// names picked.
#[page("/office")]
async fn office_page(cx: &Cx) -> topcoat::Result<impl View> {
    let query = query_params::<Turned>(cx)?;
    let turn = Turn::new(query.turn.unwrap_or_default());
    let sel = office::clean(query.pick.as_deref().unwrap_or_default());
    let (office, team) = read(cx).await?;
    Ok(view! { office::office_view(team: team, office: office, turn: turn, sel: sel) })
}

/// The company floor was at `/team/office` while it was being made.
#[route(GET "/team/office")]
async fn old_team_office() -> topcoat::Result<Response> {
    Err(redirect_permanent("/office").into())
}

/// Hiring a lead for a new project.
#[page("/new")]
async fn new_page(cx: &Cx) -> topcoat::Result<impl View> {
    let (office, team) = read(cx).await?;
    Ok(view! { new::new_project(team: team, office: office) })
}

/// The studio's settings: money, desks, hires and rules.
#[page("/studio")]
async fn studio_page(cx: &Cx) -> topcoat::Result<impl View> {
    let (office, team) = read(cx).await?;
    let ticket_usd = settings::ticket_usd(app_context::<AppConfig>(cx)).await;
    Ok(view! { settings::settings(team: team, office: office, ticket_usd: ticket_usd) })
}

/// What a message box posts.
#[derive(Deserialize)]
struct Say {
    #[serde(default)]
    text: String,
    /// The work a note sent back is about: its name goes before the note.
    #[serde(default)]
    about: String,
}

/// The owner's message to a project's lead.
#[route(POST "/leads/{project}/talk")]
async fn talk_to_lead(cx: &Cx, Form(form): Form<Say>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = path_param::<Project>(cx).to_owned();
    let text = form.text.trim();
    if text.is_empty() {
        return ui::notice(cx, "Write something to send.".into(), true).await;
    }
    let text = match form.about.trim() {
        "" => text.to_string(),
        about => format!("Send back \"{about}\": {text}"),
    };
    let request = Request::Talk {
        project: project.clone(),
        text,
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        // The message shows in the thread, so there is nothing more to say.
        Ok(_) => Ok(ui::clear_notice()),
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The owner's message to one hire's read-only conversation.
#[route(POST "/people/{id}/talk")]
async fn talk_to_person(cx: &Cx, Form(form): Form<Say>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let hire = path_param::<Id>(cx).to_owned();
    let text = form.text.trim();
    if text.is_empty() {
        return ui::notice(cx, "Write something to send.".into(), true).await;
    }
    let request = Request::TalkToHire {
        hire,
        text: text.to_string(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => Ok(ui::clear_notice()),
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The owner's message or announcement in a room.
#[route(POST "/rooms/{id}/talk")]
async fn talk_to_room(cx: &Cx, Form(form): Form<Say>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let room = path_param::<Id>(cx).to_owned();
    let text = form.text.trim();
    if text.is_empty() {
        return ui::notice(cx, "Write something to send.".into(), true).await;
    }
    let request = Request::TalkToRoom {
        room,
        text: text.to_string(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => Ok(ui::clear_notice()),
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// A message to the agent at work on a task.
#[route(POST "/tasks/{id}/tell")]
async fn tell_task(cx: &Cx, Form(form): Form<Say>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let task = path_param::<Id>(cx).to_owned();
    let text = form.text.trim();
    if text.is_empty() {
        return ui::notice(cx, "Write something to tell it.".into(), true).await;
    }
    let request = Request::Tell {
        task: task.clone(),
        text: text.to_string(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, format!("Told the agent on {task}."), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct OperationDecision {
    decision: String,
    #[serde(default)]
    note: Option<String>,
}

#[route(POST "/operations/{id}/decision")]
async fn decide_operation(
    cx: &Cx,
    Form(form): Form<OperationDecision>,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let action = path_param::<Id>(cx).to_owned();
    let approve = match form.decision.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return ui::notice(cx, "Choose approve or reject.".into(), true).await,
    };
    let request = Request::DecideOperation {
        action,
        approve,
        note: form.note.filter(|note| !note.trim().is_empty()),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            ui::notice(
                cx,
                if approve {
                    "Approved. The manager's action is complete.".into()
                } else {
                    "Rejected. Nothing was changed.".into()
                },
                false,
            )
            .await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct ReviewProject {
    project: String,
}

#[route(POST "/operations/review")]
async fn review_operations(
    cx: &Cx,
    Form(form): Form<ReviewProject>,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::ReviewOperations {
        project: form.project,
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, "The manager is reviewing the project.".into(), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[derive(Deserialize)]
struct OperationControls {
    #[serde(default)]
    autopilot: String,
    #[serde(default)]
    review_minutes: String,
    #[serde(default)]
    autonomy: String,
    #[serde(default)]
    mode: String,
}

#[route(POST "/operations/settings")]
async fn operation_controls(
    cx: &Cx,
    Form(form): Form<OperationControls>,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let autopilot = match form.autopilot.trim() {
        "" => None,
        "true" => Some(true),
        "false" => Some(false),
        _ => return ui::notice(cx, "Choose whether automatic reviews run.".into(), true).await,
    };
    let review_minutes = match form.review_minutes.trim() {
        "" => None,
        minutes => match minutes.parse::<u64>() {
            Ok(minutes @ 5..=1440) => Some(minutes),
            _ => {
                return ui::notice(
                    cx,
                    "Reviews must be between 5 minutes and 24 hours apart.".into(),
                    true,
                )
                .await;
            }
        },
    };
    let autonomy = match form.autonomy.trim() {
        "" => None,
        "supervised" => Some(studiod::api::Autonomy::Supervised),
        "balanced" => Some(studiod::api::Autonomy::Balanced),
        "autonomous" => Some(studiod::api::Autonomy::Autonomous),
        _ => return ui::notice(cx, "Choose an autonomy level.".into(), true).await,
    };
    let mode = match form.mode.trim() {
        "" => None,
        "live" => Some(studiod::api::RuntimeMode::Live),
        "simulation" => Some(studiod::api::RuntimeMode::Simulation),
        _ => return ui::notice(cx, "Choose live or simulation.".into(), true).await,
    };
    let request = Request::SetOperations {
        autopilot,
        review_minutes,
        autonomy,
        mode,
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, "Manager controls saved.".into(), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// What the New project form posts.
#[derive(Deserialize)]
struct NewProject {
    /// The project's folder.
    #[serde(default)]
    path: String,
    /// What to call the project; empty for its folder's name.
    #[serde(default)]
    name: String,
    /// An existing hire to lead it.
    #[serde(default)]
    lead: String,
    /// Or a new hire: its name and model.
    #[serde(default)]
    new_lead_name: String,
    #[serde(default)]
    new_lead_model: String,
}

/// A field left empty is sent as absent.
fn absent(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Hires the project's lead, adds the project and has the lead set it up. The
/// reply sends the browser to the project's conversation.
#[route(POST "/new")]
async fn hire_for_project(cx: &Cx, Form(form): Form<NewProject>) -> topcoat::Result<Response> {
    let config = app_context::<AppConfig>(cx);
    let Some(path) = absent(&form.path) else {
        let words = "Enter the folder of the project.".to_string();
        return ui::notice(cx, words, true).await?.into_response(cx);
    };
    // A name for a new lead is a new hire, whichever lead was picked before.
    let new_lead = absent(&form.new_lead_name).map(|name| NewLead {
        name,
        model: absent(&form.new_lead_model).unwrap_or_else(|| new::LEAD_MODEL.to_string()),
    });
    let lead = absent(&form.lead).filter(|_| new_lead.is_none());
    if lead.is_none() && new_lead.is_none() {
        let words = "Pick a lead for it, or name a new one.".to_string();
        return ui::notice(cx, words, true).await?.into_response(cx);
    }
    let request = Request::SetupProject {
        path,
        name: absent(&form.name),
        lead,
        new_lead,
    };
    match client::call::<SetupView>(&config.socket, request).await {
        Ok(setup) => PatchSignals::json(&serde_json::json!({"_go": lead_href(&setup.project)}))?
            .into_response(cx),
        Err(err) => ui::notice(cx, err.to_string(), true)
            .await?
            .into_response(cx),
    }
}

/// What the "Use these" button posts: the owner's choice as JSON.
#[derive(Deserialize)]
struct Accepted {
    #[serde(default)]
    choice: String,
}

/// The owner's go-ahead for what the lead proposed to set a project up.
#[route(POST "/leads/{project}/setup/accept")]
async fn accept_setup(cx: &Cx, Form(form): Form<Accepted>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = path_param::<Project>(cx).to_owned();
    let Ok(choice) = serde_json::from_str::<SetupChoice>(&form.choice) else {
        return ui::notice(cx, "That choice can't be read.".into(), true).await;
    };
    let request = Request::AcceptSetup {
        project: project.clone(),
        choice,
    };
    match client::call::<SetupView>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, format!("{project} is set up."), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use studio_core::ticket::Ticket;
    use studiod::api::ProjectView;

    fn ticket(id: &str, project: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({
            "id": id, "project": project, "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            fields.extend(extra.clone());
        }
        serde_json::from_value(fields).unwrap()
    }

    fn work(id: &str, project: &str, state: Value) -> Ticket {
        ticket(id, project, state, json!({}))
    }

    fn release(project: &str, version: &str, state: &str) -> studiod::api::ReleaseView {
        serde_json::from_value(json!({
            "project": project, "version": version, "state": state,
            "steps": [{"name": "Final checks", "done": true}, {"name": "Go-ahead", "done": false}],
            "started_at": 0,
        }))
        .unwrap()
    }

    fn setup(project: &str, state: SetupState) -> SetupView {
        SetupView {
            project: project.into(),
            state,
            ..SetupView::default()
        }
    }

    /// A studio with `shop`, led by Nora; `api`, which has no lead; and `docs`,
    /// led by theo, whose hire file gives no name.
    fn office(tickets: Vec<Ticket>) -> Office {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects": ["shop", "api", "docs"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "theo", "role": "lead", "model": "opus"},
                {"id": "dev", "role": "engineer", "model": "sonnet"},
            ],
            "daily_usd": 10.0, "spent_24h": 6.59,
        }))
        .unwrap();
        office.projects = vec![
            ProjectView {
                name: "shop".into(),
                lead: Some("nora".into()),
                ..ProjectView::default()
            },
            ProjectView {
                name: "api".into(),
                ..ProjectView::default()
            },
            ProjectView {
                name: "docs".into(),
                lead: Some("theo".into()),
                ..ProjectView::default()
            },
        ];
        office
    }

    fn entry(office: &Office, project: &str) -> LeadEntry {
        Team::of(office).lead(project).unwrap().clone()
    }

    #[test]
    fn a_lead_for_each_project_in_the_studios_order() {
        let team = Team::of(&office(vec![]));
        let listed: Vec<(&str, Option<&str>, &str, usize)> = team
            .leads
            .iter()
            .map(|e| {
                (
                    e.project.as_str(),
                    e.lead.as_deref(),
                    e.name.as_str(),
                    e.tint,
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("shop", Some("nora"), "Nora", 0),
                // Nobody leads it, so the project's own name is all there is.
                ("api", None, "api", 1),
                // A hire file without a name is called by its id.
                ("docs", Some("theo"), "theo", 2),
            ]
        );
        assert!(team.leads.iter().all(|e| e.role == Role::Lead));
        assert_eq!(team.lead("api").map(|e| e.project.as_str()), Some("api"));
        assert!(team.lead("garden").is_none());
        assert!(Team::of(&Office::default()).leads.is_empty());
    }

    #[test]
    fn the_phase_is_said_in_a_few_words() {
        let words = |office: &Office| entry(office, "shop").phase_words;
        assert_eq!(words(&office(vec![])), "Idle");
        let at = |state: Value| office(vec![work("S-1", "shop", state)]);
        assert_eq!(words(&at(json!("working"))), "Building");
        assert_eq!(words(&at(json!("checking"))), "Testing");
        assert_eq!(entry(&at(json!("working")), "shop").phase, Phase::Building);
        let planning = office(vec![ticket(
            "S-2",
            "shop",
            json!("working"),
            json!({"kind": "plan", "goal": "Checkout"}),
        )]);
        assert_eq!(words(&planning), "Planning");

        let mut closed = at(json!("working"));
        closed.status.closed = true;
        assert_eq!(words(&closed), "Paused");

        let mut releasing = office(vec![]);
        releasing.projects[0].release = Some(release("shop", "1.2", "waiting"));
        assert_eq!(words(&releasing), "v1.2 waits for you");
        releasing.projects[0].release = Some(release("shop", "1.2", "publishing"));
        assert_eq!(words(&releasing), "Releasing v1.2");
        // A version written with its v is not given another.
        releasing.projects[0].release = Some(release("shop", "v1.2", "waiting"));
        assert_eq!(words(&releasing), "v1.2 waits for you");

        let mut live = office(vec![]);
        live.projects[0].last_release = Some(release("shop", "1.0", "released"));
        assert_eq!(words(&live), "Live v1.0");
    }

    #[test]
    fn a_project_is_being_set_up_until_the_owner_accepts_or_it_fails() {
        let words = |state| {
            let mut office = office(vec![work("S-1", "shop", json!("working"))]);
            office.setups.insert("shop".into(), setup("shop", state));
            entry(&office, "shop").phase_words
        };
        for state in [SetupState::Reading, SetupState::Checking, SetupState::Ready] {
            assert_eq!(words(state), "Setting up…", "{state:?}");
        }
        // Then it is what its work says.
        for state in [SetupState::Accepted, SetupState::Failed] {
            assert_eq!(words(state), "Building", "{state:?}");
        }
        // Another project's setup is not this one's.
        let mut office = office(vec![]);
        office
            .setups
            .insert("api".into(), setup("api", SetupState::Reading));
        assert_eq!(entry(&office, "shop").phase_words, "Idle");
        assert_eq!(entry(&office, "api").phase_words, "Setting up…");
    }

    #[test]
    fn progress_is_the_milestones_done_tasks_over_all_of_them() {
        let goal = |version: Option<&str>| {
            let mut extra = json!({"kind": "plan", "goal": "Checkout flow"});
            extra["version"] = json!(version);
            ticket("S-1", "shop", json!("landed"), extra)
        };
        let task = |id: &str, state: Value| ticket(id, "shop", state, json!({"parent": "S-1"}));
        let progress_of = |tickets: Vec<Ticket>| entry(&office(tickets), "shop").progress;
        assert_eq!(progress_of(vec![]), 0.0);
        // A goal that has filed no task has nothing done yet.
        assert_eq!(progress_of(vec![goal(Some("0.4"))]), 0.0);
        let tasks = vec![
            goal(Some("0.4")),
            task("S-2", json!("landed")),
            task("S-3", json!("working")),
            task("S-4", json!("queued")),
            task("S-5", json!("landed")),
            // A cancelled task is not one of the goal's.
            task("S-6", json!("cancelled")),
        ];
        let share = progress_of(tasks.clone());
        assert!((share - 2.0 / 4.0).abs() < 1e-9, "{share}");
        let done = vec![goal(None), task("S-2", json!("landed"))];
        assert_eq!(progress_of(done), 1.0);
    }

    #[test]
    fn a_release_under_way_is_nearly_there_and_a_live_project_is_all_the_way() {
        let mut busy = office(vec![
            ticket(
                "S-1",
                "shop",
                json!("landed"),
                json!({"kind": "plan", "goal": "Checkout", "version": "0.4"}),
            ),
            ticket("S-2", "shop", json!("working"), json!({"parent": "S-1"})),
        ]);
        assert_eq!(entry(&busy, "shop").progress, 0.0);
        for state in ["waiting", "checking", "publishing"] {
            busy.projects[0].release = Some(release("shop", "0.4", state));
            assert_eq!(entry(&busy, "shop").progress, 0.9, "{state}");
        }
        // Once it is published and nothing is open, the project is live.
        let mut live = office(vec![]);
        live.projects[0].last_release = Some(release("shop", "0.4", "released"));
        assert_eq!(entry(&live, "shop").progress, 1.0);
        assert_eq!(entry(&live, "shop").phase, Phase::Live);
        // A project that never released and has nothing open is at the start.
        assert_eq!(entry(&live, "api").progress, 0.0);
    }

    #[test]
    fn what_waits_for_the_owner_is_counted_for_each_lead_and_for_the_team() {
        let mut office = office(vec![
            work("S-1", "shop", json!({"asking": {"ask": "a1"}})),
            work("A-1", "api", json!({"held": {"reason": "auth"}})),
            // A hold that ends on its own waits for nobody.
            work("D-1", "docs", json!({"held": {"reason": "budget"}})),
        ]);
        office.asks = vec![
            serde_json::from_value(json!({
                "id": "a1", "ticket": "S-1", "hire": "dev", "kind": "permission",
                "tool": "Bash", "input": {}, "reasons": [],
            }))
            .unwrap(),
        ];
        office.projects[2].release = Some(release("docs", "1.2", "waiting"));
        let team = Team::of(&office);
        let needs: Vec<usize> = team.leads.iter().map(|e| e.needs).collect();
        assert_eq!(needs, [1, 1, 1]);
        assert_eq!(team.needs, 3);
        assert_eq!(Team::of(&Office::default()).needs, 0);
    }

    #[test]
    fn the_team_knows_what_was_spent_against_the_limit() {
        let team = Team::of(&office(vec![]));
        assert_eq!((team.spent_today, team.daily_usd), (6.59, 10.0));
        assert_eq!(Team::of(&Office::default()).spent_today, 0.0);
    }

    #[test]
    fn the_team_pages_are_told_from_the_rest() {
        for path in [
            "/",
            "/leads/shop",
            "/people/dev",
            "/office",
            "/new",
            "/studio",
        ] {
            assert!(owns(path), "{path}");
        }
        for path in ["/hires", "/rules", "/tickets/GAME-1", "/newt", "/hq"] {
            assert!(!owns(path), "{path}");
        }
        // The layout opens the one stream for the pages it draws.
        for path in [
            "/",
            "/leads/shop?look=SHOP-1",
            "/people/dev",
            "/new",
            "/studio",
        ] {
            assert!(live(path), "{path}");
        }
        // The office opens it itself, again whenever the pick or the corner
        // changes.
        for path in [
            "/office",
            "/office?turn=1",
            "/hires",
            "/tickets/GAME-1",
            "/leads",
        ] {
            assert!(!live(path), "{path}");
        }
        for path in ["/office", "/office?turn=1"] {
            assert!(stream::Spot::of(path).is_some(), "the stream knows {path}");
        }
    }

    #[test]
    fn the_old_homes_picks_lead_to_where_they_are_now() {
        let office = office(vec![ticket(
            "S-1",
            "shop",
            json!("working"),
            json!({"kind": "plan", "goal": "Checkout"}),
        )]);
        let to = |tab: Option<&str>, sel: Option<&str>| {
            Home {
                tab: tab.map(str::to_string),
                sel: sel.map(str::to_string),
            }
            .old_address(&office)
        };
        assert_eq!(to(None, None), None);
        assert_eq!(to(Some("needs"), None), Some("/".into()));
        assert_eq!(to(Some("money"), None), Some("/studio".into()));
        assert_eq!(to(Some("company"), None), Some("/studio".into()));
        assert_eq!(to(None, Some("projects/shop")), Some("/leads/shop".into()));
        assert_eq!(to(None, Some("tickets/S-1")), Some("/tickets/S-1".into()));
        assert_eq!(
            to(None, Some("goals/S-1")),
            Some("/leads/shop?look=S-1".into())
        );
        // A pick wins over the tab it came with; one that is nowhere is HQ.
        assert_eq!(
            to(Some("money"), Some("projects/shop")),
            Some("/leads/shop".into())
        );
        assert_eq!(to(None, Some("projects/garden")), Some("/".into()));
        assert_eq!(to(None, Some("goals/S-9")), Some("/".into()));
        assert_eq!(to(None, Some("elements/racks")), Some("/".into()));
    }

    #[test]
    fn conversation_addresses_escape_their_project_or_hire() {
        assert_eq!(lead_href("shop"), "/leads/shop");
        assert_eq!(person_href("dev"), "/people/dev");
        assert_eq!(person_href("a/b?c#d"), "/people/a%2Fb%3Fc%23d");
        assert_eq!(
            lead_href("pocket-garden_2.0~x"),
            "/leads/pocket-garden_2.0~x"
        );
        assert_eq!(lead_href("my shop"), "/leads/my%20shop");
        assert_eq!(lead_href("a/b?c#d"), "/leads/a%2Fb%3Fc%23d");
        assert_eq!(lead_href("café"), "/leads/caf%C3%A9");
    }

    #[test]
    fn what_a_goal_came_to_is_its_plans_own_result_or_its_tasks_cut_to_a_line() {
        let mut office = office(vec![
            ticket(
                "S-1",
                "shop",
                json!("landed"),
                json!({"kind": "plan", "goal": "Save carts"}),
            ),
            ticket("S-2", "shop", json!("landed"), json!({"parent": "S-1"})),
            ticket("S-3", "shop", json!("landed"), json!({"parent": "S-1"})),
        ]);
        let goal = |office: &Office| super::super::office::floor::goals(office).remove(0);
        // Nobody said anything yet.
        assert_eq!(goal_result(&office, &goal(&office)), "");
        // The tasks' words, one after another.
        office.results.insert("S-2".into(), "Stored carts.".into());
        office
            .results
            .insert("S-3".into(), " Brought them back. ".into());
        assert_eq!(
            goal_result(&office, &goal(&office)),
            "Stored carts. Brought them back."
        );
        // The plan's own words come first.
        office
            .results
            .insert("S-1".into(), "Carts are saved.".into());
        assert_eq!(goal_result(&office, &goal(&office)), "Carts are saved.");
        // A long one is cut at a whole word.
        office.results.insert("S-1".into(), "word ".repeat(60));
        let cut = goal_result(&office, &goal(&office));
        assert!(cut.chars().count() <= RESULT_MAX, "{cut}");
        assert!(cut.ends_with("word…"), "{cut}");
        // A task's own result is what it said, trimmed.
        assert_eq!(task_result(&office, "S-3"), "Brought them back.");
        assert_eq!(task_result(&office, "S-9"), "");
    }

    #[test]
    fn a_field_left_empty_is_absent() {
        assert_eq!(absent("  "), None);
        assert_eq!(absent(" ~/games/garden "), Some("~/games/garden".into()));
    }
}
