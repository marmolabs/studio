use futures_util::Stream;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::Ticket;
use studiod::api::Request;
use topcoat::{
    context::{Cx, app_context},
    datastar::{PatchElements, Signals},
    router::{
        content::sse::{Event, Sse},
        page, path_param, route,
    },
    view::{Unescaped, View, view},
};

use crate::office::{
    layout::{Kind, Layout, presets},
    render::{self, Props},
    scene,
};
use crate::{AppConfig, client, live, ui};

path_param!(key);

/// A landing this recent still throws confetti.
const CONFETTI_MS: u64 = 120_000;
/// How many of the newest landed tickets get their history checked.
const LANDED_CHECKED: usize = 4;

#[page("/office")]
async fn office_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let layout = Layout::load(&config.data_dir).await;
    let office = snapshot(&config.socket, &config.data_dir).await?;
    let overlay = render::grid_overlay(&layout);
    let palette: Vec<(&'static str, String)> = Kind::ALL
        .iter()
        .map(|k| (k.name(), render::thumb(*k)))
        .collect();
    let tickets: Vec<Ticket> = client::call(&config.socket, Request::Tickets).await?;
    let counts = counts(&tickets);
    Ok(view! {
        <style>(Unescaped::new_unchecked(render::CSS))</style>
        <div data-signals="{edit: false, item: 'desk', x: 0, y: 0}" data-init="@get('/office/stream')">
            <div class="mb-3 flex flex-wrap items-center gap-2">
                <h1 class=(format!("mr-2 {}", ui::PAGE_TITLE))>"Office"</h1>
                for (key, label) in presets::ALL {
                    <button
                        type="button"
                        class=(ui::BUTTON)
                        data-on:click=(format!("@post('/office/preset/{key}')"))
                    >
                        (label)
                    </button>
                }
                <button
                    type="button"
                    class=(ui::BUTTON)
                    data-on:click="$edit = !$edit"
                    data-class:ring-2="$edit"
                    data-class:ring-indigo-500="$edit"
                >
                    "Edit"
                </button>
            </div>
            <div data-show="$edit" style="display:none" class="mb-3 flex flex-wrap items-center gap-1.5 rounded-lg border border-zinc-200 bg-white p-2 dark:border-zinc-800 dark:bg-zinc-900">
                for (name, thumb) in palette {
                    <button
                        type="button"
                        title=(name)
                        class="flex h-10 min-w-10 items-center justify-center rounded-md border border-zinc-200 bg-[#cf9d63] px-1 dark:border-zinc-700"
                        data-on:click=(format!("$item = '{name}'"))
                        data-class:ring-2=(format!("$item == '{name}'"))
                        data-class:ring-indigo-500=(format!("$item == '{name}'"))
                    >
                        (Unescaped::new_unchecked(thumb))
                    </button>
                }
                <button
                    type="button"
                    title="erase"
                    class=(format!("{} h-10", ui::DANGER))
                    data-on:click="$item = 'erase'"
                    data-class:ring-2="$item == 'erase'"
                    data-class:ring-rose-500="$item == 'erase'"
                >
                    "Erase"
                </button>
                <span class=(format!("text-[12px] {}", ui::MUTED))>"Pick an item, then click a tile."</span>
            </div>
            <div class="relative overflow-hidden rounded-lg border border-zinc-200 shadow-sm dark:border-zinc-800">
                (Unescaped::new_unchecked(office))
                (Unescaped::new_unchecked(overlay))
            </div>
            <div class=(format!("mt-3 flex flex-wrap gap-x-4 gap-y-1 text-[12px] {}", ui::MUTED))>
                <span>"Queued " <b class="tabular-nums">(counts[0])</b></span>
                <span>"Working " <b class="tabular-nums">(counts[1])</b></span>
                <span>"Needs you " <b class="tabular-nums">(counts[2])</b></span>
                <span>"Ready " <b class="tabular-nums">(counts[3])</b></span>
                <span class="ml-auto">"Click a hire to open their ticket."</span>
            </div>
        </div>
    })
}

#[route(GET "/office/stream")]
async fn office_stream(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    Ok(live::watch(move || {
        let socket = config.socket.clone();
        let data_dir = config.data_dir.clone();
        async move { snapshot(&socket, &data_dir).await }
    }))
}

#[derive(Deserialize)]
struct Place {
    x: i64,
    y: i64,
    item: String,
}

#[route(POST "/office/place")]
async fn place(cx: &Cx, Signals(wanted): Signals<Place>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let mut layout = Layout::load(&config.data_dir).await;
    if let Err(message) = layout.place(wanted.x, wanted.y, &wanted.item) {
        return ui::notice(cx, message, true).await;
    }
    save_and_show(cx, layout).await
}

#[route(POST "/office/preset/{key}")]
async fn preset(cx: &Cx) -> topcoat::Result<PatchElements> {
    let key = path_param::<Key>(cx).to_owned();
    match presets::by_key(&key) {
        Some(layout) => save_and_show(cx, layout).await,
        None => ui::notice(cx, format!("No layout called {key}."), true).await,
    }
}

async fn save_and_show(cx: &Cx, layout: Layout) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    if let Err(err) = layout.save(&config.data_dir).await {
        return ui::notice(cx, format!("Couldn't save the layout: {err}"), true).await;
    }
    Ok(PatchElements::new(
        snapshot(&config.socket, &config.data_dir).await?,
    ))
}

/// Queued, working, needs you, ready.
fn counts(tickets: &[Ticket]) -> [u32; 4] {
    let mut counts = [0; 4];
    for ticket in tickets {
        let slot = match ticket.state.kind() {
            "queued" => 0,
            "claimed" | "working" | "checking" | "reviewing" => 1,
            "asking" | "held" => 2,
            "ready" => 3,
            _ => continue,
        };
        counts[slot] += 1;
    }
    counts
}

/// The office as it is right now, ready to patch into the page.
async fn snapshot(socket: &Path, data_dir: &Path) -> topcoat::Result<String> {
    let tickets: Vec<Ticket> = client::call(socket, Request::Tickets).await?;
    let status: client::Status = client::call(socket, Request::Status).await?;
    let layout = Layout::load(data_dir).await;
    let placed = scene::assign(&layout, &status.hires, &tickets);
    let props = Props {
        counts: counts(&tickets),
        spent: status.spent_24h,
        budget: status.daily_usd,
        hour: local_hour().await,
        landed: recent_landings(socket, &tickets).await,
    };
    Ok(render::office(&layout, &placed, &props))
}

/// Hires whose ticket landed within the last couple of minutes. Only the
/// newest few landed tickets are looked up.
async fn recent_landings(socket: &Path, tickets: &[Ticket]) -> HashSet<String> {
    let mut landed: Vec<&Ticket> = tickets
        .iter()
        .filter(|t| t.state.kind() == "landed")
        .collect();
    landed.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    let now = ui::now_ms();
    let mut hires = HashSet::new();
    for ticket in landed.into_iter().take(LANDED_CHECKED) {
        let request = Request::Events {
            since: 0,
            limit: 500,
            ticket: Some(ticket.id.to_string()),
        };
        let events: Vec<client::Stamped> = client::call(socket, request).await.unwrap_or_default();
        let fresh = events.iter().any(|e| {
            matches!(e.event, TicketEvent::Landed) && now.saturating_sub(e.at_ms) < CONFETTI_MS
        });
        if fresh {
            hires.insert(ticket.hire.to_string());
        }
    }
    hires
}

/// How long a local hour read from `date` is reused.
const HOUR_CACHE_MS: u64 = 60_000;

/// The server's local hour, read at most once a minute.
async fn local_hour() -> u32 {
    static CACHE: Mutex<Option<(u64, u32)>> = Mutex::new(None);
    let now = ui::now_ms();
    if let Ok(cache) = CACHE.lock()
        && let Some((at, hour)) = *cache
        && now.saturating_sub(at) < HOUR_CACHE_MS
    {
        return hour;
    }
    let hour = read_local_hour().await;
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((now, hour));
    }
    hour
}

/// Asks `date`, since the standard library has no time zones; falls back
/// to UTC.
async fn read_local_hour() -> u32 {
    let output = tokio::process::Command::new("date")
        .arg("+%H")
        .output()
        .await
        .ok();
    output
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|h| *h < 24)
        .unwrap_or_else(|| ((ui::now_ms() / 3_600_000) % 24) as u32)
}
