//! The office: the whole studio as one isometric floor with labels pinned
//! over it, a top bar and a side panel, on one page. `plan` decides who is
//! where, `scene` draws the floor, `hq` draws the page's pieces and `panel`
//! the side panel for whatever the owner picks.

mod floor;
mod hq;
mod inspector;
mod model;
mod panel;
mod plan;
mod scene;

use futures_util::Stream;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::Ticket;
use studiod::api::{AskView, FileKind, HireDay, MAX_RUNS_LIMIT, RECENT_EVENTS, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, Signals},
    router::{
        content::{
            Form,
            sse::{Event, Sse},
        },
        error::redirect_permanent,
        page, query_params,
        response::Response,
        route,
    },
    view::{BoxView, View, ViewExt, component, view},
};

use super::ticket::{log_path, log_tail, merged_ids};
use super::timeline::relative;
use crate::summary::{Judge, review_line};
use crate::{AppConfig, client, diffstat, layout, live, ui};

/// How far back "recent" reaches: the same 24 hours as the spend.
const WINDOW_MS: u64 = 24 * 3_600_000;
/// The events the office reads: work starting, asking, going to checks,
/// being sent back, held or ended.
const KINDS: [&str; 10] = [
    "claimed",
    "ask_opened",
    "ask_resolved",
    "checks_started",
    "checks_failed",
    "review_failed",
    "held",
    "landed",
    "failed",
    "cancelled",
];

/// Everything the floor and the inspector show, read from studiod at once.
#[derive(Clone, Debug, Default)]
pub struct Office {
    pub status: client::Status,
    pub tickets: Vec<Ticket>,
    pub asks: Vec<AskView>,
    /// Every ticket's `KINDS` events over the last 24 hours, oldest first.
    pub recent: Vec<client::Happened>,
    /// When `recent` starts, if studiod cut it short of the 24 hours.
    pub cut_at: Option<u64>,
    /// What each ticket has cost, by id.
    pub spend: BTreeMap<String, f64>,
    /// Lines added and removed by each ready ticket, by id.
    pub sizes: HashMap<String, (u64, u64)>,
    /// Who is judging each ticket in review, from its whole run log.
    pub judges: HashMap<String, Judge>,
    /// Landed tickets whose goal branch is merged.
    pub merged: Vec<String>,
    /// What each hire did in the last 24 hours, by id.
    pub today: BTreeMap<String, HireDay>,
    pub now: u64,
}

impl Office {
    pub fn ticket(&self, id: &str) -> Option<&Ticket> {
        self.tickets.iter().find(|t| t.id.to_string() == id)
    }

    pub fn spent(&self, id: &str) -> f64 {
        self.spend.get(id).copied().unwrap_or(0.0)
    }

    /// When the latest of `ticket`'s events that `pick` accepts happened.
    pub fn last(&self, ticket: &str, pick: impl Fn(&TicketEvent) -> bool) -> Option<u64> {
        self.recent
            .iter()
            .rev()
            .find(|e| e.ticket_id == ticket && pick(&e.event))
            .map(|e| e.at_ms)
    }

    /// "the last 24 hours", or as far back as `recent` reaches when cut.
    pub fn window(&self) -> String {
        let Some(at) = self.cut_at else {
            return "the last 24 hours".into();
        };
        match self.now.saturating_sub(at) / 60_000 {
            0..=1 => "the last minute".into(),
            minutes @ 2..60 => format!("the last {minutes} minutes"),
            60..120 => "the last hour".into(),
            minutes => format!("the last {} hours", minutes / 60),
        }
    }

    /// A hire's role and model, when it is loaded.
    pub fn member(&self, hire: &str) -> Option<&client::Member> {
        self.status.team.iter().find(|m| m.id == hire)
    }
}

/// Reads what the office shows. Only the status and the tickets must come
/// back; anything else studiod can't say right now is left empty.
async fn gather(config: &AppConfig) -> topcoat::Result<Office> {
    let socket = &config.socket;
    let status: client::Status = client::call(socket, Request::Status).await?;
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await?;
    let now = ui::now_ms();
    let since_ms = now.saturating_sub(WINDOW_MS);
    let mut judges = HashMap::new();
    for ticket in &tickets {
        let id = ticket.id.to_string();
        if floor::room(ticket) == floor::Room::Review
            && let Some(judge) = judge_now(&config.data_dir, &id).await
        {
            judges.insert(id.clone(), judge);
        }
    }
    let ready: Vec<&Ticket> = tickets
        .iter()
        .filter(|t| t.state.kind() == "ready")
        .collect();
    let recent: Vec<client::Happened> = client::call(
        socket,
        Request::RecentEvents {
            since_ms,
            limit: None,
            kinds: KINDS.map(String::from).to_vec(),
        },
    )
    .await
    .unwrap_or_default();
    Ok(Office {
        asks: client::call(socket, Request::Asks)
            .await
            .unwrap_or_default(),
        cut_at: (recent.len() >= RECENT_EVENTS)
            .then(|| recent.first().map(|e| e.at_ms))
            .flatten(),
        recent,
        sizes: diffstat::sizes(socket, &ready).await,
        judges,
        spend: client::call(socket, Request::TicketSpends)
            .await
            .unwrap_or_default(),
        merged: merged_ids(socket).await,
        today: hires_today(socket, now).await,
        status,
        tickets,
        now,
    })
}

/// What each hire did in the last 24 hours. studiod reads every fresh run
/// log for it, so it is asked at most every `TODAY_EVERY` ms, not on every
/// tick of every open office.
async fn hires_today(socket: &Path, now: u64) -> BTreeMap<String, HireDay> {
    type Days = BTreeMap<String, HireDay>;
    const TODAY_EVERY: u64 = 30_000;
    static SEEN: Mutex<BTreeMap<PathBuf, (u64, Days)>> = Mutex::new(BTreeMap::new());
    let known = SEEN.lock().ok().and_then(|seen| seen.get(socket).cloned());
    if let Some((at, today)) = &known
        && now.saturating_sub(*at) < TODAY_EVERY
    {
        return today.clone();
    }
    let Ok(today) = client::call::<Days>(socket, Request::HiresToday).await else {
        return known.map(|(_, today)| today).unwrap_or_default();
    };
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(socket.to_path_buf(), (now, today.clone()));
    }
    today
}

/// Who is judging a ticket in review now, from its run log. The log is read
/// back once; after that only what it gained since is read.
pub async fn judge_now(data_dir: &Path, id: &str) -> Option<Judge> {
    static SEEN: Mutex<BTreeMap<PathBuf, (u64, Option<Judge>)>> = Mutex::new(BTreeMap::new());
    let path = log_path(data_dir, id);
    let len = tokio::fs::metadata(&path).await.ok()?.len();
    // A log shorter than before was started again: read it all.
    let known = SEEN
        .lock()
        .ok()
        .and_then(|seen| seen.get(&path).cloned())
        .filter(|(read, _)| *read <= len);
    let from = known.as_ref().map_or(0, |(read, _)| *read);
    let judge = match find_back(data_dir, id, from..len, review_line).await {
        Some(found) => found,
        None => known.and_then(|(_, judge)| judge),
    };
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(path, (len, judge.clone()));
    }
    judge
}

/// The latest line in `range` of a ticket's run log that `pick` takes, with
/// `range` starting at a line's start. Reads back from the end a chunk at a
/// time, so a line far back in a long log is still found without reading
/// the whole log for a recent one.
pub async fn find_back<T>(
    data_dir: &Path,
    id: &str,
    range: std::ops::Range<u64>,
    pick: impl Fn(&str) -> Option<T>,
) -> Option<T> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    const CHUNK: u64 = 64 * 1024;
    /// Gives up this far back from the end.
    const FURTHEST: u64 = 16 * 1024 * 1024;
    let mut file = tokio::fs::File::open(log_path(data_dir, id)).await.ok()?;
    let len = file.metadata().await.ok()?.len().min(range.end);
    let stop = len.saturating_sub(FURTHEST).max(range.start);
    let mut end = len;
    // The start of the line the later chunk began in the middle of.
    let mut rest: Vec<u8> = Vec::new();
    while end > stop {
        let start = end.saturating_sub(CHUNK).max(stop);
        file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
        let mut bytes = vec![0; usize::try_from(end - start).ok()?];
        file.read_exact(&mut bytes).await.ok()?;
        bytes.append(&mut rest);
        // A chunk read from the middle starts mid-line: keep that part for
        // the next chunk back.
        let cut = if start == range.start {
            0
        } else {
            bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1)
        };
        let whole = bytes.split_off(cut);
        rest = bytes;
        if let Some(found) = String::from_utf8_lossy(&whole)
            .lines()
            .rev()
            .find_map(&pick)
        {
            return Some(found);
        }
        end = start;
    }
    None
}

/// What the owner picked on the floor: `tickets/<id>`, `hires/<id>`,
/// `goals/<plan id>`, or empty for the overview.
#[derive(Clone, Debug, Default, Deserialize)]
struct Pick {
    #[serde(default)]
    sel: String,
    /// The side panel's tab.
    #[serde(default)]
    tab: String,
    /// The corner the floor is seen from, in quarter turns.
    #[serde(default)]
    turn: i64,
}

impl Pick {
    fn turn(&self) -> model::Turn {
        model::Turn::new(self.turn)
    }
}

/// `id` kept to characters that are safe inside a quoted Datastar
/// expression and a URL.
pub fn safe(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// A selection key, such as `tickets/SHOP-3`.
pub fn key(kind: &str, id: &str) -> String {
    format!("{kind}/{}", safe(id))
}

/// `key` checked: anything else selects nothing.
fn clean(sel: &str) -> String {
    match sel.split_once('/') {
        Some((kind @ ("tickets" | "hires" | "goals"), id)) => key(kind, id),
        Some(("elements", id)) if model::Element::from_key(id).is_some() => key("elements", id),
        _ => String::new(),
    }
}

/// What a floor element does when clicked: select it and show its panel,
/// scrolling to the panel where it sits under the floor.
pub fn select(key: &str) -> String {
    format!("$sel = '{key}'; @get('/office/panel'); {SHOW_PANEL}")
}

const SHOW_PANEL: &str =
    "window.innerWidth <= 900 && document.getElementById('hq-panel').scrollIntoView()";

#[query_params(error = bad_request)]
struct Opened {
    /// Opens the page with this selected, e.g. `tickets/SHOP-3`.
    sel: Option<String>,
    /// Opens the side panel on this tab, e.g. `money`.
    tab: Option<String>,
    /// Opens the floor seen from this corner, in quarter turns.
    turn: Option<i64>,
}

#[page("/")]
async fn office_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let opened = query_params::<Opened>(cx)?;
    let sel = clean(&opened.sel.clone().unwrap_or_default());
    let tab = panel::tab(opened.tab.as_deref().unwrap_or_default());
    let turn = model::Turn::new(opened.turn.unwrap_or_default());
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, &sel, &mut inspector::Memo::default()).await;
    let extra = panel::Extra::gather(config).await;
    let hq::Parts {
        hud,
        rows,
        scene,
        projects: tints,
        closed,
        feed,
        ..
    } = hq::Parts::of(&office, &extra, turn);
    let scene_sel = sel.clone();
    let projects = office.status.projects.clone();
    let team = office.status.team.clone();
    let switches = layout::theme_switches(cx);
    let signals = format!(
        "{{sel: '{sel}', tab: '{tab}', focus: 'all', zoom: 1, hints: false, turn: {}, pick: '', _dragging: false}}",
        turn.index()
    );
    Ok(view! {
        <div id="office" class="grid h-dvh grid-rows-[auto_minmax(0,1fr)] max-[900px]:h-auto" data-signals=(signals)>
            // Restarts the live stream whenever the selection or the corner the
            // floor is seen from changes, so it keeps the picked panel up to
            // date and draws the floor from that corner.
            <div hidden="" data-effect="$sel; $turn; @get('/office/stream')"></div>
            <header class="flex min-w-0 items-center gap-3 border-b border-line bg-bg-2 px-4 py-[9px] max-[900px]:flex-wrap max-[900px]:gap-y-2">
                <a href="/" class="flex items-center gap-2.5" aria-label="studio, the office">
                    <span class="grid size-[26px] shrink-0 place-items-center rounded-[7px] bg-primary text-on-primary">
                        <svg class="size-4" aria-hidden="true"><use href="#logo"></use></svg>
                    </span>
                    <b class="text-[15px] font-semibold tracking-[-0.01em] max-[520px]:hidden">"studio"</b>
                </a>
                hq::projects_strip(rows: rows.clone())
                hq::figures(hud: hud)
                <div class="flex items-center gap-2">
                    <button type="button" class=(ui::BUTTON) popovertarget="office-new-ticket" aria-label="New ticket" data-on:click=(NO_PICK)>
                        ui::icon(name: "plus")
                        <span class=(ui::LABEL)>"Ticket"</span>
                    </button>
                    <button type="button" class=(ui::PRIMARY) popovertarget="office-new-goal" aria-label="New goal" data-on:click=(NO_PICK)>
                        ui::icon(name: "target")
                        <span class=(ui::LABEL)>"New goal"</span>
                    </button>
                    layout::theme_form(switches: switches)
                </div>
            </header>
            <main class="grid min-h-0 grid-cols-[minmax(0,1fr)_384px] max-[1100px]:grid-cols-[minmax(0,1fr)_340px] max-[900px]:grid-cols-[minmax(0,1fr)]">
                <section class="world" aria-label="The studio floor, live">
                    hq::view_controls()
                    hq::wrap(
                        hq::floor_scene(scene: scene, projects: tints, closed: closed, sel: scene_sel, turn: turn)
                    )
                    hq::corners(rows: rows, feed: feed)
                </section>
                hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab.to_string())
            </main>
            new_ticket(projects: projects.clone(), team: team.clone())
            new_goal(projects: projects, team: team)
        </div>
    })
}

/// Where the office was before it became the home page.
#[route(GET "/office")]
async fn old_office(cx: &Cx) -> topcoat::Result<Response> {
    let sel = clean(&query_params::<Opened>(cx)?.sel.clone().unwrap_or_default());
    let to = match sel.as_str() {
        "" => "/".to_string(),
        sel => format!("/?sel={sel}"),
    };
    Err(redirect_permanent(&to).into())
}

/// The Live page is gone; its address leads home.
#[route(GET "/live")]
async fn old_live() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The top bar's figures, the floor, the corners and the panel, re-sent as
/// they change, for the selection the stream was opened with.
#[route(GET "/office/stream")]
async fn office_stream(
    cx: &Cx,
    signals: Option<Signals<Pick>>,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    let pick = signals.map(|s| s.0).unwrap_or_default();
    let turn = pick.turn();
    let (sel, tab) = (clean(&pick.sel), panel::tab(&pick.tab));
    let kept = Arc::new(tokio::sync::Mutex::new(Kept::default()));
    Ok(live::watch(move || {
        let config = config.clone();
        let cx = cx.clone();
        let sel = sel.clone();
        let kept = kept.clone();
        async move { render_all(&cx, &config, &sel, tab, turn, &mut *kept.lock().await).await }
    }))
}

/// How long a stream shows the panel's extra reads before reading them
/// again: trophies, checks, rules and the last 14 days change slowly, and
/// some of them read the whole history.
const EXTRA_FRESH: Duration = Duration::from_secs(15);

/// What a stream keeps between renders.
#[derive(Default)]
struct Kept {
    /// What the picked panel reads once.
    memo: inspector::Memo,
    /// The panel's extra reads, and when they were made.
    extra: Option<(Instant, panel::Extra)>,
}

async fn render_all(
    cx: &Cx,
    config: &AppConfig,
    sel: &str,
    tab: &str,
    turn: model::Turn,
    kept: &mut Kept,
) -> topcoat::Result<String> {
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, sel, &mut kept.memo).await;
    let extra = match &kept.extra {
        Some((at, extra)) if at.elapsed() < EXTRA_FRESH => extra.clone(),
        _ => {
            let extra = panel::Extra::gather(config).await;
            kept.extra = Some((Instant::now(), extra.clone()));
            extra
        }
    };
    let parts = hq::Parts::of(&office, &extra, turn);
    let (sel, tab) = (sel.to_string(), tab.to_string());
    ui::html(cx, live_parts(cx, parts, office, detail, extra, sel, tab)).await
}

/// Every piece of the page that changes, boxed so their futures live on the
/// heap.
fn live_parts(
    cx: &Cx,
    parts: hq::Parts,
    office: Office,
    detail: inspector::Detail,
    extra: panel::Extra,
    sel: String,
    tab: String,
) -> BoxView<'_> {
    view! { cx =>
        hq::projects_strip(rows: parts.rows.clone())
        hq::figures(hud: parts.hud)
        hq::floor_scene(scene: parts.scene, projects: parts.projects, closed: parts.closed, sel: sel.clone(), turn: parts.turn)
        hq::corners(rows: parts.rows, feed: parts.feed)
        hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab)
    }
    .boxed()
}

/// The panel for the current selection, right away.
#[route(GET "/office/panel")]
async fn office_panel(cx: &Cx, signals: Option<Signals<Pick>>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let pick = signals.map(|s| s.0).unwrap_or_default();
    let (sel, tab) = (clean(&pick.sel), panel::tab(&pick.tab).to_string());
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, &sel, &mut inspector::Memo::default()).await;
    let extra = panel::Extra::gather(config).await;
    let side =
        view! { cx => hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab) }
            .boxed();
    let html = ui::html(cx, view! { cx => (side) }).await?;
    Ok(PatchElements::new(html))
}

/// The floor alone, drawn from the corner the signals ask for, so a turn
/// shows at once instead of with the next live update.
#[route(GET "/office/scene")]
async fn office_scene(cx: &Cx, signals: Option<Signals<Pick>>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let pick = signals.map(|s| s.0).unwrap_or_default();
    let sel = clean(&pick.sel);
    let office = gather(config).await?;
    let extra = panel::Extra::gather(config).await;
    let hq::Parts {
        scene,
        projects,
        closed,
        turn,
        ..
    } = hq::Parts::of(&office, &extra, pick.turn());
    let floor =
        view! { cx => hq::floor_scene(scene: scene, projects: projects, closed: closed, sel: sel, turn: turn) }
            .boxed();
    let html = ui::html(cx, view! { cx => (floor) }).await?;
    Ok(PatchElements::new(html))
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

#[derive(Deserialize)]
struct NewProject {
    #[serde(default)]
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    branch: String,
}

/// A field left empty is sent as absent.
fn absent(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Adds a git repository on this computer as a project; studiod fills in
/// what is left out.
#[route(POST "/office/projects")]
async fn add_project(cx: &Cx, Form(form): Form<NewProject>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let Some(path) = absent(&form.path) else {
        let words = "Enter the folder of a git repository.";
        return ui::notice(cx, words.into(), true).await;
    };
    let name = absent(&form.name);
    let request = Request::AddProject {
        path,
        name: name.clone(),
        branch: absent(&form.branch),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(added) => {
            let name = added["name"]
                .as_str()
                .map(str::to_string)
                .or(name)
                .unwrap_or_else(|| "the project".into());
            let words = match added["checks"].as_str() {
                Some(checks) => format!("Added {name}. Its checks: {checks}"),
                None => format!("Added {name}."),
            };
            ui::notice(cx, words, false).await
        }
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The roles the Hire form offers, and the models.
pub const ROLES: [&str; 8] = [
    "engineer", "designer", "writer", "producer", "reviewer", "critic", "approver", "router",
];
pub const MODELS: [&str; 3] = ["sonnet", "opus", "haiku"];
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

/// Opening a form from the top bar picks no hire, whatever a panel picked.
const NO_PICK: &str = "$pick = ''";

/// The New ticket form, with the hire picked by `$pick` when a panel's
/// "Give it a ticket" opened it. The approver and router take no tickets.
#[component]
async fn new_ticket(
    projects: Vec<String>,
    team: Vec<client::Member>,
) -> topcoat::Result<impl View> {
    let hires: Vec<String> = team
        .into_iter()
        .filter(|m| !floor::FRONT.contains(&m.role.as_str()))
        .map(|m| m.id)
        .collect();
    Ok(view! {
        ui::dialog(id: "office-new-ticket", title: "New ticket",
            <form data-on:submit="@post('/tickets', {contentType: 'form'}); el.closest('[popover]').hidePopover()">
                <div class=(ui::DIALOG_BODY)>
                    <div class="grid grid-cols-2 gap-3 max-[760px]:grid-cols-1">
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Project"</span>
                            <select name="project" required="" class=(ui::INPUT)>
                                for p in projects { <option value=(p.clone())>(p)</option> }
                            </select>
                        </label>
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Hire"</span>
                            <select name="hire" class=(ui::INPUT) data-bind:pick="">
                                <option value="">"Pick for me"</option>
                                for h in hires { <option value=(h.clone())>(h)</option> }
                            </select>
                        </label>
                    </div>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Title"</span>
                        <input name="title" required="" placeholder="What should be done" class=(ui::INPUT)>
                    </label>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Details"</span>
                        <textarea name="detail" class=(ui::TEXTAREA) placeholder="Context, steps, what done looks like."></textarea>
                    </label>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Goal"</span>
                        <input name="goal" class=(ui::INPUT) placeholder="Optional. Tickets with the same goal land on one branch.">
                    </label>
                </div>
                <div class=(ui::DIALOG_FOOT)>
                    <span class="spacer"></span>
                    <button type="button" class=(ui::GHOST) popovertarget="office-new-ticket" popovertargetaction="hide">"Cancel"</button>
                    <button type="submit" class=(ui::PRIMARY)>"Create ticket"</button>
                </div>
            </form>
        )
    })
}

/// The New goal form; a producer turns the goal into tickets, so only
/// producers are offered.
#[component]
async fn new_goal(projects: Vec<String>, team: Vec<client::Member>) -> topcoat::Result<impl View> {
    let producers: Vec<String> = team
        .into_iter()
        .filter(|m| m.role == "producer")
        .map(|m| m.id)
        .collect();
    Ok(view! {
        ui::dialog(id: "office-new-goal", title: "New goal",
            <form data-on:submit="@post('/goals', {contentType: 'form'}); el.closest('[popover]').hidePopover()">
                <div class=(ui::DIALOG_BODY)>
                    <div class="grid grid-cols-2 gap-3 max-[760px]:grid-cols-1">
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Project"</span>
                            <select name="project" required="" class=(ui::INPUT)>
                                for p in projects { <option value=(p.clone())>(p)</option> }
                            </select>
                        </label>
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Producer"</span>
                            <select name="hire" class=(ui::INPUT) data-bind:pick="">
                                <option value="">"The studio's producer"</option>
                                for h in producers { <option value=(h.clone())>(h)</option> }
                            </select>
                        </label>
                    </div>
                    <label class=(ui::FIELD)>
                        <span class=(ui::FIELD_LABEL)>"Goal"</span>
                        <textarea name="text" required="" class=(ui::TEXTAREA) placeholder="What should be achieved. A producer files the tickets."></textarea>
                    </label>
                </div>
                <div class=(ui::DIALOG_FOOT)>
                    <span class="spacer"></span>
                    <button type="button" class=(ui::GHOST) popovertarget="office-new-goal" popovertargetaction="hide">"Cancel"</button>
                    <button type="submit" class=(ui::PRIMARY)>"Start goal"</button>
                </div>
            </form>
        )
    })
}

/// Short ages, as `relative` without "ago": "now", "2m", "3h", "2d".
pub fn ago(ms: u64) -> String {
    let words = relative(ms, 0);
    words
        .strip_suffix(" ago")
        .map_or_else(|| "now".into(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_are_safe_to_quote() {
        assert_eq!(clean("tickets/GAME-3"), "tickets/GAME-3");
        assert_eq!(clean("hires/dev'; alert(1)"), "hires/devalert1");
        assert_eq!(clean("other/x"), "");
        assert_eq!(clean(""), "");
        assert_eq!(
            select("goals/EDIT-1"),
            format!("$sel = 'goals/EDIT-1'; @get('/office/panel'); {SHOW_PANEL}")
        );
    }

    #[tokio::test]
    async fn the_judge_is_found_behind_a_long_run_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("runs")).unwrap();
        let output = "x".repeat(40 * 1024);
        let mut log = vec![
            r#"{"at":1,"note":"review by old (opus)"}"#.to_string(),
            r#"{"at":2,"note":"checks passed"}"#.to_string(),
            r#"{"at":3,"note":"review by reviewer (opus)"}"#.to_string(),
        ];
        // Far more tool output after the note than a tail or a chunk holds.
        for at in 4..10 {
            log.push(format!(
                r#"{{"at":{at},"event":{{"type":"tool_result","output":"{output}"}}}}"#
            ));
        }
        std::fs::write(dir.path().join("runs/G-1.jsonl"), log.join("\n")).unwrap();
        let all = 0..u64::MAX;
        let judge = find_back(dir.path(), "G-1", all.clone(), review_line).await;
        assert_eq!(
            judge.flatten().map(|j| (j.hire, j.at)),
            Some(("reviewer".into(), 3))
        );
        // Before its reviewer starts, a review has no judge, not the last one.
        let early = log[..2].join("\n");
        std::fs::write(dir.path().join("runs/G-2.jsonl"), &early).unwrap();
        let judge = find_back(dir.path(), "G-2", all.clone(), review_line).await;
        assert_eq!(judge, Some(None));
        assert_eq!(find_back(dir.path(), "G-3", all, review_line).await, None);
        // Read once, then only what the log gained: a critic taking over.
        assert_eq!(judge_now(dir.path(), "G-2").await, None);
        let critic = format!(
            "{early}\n{}\n",
            r#"{"at":12,"note":"gate art by critic (sonnet)"}"#
        );
        std::fs::write(dir.path().join("runs/G-2.jsonl"), critic).unwrap();
        let judge = judge_now(dir.path(), "G-2").await.unwrap();
        assert_eq!(
            (judge.hire.as_str(), judge.gate.as_deref()),
            ("critic", Some("art"))
        );
        // Nothing new: the judge stands.
        let again = judge_now(dir.path(), "G-2").await.unwrap();
        assert_eq!(again.hire, "critic");
    }

    #[test]
    fn the_window_says_how_far_back_it_reaches() {
        let at = |cut_at| Office {
            now: 100 * 3_600_000,
            cut_at,
            ..Office::default()
        };
        assert_eq!(at(None).window(), "the last 24 hours");
        assert_eq!(at(Some(91 * 3_600_000)).window(), "the last 9 hours");
        assert_eq!(at(Some(99 * 3_600_000)).window(), "the last hour");
        assert_eq!(
            at(Some(100 * 3_600_000 - 20 * 60_000)).window(),
            "the last 20 minutes"
        );
    }

    #[test]
    fn ages_read_short() {
        assert_eq!(ago(30_000), "now");
        assert_eq!(ago(120_000), "2m");
        assert_eq!(ago(7_300_000), "2h");
        assert_eq!(ago(3 * 86_400_000), "3d");
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

    #[test]
    fn a_field_left_empty_is_absent() {
        assert_eq!(absent("  "), None);
        assert_eq!(absent(" main "), Some("main".into()));
    }

    #[test]
    fn game_elements_can_be_selected() {
        for element in model::Element::ALL {
            let sel = format!("elements/{}", element.key());
            assert_eq!(clean(&sel), sel);
        }
        assert_eq!(clean("elements/nope"), "");
        assert_eq!(clean("elements/"), "");
    }
}
