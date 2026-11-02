//! The home page's pieces: the top bar's counters and spend, the company
//! floor with a sign for each project, and the side panel's frame. Each
//! piece that changes has an `id` and is re-sent whole by the live stream.

use topcoat::view::{BoxView, Child, Unescaped, View, ViewExt, component, view};

use super::hq_scene::{self, phase_key};
use super::inspector::Detail;
use super::iso::looks;
use super::model::{Doing, Hq, Phase, Pin, PinWhat, Role, Scene, Team, Turn};
use super::panel::{self, Extra};
use super::{Office, facts, plan, safe};
use crate::ui;

/// Zoom steps and limits; 1 is the whole floor.
const ZOOM_MAX: &str = "2.6";
const ZOOM_STEP: &str = "0.4";
/// The most faces a sign shows before it counts the rest.
const FACES: usize = 4;

/// Sets the tab and shows it: a picked task, hire, goal or project is put
/// away.
fn go(tab: &str) -> String {
    format!("$tab = '{tab}'; if ($sel) {{ $sel = ''; @get('/office/panel') }}")
}

/// One click handler for everything on the floor with a `data-sel`.
pub const PICK: &str = "const t = evt.target.closest('[data-sel]'); if (t) { $sel = t.dataset.sel; @get('/office/panel'); window.innerWidth <= 900 && document.getElementById('hq-panel').scrollIntoView({behavior: 'smooth', block: 'start'}) }";
/// The same from the keyboard, for floor things that aren't buttons.
const PICK_KEY: &str = "const t = evt.target.closest('.iso [data-sel]'); if (t && (evt.key === 'Enter' || evt.key === ' ')) { evt.preventDefault(); $sel = t.dataset.sel; @get('/office/panel'); window.innerWidth <= 900 && document.getElementById('hq-panel').scrollIntoView({behavior: 'smooth', block: 'start'}) }";

/// The top bar's figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    pub projects: usize,
    /// Projects with a release under way: checking, waiting or publishing.
    pub releasing: usize,
    /// Everything that waits on the owner, across the projects.
    pub needs_you: usize,
    /// Spent in the last 24 hours.
    pub spent: f64,
    /// Agents at work, and how many may be at once.
    pub working: u64,
    pub slots: u64,
}

impl Hud {
    pub fn of(office: &Office, hq: &Hq) -> Hud {
        Hud {
            projects: hq.teams.len(),
            releasing: office
                .projects
                .iter()
                .filter(|p| p.release.as_ref().is_some_and(facts::is_releasing))
                .count(),
            needs_you: hq.owner_needs,
            spent: office.status.spent_24h,
            working: office.status.running,
            slots: office.status.max_runs,
        }
    }
}

/// Everything the page draws apart from the side panel.
#[derive(Clone, Debug)]
pub struct Parts {
    pub hud: Hud,
    pub scene: Scene,
    pub projects: Vec<String>,
    pub closed: bool,
    /// The corner the floor is drawn from.
    pub turn: Turn,
}

impl Parts {
    /// Everything but the panel, with the floor seen from `turn`.
    pub fn of(office: &Office, extra: &Extra, turn: Turn) -> Parts {
        let hq = plan::hq(office, extra);
        Parts {
            hud: Hud::of(office, &hq),
            scene: hq_scene::render(&hq, turn),
            projects: hq.teams.iter().map(|t| t.project.clone()).collect(),
            closed: hq.closed,
            turn,
        }
    }
}

/// What the owner is told of the company, and what they have spent.
#[component]
pub async fn figures(hud: Hud) -> topcoat::Result<impl View> {
    let needs = hud.needs_you > 0;
    let projects = if hud.projects == 1 {
        "project"
    } else {
        "projects"
    };
    let spent = if hud.slots > 0 {
        format!(
            "Spent in the last 24 hours. {} of {} agents at work.",
            hud.working, hud.slots
        )
    } else {
        "Spent in the last 24 hours".to_string()
    };
    Ok(view! {
        <div id="hq-figures" class="contents">
            <div class="stats" aria-label="The company today">
                <button type="button" class="stat" data-on:click=(go("projects"))>
                    <b>(hud.projects)</b>
                    <span class="lbl">(projects)</span>
                </button>
                <button type="button" class="stat" data-on:click=(go("projects"))>
                    <b>(hud.releasing)</b>
                    <span class="lbl">"releasing"</span>
                </button>
                <button type="button" class=(if needs { "stat needs" } else { "stat" }) data-on:click=(go("needs"))>
                    if needs {
                        <span class="ping"></span>
                    } else {
                        <span class="dot ok"></span>
                    }
                    <b>(hud.needs_you)</b>
                    <span class="lbl">"need you"</span>
                </button>
            </div>
            <a href="/?tab=money" class="ml-auto flex items-baseline gap-2 whitespace-nowrap text-[12.5px] text-fg-3 hover:text-fg-2" title=(spent)>
                <span class="max-[520px]:hidden">"Spent today"</span>
                <b class="text-[17px] font-semibold tabular-nums text-fg">(ui::usd(hud.spent))</b>
            </a>
        </div>
    })
}

/// Sets `$zoom` to `to`, an expression, keeping what is in the middle of the
/// view in the middle: a bigger floor scrolls inside its wrap.
fn zoom(to: &str) -> String {
    format!(
        "const w = document.getElementById('hq-wrap'); \
         const x = (w.scrollLeft + w.clientWidth / 2) / w.scrollWidth, y = (w.scrollTop + w.clientHeight / 2) / w.scrollHeight; \
         $zoom = {to}; \
         requestAnimationFrame(() => w.scrollTo(x * w.scrollWidth - w.clientWidth / 2, y * w.scrollHeight - w.clientHeight / 2))"
    )
}

/// A quarter turn to the left or the right; the floor is fetched at once, so
/// the turn shows without waiting for the live stream.
const TURN_LEFT: &str = "$turn = ($turn + 3) % 4; @get('/office/scene')";
const TURN_RIGHT: &str = "$turn = ($turn + 1) % 4; @get('/office/scene')";
/// Q turns left and E turns right, anywhere on the page but in a field and
/// with no modifier key held.
const TURN_KEYS: &str = "if (!(evt.ctrlKey || evt.metaKey || evt.altKey || evt.shiftKey)) { \
    const t = evt.target; \
    const typing = t && (t.isContentEditable || (t.closest && t.closest('input, textarea, select, [contenteditable]'))); \
    const k = (evt.key || '').toLowerCase(); \
    if (!typing && (k === 'q' || k === 'e')) { $turn = ($turn + (k === 'q' ? 3 : 1)) % 4; @get('/office/scene') } }";

/// Dragging the floor with the mouse while zoomed in scrolls its frame. The
/// drag lives on the frame as `_drag`; a drag of more than 4 pixels is not a
/// click, and touch keeps its own scrolling.
const DRAG_DOWN: &str = "if (evt.button === 0 && evt.pointerType === 'mouse' && $zoom > 1) { \
    el._drag = {x: evt.clientX, y: evt.clientY, l: el.scrollLeft, t: el.scrollTop, moved: false, down: true} }";
const DRAG_MOVE: &str = "const d = el._drag; if (d && d.down) { \
    if (!(evt.buttons & 1)) { d.down = false; $_dragging = false } else { \
        const dx = evt.clientX - d.x, dy = evt.clientY - d.y; \
        if (!d.moved && Math.hypot(dx, dy) > 4) { d.moved = true; $_dragging = true; window.getSelection().removeAllRanges() } \
        if (d.moved) { el.scrollLeft = d.l - dx; el.scrollTop = d.t - dy } } }";
const DRAG_UP: &str = "const d = el._drag; if (d) { d.down = false; $_dragging = false; \
    setTimeout(() => { if (el._drag === d) { el._drag = null } }, 0) }";
/// Before anything under the pointer sees the click that ends a drag.
const DRAG_CLICK: &str = "const d = el._drag; if (d && d.moved) { evt.stopPropagation(); evt.preventDefault(); el._drag = null }";
const DRAG_CLASS: &str = "{'cursor-grab': $zoom > 1 && !$_dragging, 'cursor-grabbing [&_*]:cursor-grabbing! select-none': $_dragging}";

/// The floor's frame, which scrolls when the floor is zoomed in.
#[component]
pub async fn wrap(child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="hq-wrap" class="scene-wrap" data-class=(DRAG_CLASS)
            data-on:pointerdown=(DRAG_DOWN) data-on:pointermove__window=(DRAG_MOVE)
            data-on:pointerup__window=(DRAG_UP) data-on:pointercancel__window=(DRAG_UP)
            data-on:click__capture=(DRAG_CLICK)>
            (child)
        </div>
    })
}

/// The view controls above the floor.
#[component]
pub async fn view_controls() -> topcoat::Result<impl View> {
    let out = zoom(&format!(
        "Math.max(1, Math.round(($zoom - {ZOOM_STEP}) * 10) / 10)"
    ));
    let into = zoom(&format!(
        "Math.min({ZOOM_MAX}, Math.round(($zoom + {ZOOM_STEP}) * 10) / 10)"
    ));
    let pressed = "aria-pressed:bg-bg-4 aria-pressed:text-fg aria-pressed:shadow-[inset_0_0_0_1px_var(--line-2)]";
    Ok(view! {
        <div class="view-ctl" role="toolbar" aria-label="View" data-on:keydown__window=(TURN_KEYS)>
            <button type="button" class=(ui::GHOST_SM) aria-label="Turn left" title="Turn left (Q)" data-on:click=(TURN_LEFT)>"↺"</button>
            <button type="button" class=(ui::GHOST_SM) aria-label="Turn right" title="Turn right (E)" data-on:click=(TURN_RIGHT)>"↻"</button>
            <button type="button" class=(ui::GHOST_SM) aria-label="Zoom out" data-attr:disabled="$zoom <= 1" data-on:click=(out)>"−"</button>
            <button type="button" class=(ui::GHOST_SM) data-on:click="$zoom = 1">"Fit"</button>
            <button type="button" class=(ui::GHOST_SM) aria-label="Zoom in" data-attr:disabled=(format!("$zoom >= {ZOOM_MAX}")) data-on:click=(into)>"+"</button>
            <button type="button" class=(format!("{} {pressed}", ui::GHOST_SM)) aria-pressed="false" data-attr:aria-pressed="$hints ? 'true' : 'false'" data-on:click="$hints = !$hints">"What's what"</button>
        </div>
    })
}

/// What the props on a team's area mean, under the floor.
#[component]
pub async fn legend() -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="world-foot" aria-label="What the props mean">
            <span><i class="ph-building"></i>"Building: people at desks"</span>
            <span><i class="ph-testing"></i>"Testing: test bench"</span>
            <span><i class="ph-releasing"></i>"Releasing: launch crate"</span>
            <span><i class="ph-live"></i>"Live: plaque"</span>
        </div>
    })
}

/// The floor: the drawing, the signs and labels pinned over it, and the
/// closed sign, with `sel` marked. When a project is picked, the rest of the
/// floor fades back. Zoom and hints come from the page's signals and are put
/// back by Datastar whenever a patch changes the element.
#[component]
pub async fn floor_scene(
    scene: Scene,
    projects: Vec<String>,
    closed: bool,
    sel: String,
    turn: Turn,
) -> topcoat::Result<impl View> {
    let style = format!("--ar: {} / {}", scene.width, scene.height);
    let class = if closed { "scene closed" } else { "scene" };
    let focus = sel
        .strip_prefix("projects/")
        .and_then(|id| projects.iter().position(|p| safe(p) == id))
        .map_or_else(|| "all".to_string(), |at| facts::wrap(at).to_string());
    let svg = Unescaped::new_unchecked(picked(scene.svg, &sel));
    let [f0, f1, f2, f3] = scene.forms.map(|f| f.to_string());
    Ok(view! {
        <div id="hq-scene" class=(class) style=(style) data-turn=(turn.index().to_string()) data-focus=(focus) data-f0=(f0) data-f1=(f1) data-f2=(f2) data-f3=(f3) data-style="{'--zoom': $zoom}" data-class:show-hints="$hints" data-on:click=(PICK) data-on:keydown=(PICK_KEY)>
            (svg)
            <div class="ov">
                for pin in scene.pins {
                    pin_view(pin: pin, projects: projects.clone(), sel: sel.clone())
                }
            </div>
            if closed {
                <div class="closed-sign" id="closed-sign">"Office closed · nothing new starts"</div>
            }
        </div>
    })
}

/// Marks whatever on the drawing is `sel`. Keys are made by `key`, so they
/// need no escaping.
fn picked(svg: String, sel: &str) -> String {
    if sel.is_empty() {
        return svg;
    }
    let at = format!(r#"data-sel="{sel}""#);
    svg.replace(&at, &format!(r#"{at} data-picked="""#))
}

/// What a sign's phase is called in a style: a release that failed blocks
/// the project.
fn phase_class(team: &Team) -> &'static str {
    if team.release.as_ref().is_some_and(|r| r.failed) {
        "blocked"
    } else {
        phase_key(team.phase)
    }
}

/// What a phase is called on a sign.
fn phase_word(team: &Team) -> &'static str {
    if team.release.as_ref().is_some_and(|r| r.failed) {
        return "Release failed";
    }
    match team.phase {
        Phase::Idle => "Idle",
        Phase::Planning => "Planning",
        Phase::Building => "Building",
        Phase::Testing => "Testing",
        Phase::Releasing => "Releasing",
        Phase::Live => "Live",
        Phase::Paused => "Paused",
    }
}

/// What a project is up to, in a line: "Releasing v1.2 · 4 of 5", "v1.2 waits
/// for you", "Live · v2.0", else its milestone's progress.
pub fn sign_line(team: &Team) -> String {
    if let Some(r) = &team.release {
        if r.failed {
            return format!("v{} failed", r.version);
        }
        if r.waiting {
            return format!("v{} waits for you", r.version);
        }
        return format!("Releasing v{} · {} of {}", r.version, r.done, r.total);
    }
    if team.phase == Phase::Live
        && let Some(v) = &team.live
    {
        return format!("Live · v{v}");
    }
    match &team.milestone {
        Some(m) if m.total > 0 => {
            let what = match &m.version {
                Some(v) => format!("v{v} {}", m.title),
                None => m.title.clone(),
            };
            format!("{what} · {} of {}", m.done, m.total)
        }
        Some(m) => m.title.clone(),
        None => phase_word(team).to_string(),
    }
}

/// How far along the sign's bar is, in percent, and the words by it.
fn progress(team: &Team) -> (usize, String) {
    let share = |done: usize, total: usize| (done * 100).checked_div(total).unwrap_or(0);
    if let Some(r) = &team.release {
        let words = if r.failed {
            format!("v{} failed", r.version)
        } else if r.waiting {
            format!("v{} waits for you", r.version)
        } else {
            format!("{} of {} steps", r.done, r.total)
        };
        return (share(r.done, r.total), words);
    }
    if team.phase == Phase::Live {
        let words = team
            .live
            .as_ref()
            .map_or_else(|| "Live".to_string(), |v| format!("Live · v{v}"));
        return (100, words);
    }
    match &team.milestone {
        Some(m) if m.total > 0 => (
            share(m.done, m.total),
            format!("{} of {} tasks", m.done, m.total),
        ),
        Some(_) => (0, "Planning the tasks".to_string()),
        None => (0, "Nothing open".to_string()),
    }
}

/// What a role is called to the owner.
fn role_word(role: Role) -> &'static str {
    match role {
        Role::Engineer => "coder",
        Role::Writer => "writer",
        Role::Designer => "designer",
        Role::Producer => "planner",
        Role::Reviewer => "reviewer",
        Role::Critic => "critic",
        Role::Approver | Role::Router => "front desk",
        Role::Lead => "lead",
        Role::Owner => "owner",
        Role::Other => "teammate",
    }
}

/// Who is on a project, as (hire, name, role): its lead, then everyone at a
/// desk, once each.
fn faces(team: &Team) -> Vec<(String, String, Role)> {
    let mut people: Vec<(String, String, Role)> = Vec::new();
    let lead = team
        .lead
        .iter()
        .map(|p| (p.hire.clone(), p.name.clone(), Role::Lead));
    let rest = team
        .members
        .iter()
        .map(|w| (w.hire.clone(), w.name.clone(), w.role));
    for who in lead.chain(rest) {
        if !people.iter().any(|(hire, _, _)| *hire == who.0) {
            people.push(who);
        }
    }
    people
}

/// A person's small face: their shirt, skin and hair, and who they are.
#[component]
async fn face(hire: String, name: String, role: Role) -> topcoat::Result<impl View> {
    let (skin, hair) = looks(&hire);
    let class = format!("face {} s-{skin} h-{hair}", role.class());
    let title = format!("{name}, {}", role_word(role));
    Ok(view! { <span class=(class) title=(title)></span> })
}

/// The small faces of who is on a project, or a dashed one when nobody is.
#[component]
async fn faces_of(team: Team) -> topcoat::Result<impl View> {
    let people = faces(&team);
    let more = people.len().saturating_sub(FACES);
    Ok(view! {
        <span class="avs">
            if people.is_empty() {
                <span class="face none" title="Nobody on it"></span>
            }
            for (hire, name, role) in people.into_iter().take(FACES) {
                face(hire: hire, name: name, role: role)
            }
            if more > 0 {
                <span class="face more" title=(format!("{more} more"))>"+" (more)</span>
            }
        </span>
    })
}

/// A project's sign: its name, phase, milestone and who is on it, a control
/// that picks the project, with a bell on it when it needs the owner.
#[component]
async fn sign_card(team: Team, selected: bool) -> topcoat::Result<impl View> {
    let phase = phase_class(&team);
    let (percent, words) = progress(&team);
    let line = sign_line(&team);
    let label = format!("{}: {line}", team.project);
    let initial = team
        .project
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let key = super::key("projects", &team.project);
    let pill = format!("pill ph-{phase}");
    let dot = format!("pdot ph-{phase}");
    let bar = format!("width:{percent}%");
    let version = team
        .milestone
        .as_ref()
        .and_then(|m| m.version.clone())
        .or_else(|| team.live.clone().filter(|_| team.phase == Phase::Live));
    let title = team
        .milestone
        .as_ref()
        .map(|m| m.title.clone())
        .unwrap_or_default();
    let word = phase_word(&team);
    let needs = team.needs;
    let bell = format!("{needs} waiting for you");
    Ok(view! {
        <div class=(if selected { "sign-card is-on" } else { "sign-card" })>
            <button type="button" class="sign-open" data-sel=(key) title=(line) aria-label=(label) aria-pressed=(if selected { "true" } else { "false" })>
                <span class="s-r1">
                    <i class=(facts::dot_class(team.tint))></i>
                    <b class="s-name">(team.project.clone())</b>
                    <i class=(dot)></i>
                </span>
                <span class="s-init" aria-hidden="true">(initial)</span>
                <span class="s-full">
                    <span class="s-r2">
                        <span class=(pill.clone())><i></i>(word)</span>
                        <span class="s-mile">
                            if let Some(v) = version { <b>"v" (v)</b> " " }
                            (title)
                        </span>
                    </span>
                    <span class="s-r3">
                        <span class="bar"><i style=(bar.clone())></i></span>
                        <span class="s-num">(words)</span>
                        faces_of(team: team.clone())
                    </span>
                </span>
                <span class="s-compact">
                    <span class=(pill)><i></i>(word)</span>
                    <span class="bar"><i style=(bar)></i></span>
                    faces_of(team: team.clone())
                </span>
            </button>
            if needs > 0 {
                <button type="button" class="bell" aria-label=(bell.clone()) title=(bell) data-on:click=(go("needs"))>
                    <svg class="bell-ic" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 9a6 6 0 0 1 12 0c0 6 2 7.5 2 7.5H4S6 15 6 9z"></path><path d="M10 20a2 2 0 0 0 4 0"></path></svg>
                    <b>(needs)</b>
                </button>
            }
        </div>
    })
}

/// One label over the floor, placed in percent of the floor's box.
#[component]
async fn pin_view(pin: Pin, projects: Vec<String>, sel: String) -> topcoat::Result<impl View> {
    // A label with places for each width of the page hangs by those; the
    // style sheet picks one.
    let mut place = if pin.spots.is_empty() {
        format!("left:{:.2}%;top:{:.2}%", pin.left, pin.top)
    } else {
        let mut vars = String::new();
        for (i, at) in pin.spots.iter().enumerate() {
            vars.push_str(&format!(
                "--p{i}:{:.2}% auto auto {:.2}%;--a{i}:{:.0}% {:.0}%;",
                at.top,
                at.left,
                -at.across * 100.0,
                -at.down * 100.0
            ));
        }
        vars.pop();
        vars
    };
    let hung = !pin.spots.is_empty();
    let mut hint: Option<&'static str> = None;
    let mut sign: Option<(usize, &'static str)> = None;
    let body: BoxView = match pin.what {
        PinWhat::Ticket(w) => {
            let n = facts::tint(&projects, &w.project);
            let (tone, state) = match &w.doing {
                Doing::Working => ("run", "at work".to_string()),
                Doing::Asking(what) => ("needs", format!("waiting for you: {what}")),
                Doing::Held(why) => ("held", format!("on hold: {why}")),
            };
            let label = format!("{}: {}, {state}", w.ticket, w.title);
            let ring = format!("--p:{:.2}", w.progress);
            let key = format!("tickets/{}", safe(&w.ticket));
            // The picked task's label stays open, with its title.
            let open = key == sel;
            let tone_class = if open {
                format!("tagc {tone} is-open")
            } else {
                format!("tagc {tone}")
            };
            let (asking, held) = (tone == "needs", tone == "held");
            view! {
                <button type="button" class=(tone_class) data-sel=(key) data-pj=(n) aria-label=(label) aria-pressed=(if open { "true" } else { "false" })>
                    if asking {
                        <span class="q">"!"</span>
                    } else if held {
                        <span class="zz">"II"</span>
                    } else {
                        <span class="ring" style=(ring)></span>
                    }
                    <i class=(facts::dot_class(n))></i>
                    <b>(w.ticket.clone())</b>
                    <span class="t">(w.title.clone())</span>
                </button>
            }
            .boxed()
        }
        // What a floor element is, shown on hover or with "What's what", and
        // kept up while it is picked.
        PinWhat::Hint(e) => {
            hint = Some(e.key());
            view! { <span>(e.hint())</span> }.boxed()
        }
        PinWhat::Sign(team) => {
            let tint = facts::wrap(team.tint);
            let selected = sel == super::key("projects", &team.project);
            sign = Some((tint, phase_class(&team)));
            place = format!("{place};--pj:var(--pj-{tint})");
            let team = *team;
            view! { sign_card(team: team, selected: selected) }.boxed()
        }
    };
    let class = match (hint, sign) {
        (Some(key), _) if sel.strip_prefix("elements/") == Some(key) => "pin hint on",
        (Some(_), _) => "pin hint",
        (None, Some(_)) => "pin sign",
        (None, None) => "pin",
    };
    let class = if hung {
        format!("{class} at")
    } else {
        class.to_string()
    };
    let (pj, phase) = sign.unzip();
    Ok(view! {
        <div class=(class) data-for=(hint) data-pj=(pj.map(|n| n.to_string())) data-phase=(phase) style=(place)>
            (body)
        </div>
    })
}

/// The side panel's frame; its content is the panel's own.
#[component]
pub async fn side(
    office: Office,
    detail: Detail,
    extra: Extra,
    sel: String,
    tab: String,
    history: usize,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <aside id="hq-panel" class="panel overflow-y-auto max-[900px]:overflow-visible max-[900px]:border-l-0! max-[900px]:border-t! max-[900px]:border-line" aria-label="What needs you, projects and the company">
            panel::panel(office: office, detail: detail, extra: extra, sel: sel, tab: tab, history: history)
        </aside>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::model::{Launch, Milestone, Person, Worker};

    fn team(phase: Phase) -> Team {
        Team {
            project: "shop".into(),
            tint: 1,
            phase,
            needs: 0,
            lead: Some(Person {
                hire: "nora".into(),
                name: "Nora".into(),
                role: Role::Lead,
                idle_minutes: None,
            }),
            members: Vec::new(),
            milestone: None,
            release: None,
            live: None,
        }
    }

    fn goal() -> Milestone {
        Milestone {
            key: "goals/P-1".into(),
            title: "Boss fight".into(),
            version: Some("0.4".into()),
            done: 6,
            total: 10,
        }
    }

    fn launch(waiting: bool, failed: bool) -> Launch {
        Launch {
            version: "1.2".into(),
            done: 4,
            total: 5,
            waiting,
            failed,
        }
    }

    fn at_desk(hire: &str, task: &str) -> Worker {
        Worker {
            ticket: task.into(),
            project: "shop".into(),
            title: task.into(),
            hire: hire.into(),
            name: hire.into(),
            role: Role::Engineer,
            doing: Doing::Working,
            progress: 0.25,
        }
    }

    #[test]
    fn a_sign_says_what_the_project_is_up_to() {
        let mut t = team(Phase::Building);
        assert_eq!(sign_line(&t), "Building");
        t.milestone = Some(goal());
        assert_eq!(sign_line(&t), "v0.4 Boss fight · 6 of 10");
        assert_eq!(progress(&t), (60, "6 of 10 tasks".to_string()));

        t.phase = Phase::Releasing;
        t.release = Some(launch(false, false));
        assert_eq!(sign_line(&t), "Releasing v1.2 · 4 of 5");
        assert_eq!(progress(&t), (80, "4 of 5 steps".to_string()));
        t.release = Some(launch(true, false));
        assert_eq!(sign_line(&t), "v1.2 waits for you");
        assert_eq!(progress(&t).1, "v1.2 waits for you");
        t.release = Some(launch(false, true));
        assert_eq!(sign_line(&t), "v1.2 failed");
        assert_eq!(phase_word(&t), "Release failed");

        let live = Team {
            live: Some("2.0".into()),
            ..team(Phase::Live)
        };
        assert_eq!(sign_line(&live), "Live · v2.0");
        assert_eq!(progress(&live), (100, "Live · v2.0".to_string()));
        assert_eq!(phase_word(&live), "Live");
        assert_eq!(progress(&team(Phase::Idle)).1, "Nothing open");
    }

    #[test]
    fn a_sign_shows_the_lead_and_each_person_once() {
        let mut t = team(Phase::Building);
        assert_eq!(
            faces(&t),
            [("nora".to_string(), "Nora".to_string(), Role::Lead)]
        );
        t.members = vec![
            at_desk("kit", "S-1"),
            at_desk("kit", "S-2"),
            at_desk("nora", "S-3"),
        ];
        assert_eq!(
            faces(&t),
            [
                ("nora".to_string(), "Nora".to_string(), Role::Lead),
                ("kit".to_string(), "kit".to_string(), Role::Engineer)
            ]
        );
    }

    fn project(name: &str, release: Option<&str>) -> studiod::api::ProjectView {
        let release = release.map(|state| {
            serde_json::json!({
                "project": name, "version": "1.2", "state": state, "steps": [], "started_at": 0,
            })
        });
        studiod::api::ProjectView {
            name: name.into(),
            release: release.map(|r| serde_json::from_value(r).unwrap()),
            ..studiod::api::ProjectView::default()
        }
    }

    #[test]
    fn the_hud_counts_projects_releases_and_what_waits() {
        let hq = Hq {
            teams: vec![
                team(Phase::Releasing),
                team(Phase::Building),
                team(Phase::Live),
            ],
            owner_needs: 2,
            ..Hq::default()
        };
        let mut office = Office {
            status: serde_json::from_value(serde_json::json!({"spent_24h": 6.4})).unwrap(),
            projects: vec![
                project("shop", Some("checking")),
                project("docs", None),
                project("api", Some("released")),
            ],
            ..Office::default()
        };
        let hud = |office: &Office| Hud::of(office, &hq);
        assert_eq!(
            hud(&office),
            Hud {
                projects: 3,
                releasing: 1,
                needs_you: 2,
                spent: 6.4,
                working: 0,
                slots: 0
            }
        );
        // Every release under way counts, whatever phase its project shows:
        // checking, waiting for the go-ahead or publishing, the office
        // closed or not; one that finished, failed or was cancelled doesn't.
        office.projects = vec![
            project("a", Some("checking")),
            project("b", Some("waiting")),
            project("c", Some("publishing")),
            project("d", Some("failed")),
            project("e", Some("cancelled")),
            project("f", Some("released")),
        ];
        assert_eq!(hud(&office).releasing, 3);
        office.status.closed = true;
        assert_eq!(hud(&office).releasing, 3);
    }
}
