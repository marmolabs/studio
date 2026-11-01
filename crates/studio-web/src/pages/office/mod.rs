//! The office: the whole studio as one isometric floor with labels pinned
//! over it, a top bar and a side panel, on one page. `plan` decides who is
//! where, `scene` draws the floor, `hq` draws the page's pieces and `panel`
//! the side panel for whatever the owner picks.

mod facts;
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
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::Ticket;
use studiod::api::{
    AskView, FileKind, HireDay, MAX_RUNS_LIMIT, OfficeView, RECENT_EVENTS, Request, RunView,
};
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, PatchSignals, Signals},
    router::{
        content::{
            Form,
            sse::{Event, Sse},
        },
        error::redirect_permanent,
        page, query_params,
        response::{IntoResponse, Response},
        route,
    },
    view::{BoxView, View, ViewExt, component, view},
};

use super::timeline::relative;
use crate::summary::Judge;
use crate::{AppConfig, client, layout, live, ui};

/// Everything the floor and the inspector show, read from studiod at once.
#[derive(Clone, Debug, Default)]
pub struct Office {
    pub status: client::Status,
    pub tickets: Vec<Ticket>,
    pub asks: Vec<AskView>,
    /// Every ticket's `OFFICE_KINDS` events over the last 24 hours, oldest
    /// first.
    pub recent: Vec<client::Happened>,
    /// When `recent` starts, if studiod cut it short of the 24 hours.
    pub cut_at: Option<u64>,
    /// What each ticket has cost, by id.
    pub spend: BTreeMap<String, f64>,
    /// Lines added and removed by each ready ticket, by id.
    pub sizes: HashMap<String, (u64, u64)>,
    /// Who is judging each ticket in review.
    pub judges: HashMap<String, Judge>,
    /// What each ticket with a run under way is doing, by id.
    pub runs: HashMap<String, RunView>,
    /// Ready tickets that no reviewer on another model read.
    pub unreviewed: Vec<String>,
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

/// How long streams share one read of studiod: every open office asks
/// each second, and they all use the one that is at most this old.
const SHARED_FOR: Duration = Duration::from_secs(1);

/// What studiod last said about the office to one socket.
struct Read {
    at: Instant,
    /// Counts up whenever what studiod said changes, apart from its clock.
    version: u64,
    /// The reply with its clock zeroed and its runs left out, to tell whether
    /// the next differs: a run's steps change all the time, and only the
    /// panel of the ticket they belong to shows them.
    said: OfficeView,
    office: Office,
}

type Slot = Arc<tokio::sync::Mutex<Option<Read>>>;

/// Reads what the office shows, in one request to studiod. Only the status
/// and the tickets must come back; anything else studiod can't say right now
/// is left empty.
async fn gather(config: &AppConfig) -> topcoat::Result<Office> {
    Ok(shared(config, Duration::ZERO).await?.1)
}

/// The office as studiod last said it, if that is under `max_age` old, else
/// as it says now. With the number that changes whenever the office does,
/// apart from the time. Streams of every open page share one read, and
/// those who wait for it use it rather than asking again.
async fn shared(config: &AppConfig, max_age: Duration) -> topcoat::Result<(u64, Office)> {
    static READS: Mutex<BTreeMap<PathBuf, Slot>> = Mutex::new(BTreeMap::new());
    let slot = READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(config.socket.clone())
        .or_default()
        .clone();
    let mut read = slot.lock().await;
    if let Some(read) = read.as_ref()
        && read.at.elapsed() < max_age
    {
        return Ok((read.version, read.office.clone()));
    }
    let mut said: OfficeView = client::call(&config.socket, Request::Office).await?;
    let now = std::mem::take(&mut said.now);
    let runs = std::mem::take(&mut said.runs);
    let version = match read.as_ref() {
        Some(last) if last.said == said => last.version,
        Some(last) => last.version + 1,
        None => 0,
    };
    let office = office_of(said.clone(), runs, now)?;
    *read = Some(Read {
        at: Instant::now(),
        version,
        said,
        office: office.clone(),
    });
    Ok((version, office))
}

/// `Office` from what studiod said at `now`, with `runs` as it said them.
fn office_of(
    said: OfficeView,
    runs: BTreeMap<String, RunView>,
    now: u64,
) -> topcoat::Result<Office> {
    let status = serde_json::from_value(said.status)
        .map_err(|e| client::error(format!("bad reply: {e}")))?;
    let recent: Vec<client::Happened> = said.events.into_iter().map(Into::into).collect();
    Ok(Office {
        status,
        tickets: said.tickets,
        asks: said.asks,
        cut_at: (recent.len() >= RECENT_EVENTS)
            .then(|| recent.first().map(|e| e.at_ms))
            .flatten(),
        recent,
        spend: said.spend,
        sizes: said.sizes.into_iter().collect(),
        judges: said
            .judges
            .into_iter()
            .map(|(id, judge)| (id, judge.into()))
            .collect(),
        runs: runs.into_iter().collect(),
        unreviewed: said.unreviewed,
        merged: said.merged,
        today: said.today,
        now,
    })
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
    /// How many rows of the Projects tab's History are shown.
    #[serde(default)]
    history: usize,
}

impl Pick {
    fn turn(&self) -> model::Turn {
        model::Turn::new(self.turn)
    }

    fn history(&self) -> usize {
        panel::history_rows(self.history)
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
        "{{sel: '{sel}', tab: '{tab}', focus: 'all', zoom: 1, hints: false, turn: {}, history: {}, pick: '', _dragging: false, _found: {{path: '', name: '', branch: '', checks: ''}}}}",
        turn.index(),
        panel::HISTORY_STEP
    );
    Ok(view! {
        <div id="office" class="grid h-dvh grid-rows-[auto_minmax(0,1fr)] max-[900px]:h-auto" data-signals=(signals)>
            // Restarts the live stream whenever the selection or the corner the
            // floor is seen from changes, so it keeps the picked panel up to
            // date and draws the floor from that corner.
            <div hidden="" data-effect="$sel; $turn; $history; @get('/office/stream')"></div>
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
                hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab.to_string(), history: panel::HISTORY_STEP)
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

/// The Board and Goals pages are gone; the Projects tab has them.
#[route(GET "/board")]
async fn old_board() -> topcoat::Result<Response> {
    Err(redirect_permanent("/?tab=projects").into())
}

#[route(GET "/goals")]
async fn old_goals() -> topcoat::Result<Response> {
    Err(redirect_permanent("/?tab=projects").into())
}

/// The Asks page is gone; the Needs you tab has it.
#[route(GET "/asks")]
async fn old_asks() -> topcoat::Result<Response> {
    Err(redirect_permanent("/?tab=needs").into())
}

/// The Budget page is gone; the Money tab has it.
#[route(GET "/budget")]
async fn old_budget() -> topcoat::Result<Response> {
    Err(redirect_permanent("/?tab=money").into())
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
    let (sel, tab, history) = (clean(&pick.sel), panel::tab(&pick.tab), pick.history());
    let kept = Arc::new(tokio::sync::Mutex::new(Kept::default()));
    Ok(live::watch_changes(move || {
        let config = config.clone();
        let cx = cx.clone();
        let sel = sel.clone();
        let kept = kept.clone();
        async move {
            render_all(
                &cx,
                &config,
                &sel,
                tab,
                history,
                turn,
                &mut *kept.lock().await,
            )
            .await
        }
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
    /// What the last render was drawn from, and when.
    drawn: Option<(Instant, Drawn)>,
}

/// What a render is drawn from, to tell whether the next would differ.
#[derive(Clone, Copy, PartialEq)]
struct Drawn {
    /// As `shared` numbers what studiod said.
    office: u64,
    /// Fingerprints of the picked ticket's live detail and the panel's
    /// extra reads.
    detail: u64,
    extra: u64,
}

/// A number that is the same for values that print the same.
fn fingerprint(value: &impl std::fmt::Debug) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    format!("{value:?}").hash(&mut hasher);
    hasher.finish()
}

/// Draws the office, unless what it is drawn from is what the last render
/// drew it from, and under `live::QUIET_MAX` has passed since. Then only
/// the ages ("5m ago") would be newer.
async fn render_all(
    cx: &Cx,
    config: &AppConfig,
    sel: &str,
    tab: &str,
    history: usize,
    turn: model::Turn,
    kept: &mut Kept,
) -> topcoat::Result<Option<String>> {
    let (version, office) = shared(config, SHARED_FOR).await?;
    let detail = inspector::detail(config, &office, sel, &mut kept.memo).await;
    let extra = match &kept.extra {
        Some((at, extra)) if at.elapsed() < EXTRA_FRESH => extra.clone(),
        _ => {
            let extra = panel::Extra::gather(config).await;
            kept.extra = Some((Instant::now(), extra.clone()));
            extra
        }
    };
    let drawn = Drawn {
        office: version,
        detail: fingerprint(&detail),
        extra: fingerprint(&extra),
    };
    if let Some((at, last)) = &kept.drawn
        && *last == drawn
        && at.elapsed() < live::QUIET_MAX
    {
        return Ok(None);
    }
    kept.drawn = Some((Instant::now(), drawn));
    let parts = hq::Parts::of(&office, &extra, turn);
    let shown = Shown {
        sel: sel.to_string(),
        tab: tab.to_string(),
        history,
    };
    let html = ui::html(cx, live_parts(cx, parts, office, detail, extra, shown)).await?;
    Ok(Some(html))
}

/// What the panel shows: the selection, the tab and how much History.
struct Shown {
    sel: String,
    tab: String,
    history: usize,
}

/// Every piece of the page that changes, boxed so their futures live on the
/// heap.
fn live_parts(
    cx: &Cx,
    parts: hq::Parts,
    office: Office,
    detail: inspector::Detail,
    extra: panel::Extra,
    shown: Shown,
) -> BoxView<'_> {
    let Shown { sel, tab, history } = shown;
    view! { cx =>
        hq::projects_strip(rows: parts.rows.clone())
        hq::figures(hud: parts.hud)
        hq::floor_scene(scene: parts.scene, projects: parts.projects, closed: parts.closed, sel: sel.clone(), turn: parts.turn)
        hq::corners(rows: parts.rows, feed: parts.feed)
        hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab, history: history)
    }
    .boxed()
}

/// The panel for the current selection, right away.
#[route(GET "/office/panel")]
async fn office_panel(cx: &Cx, signals: Option<Signals<Pick>>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let pick = signals.map(|s| s.0).unwrap_or_default();
    let (sel, tab) = (clean(&pick.sel), panel::tab(&pick.tab).to_string());
    let history = pick.history();
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, &sel, &mut inspector::Memo::default()).await;
    let extra = panel::Extra::gather(config).await;
    let side =
        view! { cx => hq::side(office: office, detail: detail, extra: extra, sel: sel, tab: tab, history: history) }
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

#[query_params(error = bad_request)]
struct Confirmed {
    /// `yes` once the owner has seen what would be added.
    add: Option<String>,
}

/// Adds a git repository on this computer as a project; studiod fills in
/// what is left out. The first submit only asks studiod what would be added
/// and shows it for the owner to confirm; the Add it button then sends the
/// same fields with `?add=yes`, and only that writes anything.
#[route(POST "/office/projects")]
async fn add_project(cx: &Cx, Form(form): Form<NewProject>) -> topcoat::Result<Response> {
    let config = app_context::<AppConfig>(cx);
    let confirmed = query_params::<Confirmed>(cx)?.add.as_deref() == Some("yes");
    let Some(path) = absent(&form.path) else {
        let words = "Enter the folder of a git repository.";
        return ui::notice(cx, words.into(), true).await?.into_response(cx);
    };
    let name = absent(&form.name);
    let request = Request::AddProject {
        path: path.clone(),
        name: name.clone(),
        branch: absent(&form.branch),
        daily_usd: None,
        pack: None,
        check: !confirmed,
    };
    let added = match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(added) => added,
        Err(err) => {
            return ui::notice(cx, err.to_string(), true)
                .await?
                .into_response(cx);
        }
    };
    let named = added["name"]
        .as_str()
        .map(str::to_string)
        .or(name)
        .unwrap_or_else(|| "the project".into());
    if !confirmed {
        return found(&added, &named, &path)?.into_response(cx);
    }
    let words = match added["checks"].as_str() {
        Some(checks) => format!("Added {named}. Its checks: {checks}"),
        None => format!("Added {named}."),
    };
    ui::notice(cx, words, false).await?.into_response(cx)
}

/// What adding a project would do, for the panel to show before the owner
/// confirms: the name, the full folder, the branch and the checks.
fn found(added: &serde_json::Value, name: &str, typed_path: &str) -> topcoat::Result<PatchSignals> {
    let text = |key: &str| added[key].as_str().map(str::to_string);
    let checks = match text("checks") {
        Some(checks) => {
            format!("Its checks will be guessed and written to {checks}, for you to look over.")
        }
        None => "It already has its own checks.".to_string(),
    };
    PatchSignals::json(&serde_json::json!({
        "_found": {
            "name": name,
            "path": text("path").unwrap_or_else(|| typed_path.to_string()),
            "branch": text("branch").unwrap_or_else(|| "its current branch".to_string()),
            "checks": checks,
        }
    }))
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
