//! The office: the whole studio as one floor plan, with an inspector beside
//! it for whatever the owner picks. `floor` decides who sits where and draws
//! the rooms; `inspector` draws what needs the owner and each thing's panel.

mod floor;
mod inspector;
mod lanes;

use futures_util::Stream;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::Ticket;
use studiod::api::{AskView, HireDay, RECENT_EVENTS, Request};
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, Signals},
    router::{
        content::sse::{Event, Sse},
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
use crate::{AppConfig, client, diffstat, live, ui};

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
    /// The run log's tail for each ticket on the floor, by id.
    pub logs: HashMap<String, String>,
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

    pub fn log(&self, id: &str) -> &str {
        self.logs.get(id).map_or("", String::as_str)
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
    let mut logs = HashMap::new();
    let mut judges = HashMap::new();
    for ticket in &tickets {
        let id = ticket.id.to_string();
        let state = ticket.state.kind();
        if matches!(
            state,
            "claimed" | "working" | "asking" | "checking" | "reviewing"
        ) {
            logs.insert(id.clone(), log_tail(&config.data_dir, &id).await);
        }
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
        logs,
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
        _ => String::new(),
    }
}

/// What a floor element does when clicked: select it and show its panel,
/// scrolling to the panel where it sits under the floor.
pub fn select(key: &str) -> String {
    format!("$sel = '{key}'; @get('/office/panel'); {SHOW_PANEL}")
}

const SHOW_PANEL: &str =
    "window.innerWidth <= 1180 && document.getElementById('inspector').scrollIntoView()";

/// Whether the element for `key` is the selected one, for `aria-pressed`.
pub fn pressed_when(key: &str) -> String {
    format!("$sel == '{key}'")
}

#[query_params(error = bad_request)]
struct Opened {
    /// Opens the page with this selected, e.g. `tickets/SHOP-3`.
    sel: Option<String>,
}

#[page("/")]
async fn office_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let sel = clean(&query_params::<Opened>(cx)?.sel.clone().unwrap_or_default());
    let office = gather(config).await?;
    let lanes = lanes::load(&config.socket, &office.tickets, office.now).await;
    let now = office.now;
    let detail = inspector::detail(config, &office, &sel, &mut inspector::Memo::default()).await;
    let projects = office.status.projects.clone();
    let team = office.status.team.clone();
    let signals = format!("{{sel: '{sel}', pick: ''}}");
    Ok(view! {
        <div id="office" class="flex flex-col gap-7" data-signals=(signals)>
            ui::top_actions(
                <button type="button" class=(ui::BUTTON) popovertarget="office-new-goal" data-on:click=(NO_PICK)>
                    ui::icon(name: "target")
                    <span class=(ui::LABEL)>"New goal"</span>
                </button>
                <button type="button" class=(ui::PRIMARY) popovertarget="office-new-ticket" data-on:click=(NO_PICK)>
                    ui::icon(name: "plus")
                    <span class=(ui::LABEL)>"New ticket"</span>
                </button>
            )
            new_ticket(projects: projects.clone(), team: team.clone())
            new_goal(projects: projects, team: team)
            // Restarts the live stream whenever the selection changes, so it
            // keeps the picked panel up to date.
            <div hidden="" data-effect="$sel; @get('/office/stream')"></div>
            <div class="grid grid-cols-[minmax(0,1fr)_360px] overflow-hidden rounded-xl border border-line bg-bg max-[1180px]:grid-cols-1">
                (rooms(cx, office, detail, sel))
            </div>
            last_hour(lanes: lanes, now: now)
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

/// The Live page's lanes are the office's "Last hour" now.
#[route(GET "/live")]
async fn old_live() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The floor, the top bar's figures and the inspector, re-sent as they
/// change, for the selection the stream was opened with.
#[route(GET "/office/stream")]
async fn office_stream(
    cx: &Cx,
    signals: Option<Signals<Pick>>,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let cx = cx.clone();
    let sel = clean(&signals.map(|s| s.0.sel).unwrap_or_default());
    // What the panel reads once for this selection, kept between renders.
    let memo = Arc::new(tokio::sync::Mutex::new(inspector::Memo::default()));
    Ok(live::watch(move || {
        let config = config.clone();
        let cx = cx.clone();
        let sel = sel.clone();
        let memo = memo.clone();
        async move { render_all(&cx, &config, &sel, &mut *memo.lock().await).await }
    }))
}

async fn render_all(
    cx: &Cx,
    config: &AppConfig,
    sel: &str,
    memo: &mut inspector::Memo,
) -> topcoat::Result<String> {
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, sel, memo).await;
    let lanes = lanes::load(&config.socket, &office.tickets, office.now).await;
    let now = office.now;
    let figures = Hud::of(&office.status, &office.tickets);
    let sel = sel.to_string();
    ui::html(
        cx,
        view! { cx =>
            hud_bar(hud: figures)
            (rooms(cx, office, detail, sel))
            lanes::hour(rows: lanes, now: now)
        },
    )
    .await
}

/// The inspector for the current selection, right away.
#[route(GET "/office/panel")]
async fn office_panel(cx: &Cx, signals: Option<Signals<Pick>>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let sel = clean(&signals.map(|s| s.0.sel).unwrap_or_default());
    let office = gather(config).await?;
    let detail = inspector::detail(config, &office, &sel, &mut inspector::Memo::default()).await;
    let panel =
        view! { cx => inspector::inspector(office: office, detail: detail, sel: sel) }.boxed();
    let html = ui::html(cx, view! { cx => (panel) }).await?;
    Ok(PatchElements::new(html))
}

/// The floor and the inspector, boxed so their futures live on the heap.
fn rooms(cx: &Cx, office: Office, detail: inspector::Detail, sel: String) -> BoxView<'_> {
    view! { cx =>
        floor::floor(office: office.clone(), sel: sel.clone())
        inspector::inspector(office: office, detail: detail, sel: sel)
    }
    .boxed()
}

/// The top bar's figures on the office page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hud {
    /// Open asks plus tickets held for the owner, as on the board.
    pub needs_you: u64,
    /// Tickets at work: not paused on an ask.
    pub running: u64,
    pub ready: u64,
    pub spent: f64,
    pub daily: f64,
}

impl Hud {
    pub fn of(status: &client::Status, tickets: &[Ticket]) -> Hud {
        let held = tickets
            .iter()
            .filter(|t| t.state.kind() == "held" && ui::needs_owner(&t.state))
            .count();
        Hud {
            needs_you: status.asks + held as u64,
            running: status.at_work(),
            ready: status.tickets.get("ready").copied().unwrap_or(0),
            spent: status.spent_24h,
            daily: status.daily_usd,
        }
    }
}

/// Needs you, running, ready and the last 24 hours' spend, beside the
/// office's title. Hidden where the bar is too narrow for it.
#[component]
pub async fn hud_bar(hud: Hud) -> topcoat::Result<impl View> {
    let figures = [
        ("attn", hud.needs_you, "need you"),
        ("run", hud.running, "running"),
        ("ok", hud.ready, "ready"),
    ];
    Ok(view! {
        <div id="office-hud" class="ml-1.5 flex min-w-0 items-center gap-4 overflow-hidden whitespace-nowrap text-[13px] text-fg-2 max-[1180px]:hidden" aria-label="The studio now">
            for (tone, n, words) in figures {
                <span class="inline-flex items-center gap-1.5">
                    <span class=(format!("size-2 rounded-full {}", match tone {
                        "attn" => "bg-attn",
                        "run" => "bg-run",
                        _ => "bg-ok",
                    }))></span>
                    <b class="font-semibold tabular-nums text-fg">(n)</b>
                    (words)
                </span>
            }
            <a href="/budget" class="inline-flex items-center gap-1.5 hover:text-fg" title="Spend over the last 24 hours, against the daily limit">
                <b class="font-semibold tabular-nums text-fg">(ui::usd(hud.spent))</b>
                " of " (ui::limit(hud.daily))
                <span class="ml-0.5 inline-block w-14">ui::meter(value: hud.spent, max: hud.daily)</span>
            </a>
        </div>
    })
}

/// The "Last hour" drawer under the floor.
#[component]
async fn last_hour(lanes: Vec<lanes::Lane>, now: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <details class="group rounded-[10px] border border-line bg-bg">
            <summary class="flex cursor-pointer list-none items-center gap-2.5 px-[18px] py-3.5 [&::-webkit-details-marker]:hidden">
                <span class="text-fg-3 transition-transform group-open:rotate-90">ui::icon(name: "chevron-right", small: true)</span>
                <h2 class="text-[14px] font-semibold leading-5">"Last hour"</h2>
                <span class="text-[13px] text-fg-3">"What every ticket did, minute by minute"</span>
            </summary>
            lanes::hour(rows: lanes, now: now)
        </details>
    })
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
    fn the_hud_counts_what_needs_you_and_what_runs_as_the_board_does() {
        use serde_json::json;
        // Four runs hold a slot, but one of them waits on an ask.
        let status: client::Status = serde_json::from_value(json!({
            "asks": 2, "running": 4,
            "tickets": {"held": 2, "ready": 4, "working": 2, "reviewing": 1, "asking": 1},
            "daily_usd": 100.0, "spent_24h": 34.12,
        }))
        .unwrap();
        let held = |id: &str, reason: &str| -> Ticket {
            serde_json::from_value(json!({
                "id": id, "project": "game", "title": id, "goal": "", "hire": "dev",
                "state": {"held": {"reason": reason}}, "attempts": 0,
                "base_branch": "main", "created_at": 0,
            }))
            .unwrap()
        };
        // A budget hold starts again by itself; a pause waits for the owner.
        let tickets = [held("G-1", "budget"), held("G-2", "paused")];
        assert_eq!(
            Hud::of(&status, &tickets),
            Hud {
                needs_you: 3,
                running: 3,
                ready: 4,
                spent: 34.12,
                daily: 100.0
            }
        );
    }
}
