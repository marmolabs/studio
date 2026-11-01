//! The home page's pieces: the top bar's projects and figures, the floor
//! with its pins, the two corners and the side panel's frame. Each piece that
//! changes has an `id` and is re-sent whole by the live stream.

use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{HeldReason, Ticket, TicketKind};
use topcoat::view::{BoxView, Child, Unescaped, View, ViewExt, component, view};

use super::inspector::Detail;
use super::model::{Doing, Floor, Pin, PinWhat, Scene, TINTS, Turn};
use super::panel::{self, Extra};
use super::{Office, ago, plan, safe};
use crate::ui;

/// The newest events the live corner shows.
const FEED_SHOWN: usize = 4;
/// Zoom steps and limits; 1 is the whole floor.
const ZOOM_MAX: &str = "2.6";
const ZOOM_STEP: &str = "0.4";

/// Sets the tab and shows it: a picked ticket, hire or goal is put away.
fn go(tab: &str) -> String {
    format!("$tab = '{tab}'; if ($sel) {{ $sel = ''; @get('/office/panel') }}")
}

/// Focuses one project (or `all`) and shows the Projects tab.
fn focus(tint: Option<usize>) -> String {
    match tint {
        Some(n) => format!("$focus = {n}; {}", go("projects")),
        None => "$focus = 'all'".to_string(),
    }
}

/// One click handler for everything on the floor with a `data-sel`.
pub const PICK: &str = "const t = evt.target.closest('[data-sel]'); if (t) { $sel = t.dataset.sel; @get('/office/panel'); window.innerWidth <= 900 && document.getElementById('hq-panel').scrollIntoView({behavior: 'smooth', block: 'start'}) }";
/// The same from the keyboard, for floor things that aren't buttons.
const PICK_KEY: &str = "const t = evt.target.closest('.iso [data-sel]'); if (t && (evt.key === 'Enter' || evt.key === ' ')) { evt.preventDefault(); $sel = t.dataset.sel; @get('/office/panel'); window.innerWidth <= 900 && document.getElementById('hq-panel').scrollIntoView({behavior: 'smooth', block: 'start'}) }";

/// The top bar's figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    /// Open asks plus tickets held for the owner, as on the board.
    pub needs_you: u64,
    /// Tickets at work: not paused on an ask.
    pub running: u64,
    /// Tickets that landed in the last 24 hours.
    pub shipped: u64,
    /// Runs holding a desk, and how many desks there are.
    pub desks_used: u64,
    pub desks: u64,
    pub spent: f64,
    pub daily: f64,
}

impl Hud {
    pub fn of(office: &Office) -> Hud {
        let held = office
            .tickets
            .iter()
            .filter(|t| t.state.kind() == "held" && ui::needs_owner(&t.state))
            .count();
        let shipped = super::floor::shipped(office)
            .iter()
            .filter(|(t, _)| t.state.kind() == "landed")
            .count();
        Hud {
            needs_you: office.status.asks + held as u64,
            running: office.status.at_work(),
            shipped: shipped as u64,
            desks_used: office.status.running,
            desks: office.status.max_runs,
            spent: office.status.spent_24h,
            daily: office.status.daily_usd,
        }
    }
}

/// One project: its tint is its place in the studio's list of projects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    pub tint: usize,
    /// Tickets in progress or waiting: not landed, failed or cancelled.
    pub open: u64,
    /// Open asks and tickets held for the owner.
    pub needs: u64,
    /// Tickets at work.
    pub working: u64,
    /// Tickets landed and filed over the project's open goals.
    pub landed: usize,
    pub total: usize,
}

impl ProjectRow {
    /// The share of the project's goals' tickets that landed, in percent.
    fn percent(&self) -> usize {
        (self.landed * 100).checked_div(self.total).unwrap_or(0)
    }
}

/// A line of the live corner.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedLine {
    /// How long ago, short: "now", "2m".
    pub ago: String,
    /// `run`, `ok`, `bad`, `needs` or `held`, as `.dot` takes it.
    pub tone: &'static str,
    /// What comes before the ticket's id, which is set in bold: "dev is
    /// checking".
    pub before: String,
    pub id: String,
    /// What follows it: " landed". Short enough to leave the id in view.
    pub after: String,
}

/// Everything the page draws apart from the side panel.
#[derive(Clone, Debug)]
pub struct Parts {
    pub hud: Hud,
    pub rows: Vec<ProjectRow>,
    pub scene: Scene,
    pub projects: Vec<String>,
    pub closed: bool,
    pub feed: Vec<FeedLine>,
    /// The corner the floor is drawn from.
    pub turn: Turn,
}

impl Parts {
    /// Everything but the panel, with the floor seen from `turn`.
    pub fn of(office: &Office, extra: &Extra, turn: Turn) -> Parts {
        let floor = plan::floor(office, extra);
        Parts {
            hud: Hud::of(office),
            rows: project_rows(office, &floor),
            scene: super::scene::render(&floor, turn),
            projects: floor.projects.clone(),
            closed: floor.closed,
            feed: feed(office),
            turn,
        }
    }
}

fn project_rows(office: &Office, floor: &Floor) -> Vec<ProjectRow> {
    floor
        .projects
        .iter()
        .enumerate()
        .map(|(tint, name)| {
            let own = |t: &&Ticket| t.project.to_string() == *name;
            let open = office
                .tickets
                .iter()
                .filter(own)
                .filter(|t| !matches!(t.state.kind(), "landed" | "failed" | "cancelled"))
                .count() as u64;
            let working = office
                .tickets
                .iter()
                .filter(own)
                .filter(|t| {
                    matches!(
                        t.state.kind(),
                        "claimed" | "working" | "checking" | "reviewing"
                    )
                })
                .count() as u64;
            let held = office
                .tickets
                .iter()
                .filter(own)
                .filter(|t| t.state.kind() == "held" && ui::needs_owner(&t.state))
                .count() as u64;
            let asks = office
                .asks
                .iter()
                .filter(|a| {
                    office
                        .ticket(&a.ticket)
                        .is_some_and(|t| t.project.to_string() == *name)
                })
                .count() as u64;
            let (landed, total) = floor
                .goals
                .iter()
                .filter(|g| g.project == *name)
                .fold((0, 0), |(l, t), g| (l + g.landed, t + g.total));
            ProjectRow {
                name: name.clone(),
                tint: tint % TINTS,
                open,
                needs: asks + held,
                working,
                landed,
                total,
            }
        })
        .collect()
}

/// The newest events in plain words, newest first.
fn feed(office: &Office) -> Vec<FeedLine> {
    office
        .recent
        .iter()
        .rev()
        .take(FEED_SHOWN)
        .map(|e| {
            let ticket = office.ticket(&e.ticket_id);
            let who = ticket.map_or_else(|| "A ticket".to_string(), |t| t.hire.to_string());
            let plan = ticket.is_some_and(|t| t.kind == TicketKind::Plan);
            let (tone, before, after) = match &e.event {
                TicketEvent::Claimed => ("run", format!("{who} started"), String::new()),
                TicketEvent::AskOpened { .. } => {
                    ("needs", String::new(), " needs your say".to_string())
                }
                TicketEvent::AskResolved { approved, .. } => (
                    if *approved { "ok" } else { "bad" },
                    "A call on".to_string(),
                    format!(" was {}", decided(*approved)),
                ),
                TicketEvent::ChecksStarted => ("run", format!("{who} is checking"), String::new()),
                TicketEvent::ChecksFailed { .. } => {
                    ("bad", String::new(), " failed its checks".to_string())
                }
                TicketEvent::ReviewFailed { .. } => (
                    "bad",
                    String::new(),
                    " was sent back from review".to_string(),
                ),
                TicketEvent::Held { reason } => {
                    ("held", String::new(), format!(" on hold: {}", why(reason)))
                }
                // A plan lands when its tickets are filed.
                TicketEvent::Landed if plan => ("ok", format!("{who} planned"), String::new()),
                TicketEvent::Landed => ("ok", String::new(), " landed".to_string()),
                TicketEvent::Failed { .. } => ("bad", String::new(), " failed".to_string()),
                TicketEvent::Cancelled => ("held", String::new(), " was cancelled".to_string()),
                _ => ("held", String::new(), " changed".to_string()),
            };
            FeedLine {
                ago: ago(office.now.saturating_sub(e.at_ms)),
                tone,
                before,
                id: e.ticket_id.clone(),
                after,
            }
        })
        .collect()
}

fn decided(approved: bool) -> &'static str {
    if approved { "allowed" } else { "denied" }
}

fn why(reason: &HeldReason) -> &'static str {
    match reason {
        HeldReason::Budget => "budget",
        HeldReason::Quota => "quota",
        HeldReason::Auth => "login",
        HeldReason::Conflict => "conflict",
        HeldReason::Paused => "paused",
    }
}

/// The projects strip: every project, and the way back to all of them.
#[component]
pub async fn projects_strip(rows: Vec<ProjectRow>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="hq-projects" class="contents">
            <div class="flex min-w-0 gap-1 overflow-x-auto rounded-[10px] border border-line bg-bg-3 p-[3px] [scrollbar-width:none] max-[900px]:order-5 max-[900px]:w-full" role="group" aria-label="Projects">
                <button type="button" class="pj-btn on" data-class:on="$focus === 'all'" data-on:click=(focus(None))>"All projects"</button>
                for row in rows {
                    <button type="button" class="pj-btn" data-class:on=(format!("$focus === {}", row.tint)) data-on:click=(focus(Some(row.tint))) title=(format!("{}: {} in progress or waiting", row.name, row.open))>
                        <i class=(format!("pjd pj-{}", row.tint))></i>
                        (row.name.clone())
                        <span class="n">(row.open)</span>
                    </button>
                }
            </div>
        </div>
    })
}

/// What needs you, what is at work, what shipped, and the money meter.
#[component]
pub async fn figures(hud: Hud) -> topcoat::Result<impl View> {
    let needs = hud.needs_you > 0;
    let desks = if hud.desks > 0 {
        format!("{} of {} desks in use", hud.desks_used, hud.desks)
    } else {
        "At work now".to_string()
    };
    Ok(view! {
        <div id="hq-figures" class="contents">
            <div class="stats">
                <button type="button" class=(if needs { "stat needs" } else { "stat" }) data-on:click=(go("needs"))>
                    if needs {
                        <span class="ping"></span>
                    } else {
                        <span class="dot ok"></span>
                    }
                    <b>(hud.needs_you)</b>
                    <span class="lbl max-[520px]:hidden">"need you"</span>
                </button>
                <button type="button" class="stat max-[900px]:hidden!" title=(desks) data-on:click=(super::select("elements/add-desk"))>
                    <span class="dot run"></span>
                    <b>(hud.running)</b>
                    "at work"
                </button>
                <button type="button" class="stat max-[1320px]:hidden!" title="Landed in the last 24 hours" data-on:click=(super::select("elements/crates"))>
                    ui::icon(name: "circle-check", small: true)
                    <b>(hud.shipped)</b>
                    "shipped"
                </button>
            </div>
            <a href="/budget" class="ml-auto flex min-w-[140px] flex-col gap-[5px] max-[900px]:min-w-0 max-[900px]:flex-1" title="Spent in the last 24 hours, of the daily limit">
                <span class="flex items-baseline justify-between gap-2.5 whitespace-nowrap text-[12px] text-fg-3">
                    <span class="max-[1100px]:hidden">"Last 24 hours"</span>
                    <span>
                        <b class="text-[15px] font-semibold tabular-nums text-fg">(ui::usd(hud.spent))</b>
                        <span class="max-[520px]:hidden">" of " (ui::limit(hud.daily))</span>
                    </span>
                </span>
                ui::meter(value: hud.spent, max: hud.daily)
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

/// The floor: the drawing, the labels pinned over it, and the closed sign,
/// with `sel` marked. Zoom, focus and hints come from the page's signals and
/// are put back by Datastar whenever a patch changes the element.
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
    let svg = Unescaped::new_unchecked(picked(scene.svg, &sel));
    Ok(view! {
        <div id="hq-scene" class=(class) style=(style) data-turn=(turn.index().to_string()) data-style="{'--zoom': $zoom}" data-attr:data-focus="$focus" data-class:show-hints="$hints" data-on:click=(PICK) data-on:keydown=(PICK_KEY)>
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

fn tint(projects: &[String], project: &str) -> usize {
    projects.iter().position(|p| p == project).unwrap_or(0) % TINTS
}

/// One label over the floor, placed in percent of the floor's box.
#[component]
async fn pin_view(pin: Pin, projects: Vec<String>, sel: String) -> topcoat::Result<impl View> {
    let place = format!("left:{:.2}%;top:{:.2}%", pin.left, pin.top);
    let mut hint: Option<&'static str> = None;
    let body: BoxView = match pin.what {
        PinWhat::Ticket(w) => {
            let n = tint(&projects, &w.project);
            let (tone, state) = match &w.doing {
                Doing::Working => ("run", "at work".to_string()),
                Doing::Asking(what) => ("needs", format!("waiting for you: {what}")),
                Doing::Held(why) => ("held", format!("on hold: {why}")),
            };
            let label = format!("{}: {}, {state}", w.ticket, w.title);
            let ring = format!("--p:{:.2}", w.progress);
            let key = format!("tickets/{}", safe(&w.ticket));
            // The picked ticket's label stays open, with its title.
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
                    <i class=(format!("pjd pj-{n}"))></i>
                    <b>(w.ticket.clone())</b>
                    <span class="t">(w.title.clone())</span>
                </button>
            }
            .boxed()
        }
        PinWhat::Ready(c) => {
            let n = tint(&projects, &c.project);
            let land = format!("@post('/tickets/{}/land')", safe(&c.ticket));
            let label = format!("Land {}", c.ticket);
            view! {
                <div class="ready" data-pj=(n)>
                    <i class=(format!("pjd pj-{n}"))></i>
                    <span><b>(c.ticket.clone())</b>" ready"</span>
                    <button type="button" class=(ui::PRIMARY_SM) aria-label=(label) data-on:click=(land)>"Land"</button>
                </div>
            }
            .boxed()
        }
        PinWhat::Queue(n) => view! {
            <button type="button" class="tagc quiet" data-sel="elements/queue"><b>(n)</b>" up next"</button>
        }
        .boxed(),
        PinWhat::Planning(n) => view! {
            <button type="button" class="tagc quiet" data-sel="elements/board">"Planning "<b>(n)</b></button>
        }
        .boxed(),
        // What a game element is, shown on hover or with "What's what", and
        // kept up while it is picked.
        PinWhat::Hint(e) => {
            hint = Some(e.key());
            view! { <span>(e.hint())</span> }.boxed()
        }
    };
    let class = match hint {
        Some(key) if sel.strip_prefix("elements/") == Some(key) => "pin hint on",
        Some(_) => "pin hint",
        None => "pin",
    };
    Ok(view! {
        <div class=(class) data-for=(hint) style=(place)>
            (body)
        </div>
    })
}

/// The two corners over the floor, or under it on a phone.
#[component]
pub async fn corners(rows: Vec<ProjectRow>, feed: Vec<FeedLine>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="hq-corners" class="max-[900px]:grid max-[900px]:gap-2.5 max-[900px]:px-4 max-[900px]:pb-3.5 max-[900px]:pt-2.5">
            <div class="corner left">
                <h4><span>"Projects"</span><span>"tickets landed"</span></h4>
                if rows.is_empty() {
                    <p class="text-[12px] text-fg-3">"No projects yet."</p>
                }
                for row in rows {
                    <button type="button" class="trk" data-on:click=(focus(Some(row.tint)))>
                        <i class=(format!("pjd pj-{}", row.tint))></i>
                        <span class="name">(row.name.clone())</span>
                        <span class="bar"><i style=(format!("width: {}%; background: var(--pj-{})", row.percent(), row.tint))></i></span>
                        <span class="num">
                            if row.total > 0 { (row.landed) "/" (row.total) }
                        </span>
                        <span class="flags">
                            if row.needs > 0 { <span class="flag needs" title="Need you">(row.needs)</span> }
                            if row.working > 0 { <span class="flag run" title="At work">(row.working)</span> }
                        </span>
                    </button>
                }
            </div>
            <div class="corner right">
                <h4>
                    <span class="live">"Live"</span>
                    <button type="button" class=(ui::GHOST_SM) data-on:click=(go("log"))>"All"</button>
                </h4>
                if feed.is_empty() {
                    <p class="text-[12px] text-fg-3">"Nothing has happened in the last 24 hours."</p>
                }
                <ol class="mini-feed">
                    for line in feed {
                        <li>
                            <time>(line.ago.clone())</time>
                            <span class=(format!("dot {}", line.tone))></span>
                            <span>(line.before.clone()) if !line.before.is_empty() { " " } <b>(line.id.clone())</b>(line.after.clone())</span>
                        </li>
                    }
                </ol>
            </div>
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
) -> topcoat::Result<impl View> {
    Ok(view! {
        <aside id="hq-panel" class="panel overflow-y-auto max-[900px]:overflow-visible max-[900px]:border-l-0! max-[900px]:border-t! max-[900px]:border-line" aria-label="What needs you, projects and team">
            panel::panel(office: office, detail: detail, extra: extra, sel: sel, tab: tab)
        </aside>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client;
    use serde_json::json;

    fn ticket(id: &str, project: &str, state: serde_json::Value) -> Ticket {
        serde_json::from_value(json!({
            "id": id, "project": project, "title": id, "goal": "", "hire": "dev",
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    #[test]
    fn the_hud_counts_what_needs_you_and_what_runs_as_the_board_does() {
        // Four runs hold a slot, but one of them waits on an ask.
        let status: client::Status = serde_json::from_value(json!({
            "asks": 2, "running": 4,
            "tickets": {"held": 2, "ready": 4, "working": 2, "reviewing": 1, "asking": 1},
            "daily_usd": 100.0, "spent_24h": 34.12,
        }))
        .unwrap();
        // A budget hold starts again by itself; a pause waits for the owner.
        let tickets = vec![
            ticket("G-1", "game", json!({"held": {"reason": "budget"}})),
            ticket("G-2", "game", json!({"held": {"reason": "paused"}})),
        ];
        let office = Office {
            status,
            tickets,
            ..Office::default()
        };
        assert_eq!(
            Hud::of(&office),
            Hud {
                needs_you: 3,
                running: 3,
                shipped: 0,
                desks_used: 4,
                desks: 0,
                spent: 34.12,
                daily: 100.0
            }
        );
    }

    #[test]
    fn each_project_counts_its_open_work_and_what_needs_you() {
        let status: client::Status = serde_json::from_value(json!({
            "projects": ["shop", "api"], "asks": 1,
        }))
        .unwrap();
        let tickets = vec![
            ticket("S-1", "shop", json!("working")),
            ticket("S-2", "shop", json!("queued")),
            ticket("S-3", "shop", json!("landed")),
            ticket("S-4", "shop", json!({"held": {"reason": "paused"}})),
            ticket("A-1", "api", json!("checking")),
        ];
        let asks = serde_json::from_value(json!([{
            "id": "ASK-1", "ticket": "A-1", "hire": "dev", "kind": "permission",
            "tool": "Bash", "input": {}, "reasons": [],
        }]))
        .unwrap();
        let office = Office {
            status,
            tickets,
            asks,
            ..Office::default()
        };
        let floor = plan::floor(&office, &Extra::default());
        let rows = project_rows(&office, &floor);
        let seen: Vec<_> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.tint, r.open, r.needs, r.working))
            .collect();
        assert_eq!(seen, [("shop", 0, 3, 1, 1), ("api", 1, 1, 1, 1)]);
    }

    #[test]
    fn the_live_corner_tells_the_latest_events_in_words() {
        let at = |id: &str, ms: u64, event: serde_json::Value| -> client::Happened {
            serde_json::from_value(json!({"ticket_id": id, "at_ms": ms, "event": event})).unwrap()
        };
        let office = Office {
            tickets: vec![ticket("G-1", "game", json!("working"))],
            recent: vec![
                at("G-1", 1_000, json!({"type": "claimed"})),
                at("G-1", 2_000, json!({"type": "checks_started"})),
                at("G-1", 3_000, json!({"type": "ask_opened", "ask": "A-1"})),
                at("G-1", 4_000, json!({"type": "landed"})),
                at("G-1", 5_000, json!({"type": "held", "reason": "budget"})),
            ],
            now: 125_000,
            ..Office::default()
        };
        let lines = feed(&office);
        // Newest first, four of them.
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].after, " on hold: budget");
        assert_eq!(lines[0].tone, "held");
        assert_eq!(lines[0].ago, "2m");
        assert_eq!(lines[1].after, " landed");
        assert_eq!(lines[2].after, " needs your say");
        assert_eq!(lines[2].tone, "needs");
        assert_eq!(lines[3].before, "dev is checking");
        assert!(lines.iter().all(|l| l.id == "G-1"));
    }
}
