//! The frame of every team page: the team sidebar, the page's main area and,
//! for a page that has one, a rail on the right.
//!
//! A page draws itself whole with `page`, and `stream` draws it again every
//! second the office changed. So a page's content must be a pure function of
//! the office it is given; what the owner has typed or opened in the page
//! itself (a text box, an open note) belongs under `data-ignore-morph`, which
//! tells Datastar to leave an element as it is when the page is patched.

use topcoat::view::{Child, Unescaped, View, component, view};

use super::{Floor, LeadEntry, Team, faces, lead_href};
use crate::pages::office::model::Phase;
use crate::ui;

/// Where the owner is, so the sidebar marks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Here {
    Hq,
    /// A lead's conversation, by project.
    Lead(String),
    Office,
    New,
    Studio,
}

/// A row of the sidebar that is a link: a picture, two lines of words and, at
/// the end, a count.
const ROW: &str = "grid grid-cols-[40px_minmax(0,1fr)_auto] items-center gap-x-[11px] gap-y-px rounded-[11px] px-2.5 py-2 hover:bg-bg-3 aria-[current=page]:bg-bg-4";
/// What waits for the owner, as a number in a yellow ball.
const COUNT: &str = "tm-count col-start-3 row-start-1 grid h-[21px] min-w-[21px] place-items-center rounded-full bg-attn px-1.5 text-[11.5px] font-bold tabular-nums text-[#1d1300]";
/// The few words under a name.
const META: &str = "truncate text-[12.5px] text-fg-3";

/// A page of the team: the sidebar on the left, `child` as the main area and,
/// when there is one, `rail` on the right. The main area has room around it
/// unless it is `flush`, which gives the page the whole area, for one that
/// scrolls inside itself or fills it. The rail is hidden on a phone, and the
/// sidebar becomes one compact header above the content; what was spent moves
/// from it to the foot of HQ.
#[component]
pub async fn page(
    team: Team,
    here: Here,
    #[default] rail: Child<'_>,
    #[default] flush: bool,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    let main = if flush {
        "flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto max-[760px]:overflow-visible"
    } else {
        "min-h-0 min-w-0 flex-1 overflow-y-auto px-8 py-9 max-[760px]:overflow-visible max-[760px]:px-4 max-[760px]:py-6"
    };
    let (spent_today, daily_usd) = (team.spent_today, team.daily_usd);
    let at_hq = here == Here::Hq;
    Ok(view! {
        <div id="team" class="flex h-dvh bg-bg max-[760px]:h-auto max-[760px]:min-h-dvh max-[760px]:flex-col">
            sidebar(team: team, here: here)
            <main id="team-main" class=(main)>(child)</main>
            <aside id="team-rail" class="w-[272px] shrink-0 overflow-y-auto border-l border-line bg-bg-2 empty:hidden max-[760px]:hidden" aria-label="Beside the page">(rail)</aside>
            if at_hq {
                <footer id="team-spend" class="hidden border-t border-line px-4 py-4 max-[760px]:block">
                    spend(spent: spent_today, limit: daily_usd)
                </footer>
            }
        </div>
    })
}

/// What was spent over the last 24 hours against the daily limit, as a line
/// and a bar.
#[component]
async fn spend(spent: f64, limit: f64) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex flex-col gap-1.5" title="Spend over the last 24 hours, against the daily limit">
            <span class="flex items-baseline justify-between text-[12.5px]">
                <span class="text-fg-3">"Spent today"</span>
                <span class="tabular-nums text-[13.5px]"><b class="font-semibold">(ui::usd(spent))</b><span class="text-fg-3">" of " (ui::limit(limit))</span></span>
            </span>
            ui::meter(value: spent, max: limit)
        </div>
    })
}

/// The studio's name and settings, HQ, a lead for each project, a way to hire
/// another, the office and what was spent today. On a phone it is a single
/// row: the mark, HQ with its count, the leads as faces that scroll sideways
/// (the one open is ringed), then the office and the settings as small
/// buttons. The words that go with a face stay for a screen reader.
#[component]
async fn sidebar(team: Team, here: Here) -> topcoat::Result<impl View> {
    let Team {
        leads,
        needs,
        spent_today,
        daily_usd,
        floor,
    } = team;
    let open_lead = match &here {
        Here::Lead(project) => Some(project.clone()),
        _ => None,
    };
    let at = |here_is: Here| (here == here_is).then_some("page");
    let (hq, office, new, settings) = (
        at(Here::Hq),
        at(Here::Office),
        at(Here::New),
        at(Here::Studio),
    );
    Ok(view! {
        <aside id="team-sidebar" class="flex h-dvh w-[288px] shrink-0 flex-col gap-1 overflow-y-auto border-r border-line bg-bg-2" aria-label="Team">
            <div class="tm-top flex items-center gap-2 px-4 pb-3 pt-3.5">
                <a href="/" class="tm-mark flex items-center gap-2.5 text-[15px] font-semibold" aria-label="studio">
                    <span class="grid size-[26px] shrink-0 place-items-center rounded-[7px] bg-primary text-on-primary">
                        <svg class="size-4" aria-hidden="true"><use href="#logo"></use></svg>
                    </span>
                    <span class="tm-words">"studio"</span>
                </a>
                <a href="/studio" class="tm-gear ml-auto grid size-8 place-items-center rounded-lg text-fg-3 hover:bg-bg-3 hover:text-fg aria-[current=page]:bg-bg-4 aria-[current=page]:text-fg" aria-label="Studio settings" title="Studio settings" aria-current=(settings)>
                    ui::icon(name: "settings-2")
                </a>
            </div>
            <div class="tm-hqbox px-2">
                <a href="/" class=(format!("tm-hq {ROW}")) aria-current=(hq)>
                    <span class="tm-hq-icon row-span-2 grid size-10 place-items-center rounded-xl border border-line-2 bg-bg-3 text-fg">ui::icon(name: "building-2")</span>
                    <b class="text-[14.5px] font-semibold">"HQ"</b>
                    <span class=(format!("tm-words col-span-2 {META}"))>"Your morning round-up"</span>
                    if needs > 0 { <span class=(COUNT)>(needs)</span> }
                </a>
            </div>
            <h2 class="tm-title px-[18px] pb-1.5 pt-[18px] text-[11px] font-semibold uppercase tracking-[.07em] text-fg-3">"Your leads"</h2>
            <ul class="tm-leads flex flex-col gap-0.5 px-2">
                for entry in leads {
                    <li>lead_row(current: open_lead.as_deref() == Some(entry.project.as_str()), entry: entry)</li>
                }
                <li>
                    <a href="/new" class=(format!("tm-new {ROW} text-fg-3")) aria-current=(new) title="New project">
                        <span class="row-span-2 grid size-10 place-items-center rounded-full border-[1.5px] border-dashed border-line-3">ui::icon(name: "plus")</span>
                        <span class="tm-words font-medium text-fg-2">"New project"</span>
                        <span class=(format!("tm-words col-span-2 {META}"))>"Hire a lead for it"</span>
                    </a>
                </li>
            </ul>
            <div class="tm-dock mt-auto flex flex-col gap-3 border-t border-line px-4 py-3.5">
                mini_floor(floor: floor, current: office)
                <div class="tm-spend">spend(spent: spent_today, limit: daily_usd)</div>
            </div>
        </aside>
    })
}

/// The company floor at thumbnail size, drawn by the same code as the office
/// itself, and a way into the office. On a phone it is only a small button
/// that leads there.
#[component]
async fn mini_floor(floor: Floor, current: Option<&'static str>) -> topcoat::Result<impl View> {
    let Floor { svg, ratio, closed } = floor;
    let drawing = Unescaped::new_unchecked(svg);
    let class = if closed {
        "scene mini closed"
    } else {
        "scene mini"
    };
    Ok(view! {
        <a href="/office" class="mini-floor group" aria-current=(current) aria-label="The office, live" title="The office">
            <span class=(class) style=(format!("--ar: {ratio}")) data-focus="all" aria-hidden="true">(drawing)</span>
            <span class="mini-floor-tag">
                ui::icon(name: "building-2", small: true)
                <span class="tm-words">"The office"</span>
            </span>
        </a>
    })
}

/// One lead in the sidebar: its face, its name, its project and phase, how far
/// the project has come, and what waits for the owner. On a phone, the face
/// and the count.
#[component]
async fn lead_row(entry: LeadEntry, current: bool) -> topcoat::Result<impl View> {
    let LeadEntry {
        project,
        lead,
        name,
        role,
        tint,
        phase,
        phase_words,
        progress,
        needs,
    } = entry;
    // A project without a lead has no hire to draw: its own name will do.
    let seed = lead.unwrap_or_else(|| project.clone());
    let done = (progress.clamp(0.0, 1.0) * 100.0).round();
    let title = format!("{name}, {project}");
    Ok(view! {
        <a href=(lead_href(&project)) class=(format!("tm-lead {ROW} pj-{tint}")) aria-current=(current.then_some("page")) data-phase=(phase_key(phase)) title=(title)>
            <span class="row-span-2">faces::face(seed: seed, role: role, tint: tint, size: 40)</span>
            <b class="tm-words truncate text-[14.5px] font-semibold">(name)</b>
            <span class="tm-words col-span-2 flex min-w-0 flex-col gap-[5px]">
                <span class=(META)>(project) " · " (phase_words)</span>
                <span class="block h-1 overflow-hidden rounded-full bg-bg-4" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow=(format!("{done:.0}"))>
                    <i class="block h-full rounded-full bg-[var(--pj)]" style=(format!("width:{done:.0}%"))></i>
                </span>
            </span>
            if needs > 0 { <span class=(COUNT)>(needs)</span> }
        </a>
    })
}

/// A phase as a style name, for the `data-phase` of a lead's row.
fn phase_key(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Planning => "planning",
        Phase::Building => "building",
        Phase::Testing => "testing",
        Phase::Releasing => "releasing",
        Phase::Live => "live",
        Phase::Paused => "paused",
    }
}
