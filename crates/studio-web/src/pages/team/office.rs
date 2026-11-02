//! The office: the company floor, inside the team's frame. Picking a sign or a
//! person on it shows a floating card for that project's lead, with a way to
//! talk to them. Like every team page it keeps up from the one stream of the
//! team pages; what the owner picked and the corner the floor is seen from are
//! signals the stream is opened with.

use serde::Deserialize;
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, Signals},
    router::route,
    view::{View, component, view},
};

use super::shell::{self, Here};
use super::stream::OPEN;
use super::talk::look_href;
use super::talk::thread::{self, Person};
use super::{Team, faces, lead_href};
use crate::pages::office::hq::{self, Parts};
use crate::pages::office::model::{Element, Turn};
use crate::pages::office::{Office, facts, gather, key, safe};
use crate::{AppConfig, ui};

/// What the page's signals hold: what the owner picked and the corner the
/// floor is seen from.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Picked {
    #[serde(default)]
    pub sel: String,
    #[serde(default)]
    pub turn: i64,
}

/// The card of a project's lead, shown beside the floor while one of that
/// project's signs or people is picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Peek {
    pub project: String,
    /// The lead; none when nobody leads the project.
    pub lead: Option<Person>,
    pub tint: usize,
    pub phase_words: String,
    pub needs: usize,
    /// What the lead says of the project: its standup, or the facts.
    pub line: String,
    /// What `?look=` opens, when something is ready to ship.
    pub ready: Option<String>,
}

/// `sel` as the floor writes it, kept to what it can write: a kind, a slash and
/// an id of safe characters.
pub fn clean(sel: &str) -> String {
    match sel.split_once('/') {
        Some((kind @ ("tickets" | "hires" | "goals" | "projects"), id)) => key(kind, id),
        Some(("elements", id)) if Element::from_key(id).is_some() => key("elements", id),
        _ => String::new(),
    }
}

/// The project whose lead's card `sel` shows: its sign, its lead, any of its
/// work and the people at work on it. A person at work on two projects belongs
/// to the first.
fn project_of(office: &Office, sel: &str) -> Option<String> {
    let (kind, id) = sel.split_once('/')?;
    let names = office.project_names();
    let working = |t: &&studio_core::ticket::Ticket| {
        !matches!(t.state.kind(), "landed" | "failed" | "cancelled")
    };
    match kind {
        "projects" => names.into_iter().find(|p| safe(p) == id),
        "tickets" => office
            .tickets
            .iter()
            .find(|t| safe(&t.id.to_string()) == id)
            .map(|t| t.project.to_string()),
        "goals" => office
            .tickets
            .iter()
            .find(|t| safe(&t.id.to_string()) == id)
            .map(|t| t.project.to_string()),
        "hires" => names.into_iter().find(|p| {
            office
                .project(p)
                .and_then(|view| view.lead.as_deref())
                .is_some_and(|lead| safe(lead) == id)
                || office
                    .tickets
                    .iter()
                    .filter(working)
                    .any(|t| t.project.to_string() == *p && safe(&t.hire.to_string()) == id)
                || office.judges.iter().any(|(ticket, judge)| {
                    safe(&judge.hire) == id
                        && office
                            .ticket(ticket)
                            .is_some_and(|t| t.project.to_string() == *p)
                })
        }),
        _ => None,
    }
}

/// The card to show for the pick `sel`, if it is a project's.
pub fn peek_for(team: &Team, office: &Office, sel: &str) -> Option<Peek> {
    let project = project_of(office, sel)?;
    let entry = team.lead(&project)?;
    let lead = thread::lead_of(office, &project);
    let ready = thread::ready_goals(office, &project)
        .first()
        .map(|goal| goal.plan.id.to_string())
        .or_else(|| {
            thread::ready_tasks(office, &project)
                .first()
                .map(|task| task.id.to_string())
        });
    Some(Peek {
        line: office
            .standups
            .get(&project)
            .map(|standup| standup.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| facts_line(office, &project)),
        lead,
        tint: entry.tint,
        phase_words: entry.phase_words.clone(),
        needs: entry.needs,
        ready,
        project,
    })
}

/// What a project is up to, from the facts: what is at work and what waits for
/// the owner.
fn facts_line(office: &Office, project: &str) -> String {
    let at_work = office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project && ui::at_work(t.state.kind()))
        .count();
    let needs = facts::project_needs(office, project);
    let mut parts = Vec::new();
    if at_work > 0 {
        parts.push(format!(
            "{at_work} {} at work.",
            if at_work == 1 { "task is" } else { "tasks are" }
        ));
    }
    if needs > 0 {
        parts.push(format!("{needs} for you."));
    }
    if parts.is_empty() {
        parts.push("Nothing is open right now.".to_string());
    }
    parts.join(" ")
}

/// Where the floor and the card of the pick's lead are drawn from at once, so
/// that a pick or a turn shows without waiting for the stream.
const SCENE: &str = "/team/office/scene";
/// What a click on the floor does: it picks what is under the pointer, or
/// nothing, and has the floor drawn as picked, with the card of the pick's lead.
const PICK: &str = "const t = evt.target.closest('[data-sel]'); $sel = t ? t.dataset.sel : ''; @get('/team/office/scene', {requestCancellation: 'disabled'})";
/// The same from the keyboard, for what on the floor is not a button.
const PICK_KEY: &str = "const t = evt.target.closest('.iso [data-sel], button[data-sel]'); if (t && (evt.key === 'Enter' || evt.key === ' ')) { evt.preventDefault(); $sel = t.dataset.sel; @get('/team/office/scene', {requestCancellation: 'disabled'}) }";

/// The whole page: the frame around the floor and its controls, with `sel`
/// picked and the floor seen from the corner `turn`.
#[component]
pub async fn office_view(
    team: Team,
    office: Office,
    turn: Turn,
    sel: String,
) -> topcoat::Result<impl View> {
    let Parts {
        scene,
        projects,
        closed,
        turn,
        ..
    } = Parts::of(&office, turn);
    let peek = peek_for(&team, &office, &sel);
    let signals = format!(
        "{{sel: '{sel}', zoom: 1, hints: false, turn: {}, _dragging: false}}",
        turn.index()
    );
    Ok(view! {
        shell::page(team: team, here: Here::Office,
            // The signals are set once, when the page opens: a patch that
            // draws the floor from another corner or with another pick must
            // not put the zoom and the hints back.
            <div id="office-page" class="office-page world" data-signals__ifmissing=(signals)>
                // The one stream of the team pages, opened again whenever the
                // pick or the corner the floor is seen from changes, so that it
                // draws the floor as picked.
                <div id="office-live" hidden="" data-effect=(format!("$sel; $turn; {OPEN}"))></div>
                <h1 class="sr-only">"The office"</h1>
                hq::view_controls(scene: SCENE)
                <p class="office-hint">
                    ui::icon(name: "sprout", small: true)
                    "Click anyone to talk to them"
                </p>
                hq::wrap(
                    hq::floor_scene(scene: scene, projects: projects, closed: closed, sel: sel, turn: turn, pick: PICK, pick_key: PICK_KEY)
                )
                peek_card(peek: peek)
                hq::legend()
            </div>
        )
    })
}

/// The lead's card, or nothing while nothing is picked.
#[component]
async fn peek_card(peek: Option<Peek>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="office-peek" class="office-peek" aria-live="polite">
            if let Some(peek) = peek {
                lead_card(peek: peek)
            }
        </div>
    })
}

#[component]
async fn lead_card(peek: Peek) -> topcoat::Result<impl View> {
    let Peek {
        project,
        lead,
        tint,
        phase_words,
        needs,
        line,
        ready,
    } = peek;
    let (name, seed, role) = match &lead {
        Some(lead) => (lead.name.clone(), lead.hire.clone(), lead.role),
        None => (
            project.clone(),
            project.clone(),
            crate::pages::office::model::Role::Lead,
        ),
    };
    let talk = if lead.is_some() {
        format!("Talk to {name}")
    } else {
        format!("Open {project}")
    };
    Ok(view! {
        <section class="flex flex-col gap-3 rounded-2xl border border-line-2 bg-bg-2 p-4 shadow-pop" aria-label=(format!("{name}, lead of {project}"))>
            <div class="flex items-center gap-3">
                faces::face(seed: seed, role: role, tint: tint, size: 40)
                <span class="min-w-0 flex-1">
                    <b class="block text-[15px] font-semibold leading-5">(name)</b>
                    <span class="block text-[12.5px] text-fg-3">
                        if lead.is_some() { "Leads " (project.clone()) } else { (project.clone()) }
                        " · " (phase_words)
                    </span>
                </span>
                if needs > 0 {
                    <span class="grid h-[22px] min-w-[22px] place-items-center rounded-full bg-attn px-1.5 text-[12px] font-semibold tabular-nums text-on-primary" title="Waits for you">(needs)</span>
                }
            </div>
            <p>(line)</p>
            <div class="flex flex-wrap gap-2">
                <a href=(lead_href(&project)) class=(ui::PRIMARY_SM)>(talk)</a>
                if let Some(look) = ready {
                    <a href=(look_href(&project, &look)) class=(ui::GHOST_SM)>"What's ready"</a>
                }
            </div>
        </section>
    })
}

/// The floor and the card of the pick's lead, right away, for the pick and the
/// corner in the page's signals.
#[route(GET "/team/office/scene")]
async fn show_scene(cx: &Cx, signals: Option<Signals<Picked>>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let pick = signals.map(|s| s.0).unwrap_or_default();
    let office = gather(config).await?;
    let (team, sel) = (Team::of(&office), clean(&pick.sel));
    let Parts {
        scene,
        projects,
        closed,
        turn,
        ..
    } = Parts::of(&office, Turn::new(pick.turn));
    let peek = peek_for(&team, &office, &sel);
    let html = ui::html(
        cx,
        view! { cx =>
            hq::floor_scene(scene: scene, projects: projects, closed: closed, sel: sel, turn: turn, pick: PICK, pick_key: PICK_KEY)
            peek_card(peek: peek)
        },
    )
    .await?;
    Ok(PatchElements::new(html))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use studio_core::ticket::Ticket;
    use studiod::api::{ProjectView, Standup};

    fn ticket(id: &str, project: &str, state: Value, hire: &str) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": project, "title": id, "goal": "", "hire": hire,
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    fn studio(tickets: Vec<Ticket>) -> (Team, Office) {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects": ["shop", "api"],
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
            ],
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
        ];
        let team = Team::of(&office);
        (team, office)
    }

    #[test]
    fn a_sign_a_lead_and_the_people_at_work_show_their_projects_lead() {
        let (team, office) = studio(vec![
            ticket("S-1", "shop", json!("working"), "dev"),
            ticket("A-1", "api", json!("landed"), "dev"),
        ]);
        let who = |sel: &str| peek_for(&team, &office, sel).map(|p| p.project);
        assert_eq!(who("projects/shop").as_deref(), Some("shop"));
        assert_eq!(who("projects/api").as_deref(), Some("api"));
        assert_eq!(who("hires/nora").as_deref(), Some("shop"));
        // Dev's work in the api is done; the shop is where Dev is at work.
        assert_eq!(who("hires/dev").as_deref(), Some("shop"));
        assert_eq!(who("tickets/S-1").as_deref(), Some("shop"));
        assert_eq!(who("tickets/A-1").as_deref(), Some("api"));
        assert_eq!(who("elements/lounge"), None);
        assert_eq!(who(""), None);
        assert_eq!(who("projects/garden"), None);
    }

    #[test]
    fn the_card_says_the_lead_and_what_is_going_on() {
        let (team, mut office) = studio(vec![ticket("S-1", "shop", json!("working"), "dev")]);
        let card = peek_for(&team, &office, "projects/shop").unwrap();
        assert_eq!(card.lead.as_ref().map(|p| p.name.as_str()), Some("Nora"));
        assert_eq!(card.phase_words, "Building");
        assert_eq!(card.line, "1 task is at work.");
        assert_eq!(card.ready, None);
        // The lead's standup is its own words.
        office.standups.insert(
            "shop".into(),
            Standup {
                at: 1,
                text: " The logo's done. ".into(),
            },
        );
        assert_eq!(
            peek_for(&team, &office, "projects/shop").unwrap().line,
            "The logo's done."
        );
        // A project nobody leads still has a card, called by its name.
        let card = peek_for(&team, &office, "projects/api").unwrap();
        assert_eq!(card.lead, None);
        assert_eq!(card.line, "Nothing is open right now.");
    }

    #[test]
    fn what_is_ready_opens_the_work_to_look_at() {
        let (team, mut office) = studio(vec![ticket("S-1", "shop", json!("ready"), "dev")]);
        assert_eq!(
            peek_for(&team, &office, "projects/shop")
                .unwrap()
                .ready
                .as_deref(),
            Some("S-1")
        );
        office.tickets.clear();
        assert_eq!(
            peek_for(&team, &office, "projects/shop").unwrap().ready,
            None
        );
    }

    #[test]
    fn a_pick_is_kept_to_what_the_floor_writes() {
        assert_eq!(clean("projects/shop"), "projects/shop");
        assert_eq!(clean("hires/dev"), "hires/dev");
        assert_eq!(clean("elements/lounge"), "elements/lounge");
        assert_eq!(clean("elements/nope"), "");
        assert_eq!(clean("tickets/S-1\"><script>"), "tickets/S-1script");
        assert_eq!(clean("other/x"), "");
        assert_eq!(clean(""), "");
    }
}
