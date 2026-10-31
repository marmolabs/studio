use serde::Deserialize;
use topcoat::{
    context::{Cx, app_context},
    cookie::{Cookie, Cookies, SameSite, cookies, time},
    router::{
        Slot,
        content::Form,
        error::see_other,
        layout,
        request::uri,
        response::{IntoResponse, Response},
        route,
    },
    view::{Unescaped, View, component, view},
};

use std::collections::BTreeMap;
use studiod::api::{HireDay, Request};

use crate::{AppConfig, client, pages, statics, ui};

/// Sidebar links: (href, label, icon), the owner's work first.
const WORK: [(&str, &str, &str); 4] = [
    OFFICE,
    BOARD,
    ("/asks", "Asks", "inbox"),
    ("/goals", "Goals", "target"),
];
/// The office is the home page.
const OFFICE: (&str, &str, &str) = ("/", "Office", "building-2");
const BOARD: (&str, &str, &str) = ("/board", "Board", "layout-dashboard");
const STUDIO: [(&str, &str, &str); 3] = [
    ("/hires", "Hires", "users"),
    ("/rules", "Rules", "shield-check"),
    ("/budget", "Budget", "wallet"),
];

const NAV_LINK: &str = "flex h-8 shrink-0 items-center gap-2.5 rounded-md px-2 font-medium text-fg-2 transition-colors hover:bg-bg-3 hover:text-fg aria-[current=page]:bg-bg-3 aria-[current=page]:text-fg max-[760px]:h-[30px] max-[760px]:whitespace-nowrap max-[760px]:px-2.5 max-[760px]:[&>.i]:hidden";

/// A link in the Office/Board switch.
const SEGMENT: &str = "inline-flex h-[26px] items-center gap-1.5 rounded-md px-2.5 text-[12.5px] font-medium leading-none text-fg-2 hover:text-fg aria-[current=page]:bg-bg aria-[current=page]:text-fg aria-[current=page]:shadow-[0_0_0_1px_var(--line-2)]";

/// The cookie that forces a theme; without it the system decides.
const THEME_COOKIE: &str = "theme";

/// The top bar's title: the page name, plus what is open inside it, shown
/// as a breadcrumb: the ticket on a ticket page ("Board › GAME-3"), the hire
/// being edited ("Hires › dev").
fn title(path: &str, query: &str) -> (String, Option<String>) {
    let mut parts = path.trim_matches('/').split('/');
    let first = parts.next().unwrap_or_default();
    let param = |key: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (k == key).then(|| v.to_owned())
        })
    };
    match (first, parts.next()) {
        ("tickets", Some(id)) => ("Board".into(), Some(id.to_owned())),
        ("hires", None) => match param("file").filter(|f| !f.is_empty()) {
            Some(file) => ("Hires".into(), Some(file)),
            None => ("Hires".into(), None),
        },
        ("", _) => ("Office".into(), None),
        (name, _) => {
            let label = WORK
                .iter()
                .chain(&STUDIO)
                .find(|(href, ..)| href.trim_start_matches('/') == name)
                .map(|(_, label, _)| (*label).to_owned());
            let fallback = || {
                let mut chars = name.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            };
            (label.unwrap_or_else(fallback), None)
        }
    }
}

/// A line beside the page's title in the top bar, for pages that need one.
fn note(path: &str) -> Option<&'static str> {
    match path {
        "/budget" => Some(
            "Spend over the last 24h. A run only starts if its allowance fits under every limit.",
        ),
        _ => None,
    }
}

/// What the approver did for the owner in the last 24 hours, for the Asks
/// page; `None` if it decided nothing or studiod can't say.
async fn approver_note(socket: &std::path::Path) -> Option<String> {
    let days: BTreeMap<String, HireDay> = client::call(socket, Request::HiresToday).await.ok()?;
    approver_line(days.values())
}

/// "The approver decided 96 calls in the last 24h and sent you 4." Only the
/// approver decides calls, so its counts are the only ones.
fn approver_line<'a>(days: impl Iterator<Item = &'a HireDay>) -> Option<String> {
    let (decided, to_owner) = days.fold((0, 0), |(n, o), d| {
        (n + d.allowed + d.denied + d.to_owner, o + d.to_owner)
    });
    let calls = if decided == 1 { "call" } else { "calls" };
    (decided > 0).then(|| {
        format!(
            "The approver decided {decided} {calls} in the last 24h and sent you\u{a0}{to_owner}."
        )
    })
}

/// Whether the link to `href` is the page at `path`.
fn current(href: &str, path: &str) -> bool {
    match href {
        "/" => path == "/",
        "/board" => path.starts_with("/board") || path.starts_with("/tickets"),
        _ => path.starts_with(href),
    }
}

/// The count beside a sidebar link, when there is one to show.
fn count(label: &str, status: &client::Status) -> Option<u64> {
    let n = match label {
        "Asks" => status.asks,
        "Goals" => status.goals,
        "Hires" => status.hires.len() as u64,
        "Rules" => status.rules,
        _ => 0,
    };
    (n > 0).then_some(n)
}

/// The theme the owner forced, if any.
fn chosen_theme(cx: &Cx) -> Option<&'static str> {
    match cookies(cx).get(THEME_COOKIE)?.value() {
        "light" => Some("light"),
        "dark" => Some("dark"),
        _ => None,
    }
}

/// What the theme switch shows on a system that prefers `system`: the icon
/// of the theme in use, the value that flips it (back to `system` when the
/// other theme is the system's own), and the button's label.
fn theme_switch(
    chosen: Option<&str>,
    system: &'static str,
) -> (&'static str, &'static str, String) {
    let shown = chosen.unwrap_or(system);
    let other = if shown == "dark" { "light" } else { "dark" };
    let value = if other == system { "system" } else { other };
    let icon = if shown == "dark" { "moon" } else { "sun" };
    (icon, value, format!("Switch to the {other} theme"))
}

#[layout("/")]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    // The shell still renders when studiod is down; the page says why.
    let status: client::Status = client::call(&config.socket, Request::Status)
        .await
        .unwrap_or_default();
    let path = uri(cx).path().to_owned();
    let here = uri(cx)
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    let query = uri(cx).query().unwrap_or_default().to_owned();
    let (page, crumb) = title(&path, &query);
    // A crumb leads back to the page it is in.
    let crumb_home = if path.starts_with("/tickets") {
        BOARD.0
    } else {
        "/hires"
    };
    let note = match path.as_str() {
        "/asks" => approver_note(&config.socket).await,
        _ => note(&path).map(str::to_string),
    };
    let queued = status.tickets.get("queued").copied().unwrap_or(0);
    let (spent, daily) = (status.spent_24h, status.daily_usd);
    let theme = chosen_theme(cx);
    // One button for each system preference; CSS shows the one that applies.
    let switches = [
        (
            theme_switch(theme, "dark"),
            "[@media(prefers-color-scheme:light)]:hidden",
        ),
        (
            theme_switch(theme, "light"),
            "[@media(prefers-color-scheme:dark)]:hidden",
        ),
    ];
    let link = |(href, label, icon): (&'static str, &'static str, &'static str)| NavLink {
        href,
        label,
        icon,
        current: current(href, &path),
        count: count(label, &status),
    };
    let work: Vec<NavLink> = WORK.into_iter().map(link).collect();
    let studio: Vec<NavLink> = STUDIO.into_iter().map(link).collect();
    // The office collapses the sidebar to an icon rail on a wide screen.
    let rail = path == OFFICE.0;
    let side = if rail {
        "contents min-[761px]:hidden"
    } else {
        "contents"
    };
    let switch: Vec<NavLink> = if rail || path == BOARD.0 {
        [OFFICE, BOARD].into_iter().map(link).collect()
    } else {
        Vec::new()
    };
    // The office's figures count held tickets by why they are held.
    let tickets: Vec<studio_core::ticket::Ticket> = if rail {
        client::call(&config.socket, Request::Tickets)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let figures = pages::office::Hud::of(&status, &tickets);
    let running = status.at_work();
    let rail_links: Vec<NavLink> = if rail {
        WORK.into_iter().chain(STUDIO).map(link).collect()
    } else {
        Vec::new()
    };

    Ok(view! {
        <!DOCTYPE html>
        <html lang="en" data-theme=(theme)>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"studio"</title>
                <link rel="preload" href=(statics::SANS) as="font" type="font/woff2" crossorigin="">
                <link rel="stylesheet" href=(statics::STYLESHEET)>
                <script type="module" src=(statics::DATASTAR)></script>
            </head>
            <body>
                (Unescaped::new_unchecked(statics::ICONS))
                (Unescaped::new_unchecked(ui::GLYPHS))
                <div class="flex min-h-screen max-[760px]:flex-col">
                    if rail {
                        icon_rail(links: rail_links, running: running, queued: queued, here: here.clone(), switches: switches.clone())
                    }
                    <div class=(side)>
                    <aside class="sticky top-0 flex h-screen w-58 shrink-0 flex-col gap-1 border-r border-line bg-bg px-3 pb-3 pt-3.5 max-[760px]:z-[6] max-[760px]:h-auto max-[760px]:w-full max-[760px]:gap-0 max-[760px]:border-b max-[760px]:border-r-0 max-[760px]:px-4 max-[760px]:pb-2 max-[760px]:pt-2.5" aria-label="Studio">
                        <div class="mb-2.5 flex h-8 items-center gap-2.5 px-1.5 max-[760px]:mb-1.5 max-[760px]:px-0">
                            <a href="/" class="flex items-center gap-2.5">
                                <span class="grid size-[22px] shrink-0 place-items-center rounded-md bg-primary text-on-primary">
                                    <svg class="size-3.5" aria-hidden="true"><use href="#logo"></use></svg>
                                </span>
                                <b class="text-[14.5px] font-semibold tracking-[-0.01em]">"studio"</b>
                            </a>
                            <a href="/budget" class="ml-auto hidden items-center gap-1.5 text-[12.5px] tabular-nums text-fg-2 max-[760px]:inline-flex">
                                if running > 0 {
                                    <span class="pulse"></span>
                                    (running) " running · "
                                }
                                (ui::usd(spent))
                            </a>
                        </div>
                        <div class="max-[760px]:-mx-1 max-[760px]:flex max-[760px]:gap-0.5 max-[760px]:overflow-x-auto max-[760px]:[scrollbar-width:none]">
                            <nav class="flex flex-col gap-0.5 max-[760px]:flex-row" aria-label="Work">
                                for link in work { nav_link(link: link) }
                            </nav>
                            <div class="px-2 pb-1.5 pt-[18px] text-[12px] font-medium leading-4 text-fg-3 max-[760px]:hidden">"Studio"</div>
                            <nav class="flex flex-col gap-0.5 max-[760px]:flex-row" aria-label="Studio">
                                for link in studio { nav_link(link: link) }
                            </nav>
                        </div>
                        <div class="mt-auto flex flex-col gap-2.5 border-t border-line px-2 pb-1 pt-3 max-[760px]:hidden">
                            <div class="flex items-center gap-2 text-[13px] text-fg-2">
                                if running > 0 {
                                    <span class="pulse"></span>
                                    <span>
                                        <b class="font-medium text-fg">(running) " running"</b>
                                        if queued > 0 { " · " (queued) " queued" }
                                    </span>
                                } else {
                                    <span class="size-2 shrink-0 rounded-full bg-fg-3"></span>
                                    <span>
                                        "Idle"
                                        if queued > 0 { " · " (queued) " queued" }
                                    </span>
                                }
                            </div>
                            <a href="/budget" class="flex flex-col gap-1.5" title="Spend over the last 24 hours, against the daily limit">
                                <span class="flex items-baseline justify-between text-[13px]">
                                    <span class="tabular-nums">(ui::usd(spent)) <span class="text-fg-3">" of " (ui::limit(daily))</span></span>
                                    <span class="text-fg-3">"last 24h"</span>
                                </span>
                                ui::meter(value: spent, max: daily)
                            </a>
                            <form method="post" action="/theme" class="flex justify-end">
                                <input type="hidden" name="back" value=(here)>
                                for ((icon, value, label), hide) in switches {
                                    <button type="submit" name="theme" value=(value) class=(format!("{} {hide}", ui::ICON_BUTTON_SM)) aria-label=(label.clone()) title=(label)>
                                        ui::icon(name: icon)
                                    </button>
                                }
                            </form>
                        </div>
                    </aside>
                    </div>
                    // `relative` anchors `ui::top_actions` on a phone.
                    <div class="relative flex min-w-0 flex-1 flex-col">
                        <header class="sticky top-0 z-[5] flex h-14 shrink-0 items-center gap-3 border-b border-line bg-bg/90 px-8 backdrop-blur-[10px] backdrop-saturate-[1.4] max-[760px]:static max-[760px]:h-auto max-[760px]:min-h-[52px] max-[760px]:flex-wrap max-[760px]:gap-y-1 max-[760px]:px-4 max-[760px]:py-2.5">
                            // A ticket's own title is that page's h1.
                            if let Some(id) = crumb {
                                <nav aria-label="Breadcrumb" class="flex min-w-0 items-center gap-1.5 text-[14px] text-fg-3">
                                    <a href=(crumb_home) class="hover:text-fg">(page)</a>
                                    ui::icon(name: "chevron-right", small: true)
                                    <span class="truncate font-mono text-[12.5px] font-medium text-fg">(id)</span>
                                </nav>
                            } else {
                                <h1 class="min-w-0 shrink-0 truncate text-[15px] font-semibold leading-5 tracking-[-0.005em]">(page)</h1>
                                if let Some(note) = note {
                                    <span class="min-w-0 truncate text-[13px] text-fg-3 max-[760px]:basis-full max-[760px]:whitespace-normal">(note)</span>
                                }
                            }
                            if !switch.is_empty() {
                                <nav class="inline-flex shrink-0 gap-0.5 rounded-lg bg-bg-3 p-0.5" aria-label="View">
                                    for link in switch {
                                        <a href=(link.href) class=(SEGMENT) aria-current=(link.current.then_some("page"))>
                                            ui::icon(name: link.icon, small: true)
                                            (link.label)
                                        </a>
                                    }
                                </nav>
                            }
                            if rail {
                                pages::office::hud_bar(hud: figures)
                            }
                        </header>
                        <main class="mx-auto flex w-full max-w-[1200px] flex-col has-[#office]:max-w-[1480px] gap-7 px-8 pb-18 pt-7 max-[760px]:gap-6 max-[760px]:px-4 max-[760px]:pb-14 max-[760px]:pt-5">
                            <div id="notice" class="empty:hidden"></div>
                            (slot)
                        </main>
                    </div>
                </div>
            </body>
        </html>
    })
}

/// One sidebar link, worked out for the page being shown.
struct NavLink {
    href: &'static str,
    label: &'static str,
    icon: &'static str,
    current: bool,
    count: Option<u64>,
}

#[component]
async fn nav_link(link: NavLink) -> topcoat::Result<impl View> {
    let NavLink {
        href,
        label,
        icon,
        current,
        count,
    } = link;
    Ok(view! {
        // On a phone the nav scrolls sideways; the current page's link is
        // brought into view.
        <a href=(href) class=(NAV_LINK) aria-current=(current.then_some("page")) data-init=(current.then_some("el.scrollIntoView({block: 'nearest', inline: 'nearest'})"))>
            ui::icon(name: icon)
            (label)
            if let Some(n) = count {
                if label == "Asks" {
                    <span class="ml-auto rounded-full bg-attn px-[7px] py-[3px] text-[12px] font-medium leading-none tabular-nums text-[#1d1400]" aria-label=(format!("{n} waiting"))>(n)</span>
                } else {
                    <span class="ml-auto text-[12px] font-medium leading-none tabular-nums text-fg-3">(n)</span>
                }
            }
        </a>
    })
}

/// A theme button's (icon, value, label) and the class that hides it.
type Switch = ((&'static str, &'static str, String), &'static str);

/// The office's sidebar on a wide screen: a 64px rail of icons, the asks
/// badge, and the running pulse and theme switch at its foot.
#[component]
async fn icon_rail(
    links: Vec<NavLink>,
    running: u64,
    queued: u64,
    here: String,
    switches: [Switch; 2],
) -> topcoat::Result<impl View> {
    // The work links, then a rule, then the studio's own.
    let split = links.len().saturating_sub(STUDIO.len());
    let mut work = links;
    let studio = work.split_off(split);
    let live = format!("{running} running, {queued} queued");
    Ok(view! {
        <aside class="sticky top-0 flex h-screen w-16 shrink-0 flex-col items-center gap-1 border-r border-line bg-bg px-2.5 pb-3 pt-3.5 max-[760px]:hidden" aria-label="Studio">
            <a href="/" class="mb-3.5 grid h-8 place-items-center" aria-label="studio" title="studio">
                <span class="grid size-[22px] place-items-center rounded-md bg-primary text-on-primary">
                    <svg class="size-3.5" aria-hidden="true"><use href="#logo"></use></svg>
                </span>
            </a>
            <nav class="flex flex-col gap-0.5" aria-label="Work">
                for link in work { rail_link(link: link) }
            </nav>
            <div class="my-2.5 h-px w-6 bg-line" role="presentation"></div>
            <nav class="flex flex-col gap-0.5" aria-label="Studio">
                for link in studio { rail_link(link: link) }
            </nav>
            <div class="mt-auto flex flex-col items-center gap-2.5 border-t border-line pb-1 pt-3">
                if running > 0 {
                    <span class="grid size-7 place-items-center" title=(live.clone()) role="img" aria-label=(live)>
                        <span class="pulse"></span>
                    </span>
                } else {
                    <span class="grid size-7 place-items-center" title="Idle" role="img" aria-label="Idle">
                        <span class="size-2 rounded-full bg-fg-3"></span>
                    </span>
                }
                <form method="post" action="/theme">
                    <input type="hidden" name="back" value=(here)>
                    for ((icon, value, label), hide) in switches {
                        <button type="submit" name="theme" value=(value) class=(format!("{} {hide}", ui::ICON_BUTTON_SM)) aria-label=(label.clone()) title=(label)>
                            ui::icon(name: icon)
                        </button>
                    }
                </form>
            </div>
        </aside>
    })
}

/// An icon in the rail, named by its title; only the asks count shows.
#[component]
async fn rail_link(link: NavLink) -> topcoat::Result<impl View> {
    let NavLink {
        href,
        label,
        icon,
        current,
        count,
    } = link;
    let asks = count.filter(|_| label == "Asks");
    let name = match asks {
        Some(n) => format!("{label}, {n} waiting"),
        None => label.to_string(),
    };
    Ok(view! {
        <a href=(href) title=(label) aria-label=(name) class="relative flex h-8 w-10 items-center justify-center rounded-md text-fg-2 transition-colors hover:bg-bg-3 hover:text-fg aria-[current=page]:bg-bg-3 aria-[current=page]:text-fg" aria-current=(current.then_some("page"))>
            ui::icon(name: icon)
            if let Some(n) = asks {
                <span class="absolute -right-0.5 top-px rounded-full bg-attn px-[5px] py-0.5 text-[10px] font-medium leading-none tabular-nums text-[#1d1400]" aria-hidden="true">(n)</span>
            }
        </a>
    })
}

#[derive(Deserialize)]
struct ThemeForm {
    /// `light`, `dark`, or anything else for the system's.
    theme: String,
    /// The page to go back to.
    #[serde(default)]
    back: String,
}

/// Sets or clears the theme cookie, then goes back to the page.
#[route(POST "/theme")]
async fn set_theme(cx: &Cx, Form(form): Form<ThemeForm>) -> topcoat::Result<Response> {
    let jar = cookies(cx);
    match form.theme.as_str() {
        theme @ ("light" | "dark") => jar.add(
            Cookie::build((THEME_COOKIE, theme.to_owned()))
                .path("/")
                .max_age(time::Duration::days(365))
                .same_site(SameSite::Lax)
                .build(),
        ),
        // Expired at once, so the browser drops it.
        _ => jar.add(
            Cookie::build((THEME_COOKIE, ""))
                .path("/")
                .removal()
                .build(),
        ),
    }
    see_other(back_to(&form.back)).into_response(cx)
}

/// `back` if it is a path on this site, else the office.
fn back_to(back: &str) -> &str {
    // Browsers drop tabs and newlines from a URL, so "/\t/x" would become "//x".
    let local = back.starts_with('/')
        && !back.starts_with("//")
        && !back.contains('\\')
        && !back.chars().any(char::is_control);
    if local { back } else { "/" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_bar_names_the_page() {
        assert_eq!(title("/", ""), ("Office".into(), None));
        assert_eq!(title("/board", ""), ("Board".into(), None));
        assert_eq!(title("/goals", "open=G-1"), ("Goals".into(), None));
        assert_eq!(title("/hires", ""), ("Hires".into(), None));
        assert_eq!(
            title("/hires", "file=dev"),
            ("Hires".into(), Some("dev".into()))
        );
        assert_eq!(
            title("/tickets/GAME-3", ""),
            ("Board".into(), Some("GAME-3".into()))
        );
    }

    #[test]
    fn the_asks_page_says_what_the_approver_did() {
        let approver = HireDay {
            allowed: 81,
            denied: 11,
            to_owner: 4,
            ..HireDay::default()
        };
        let dev = HireDay {
            done: 3,
            ..HireDay::default()
        };
        assert_eq!(
            approver_line([&approver, &dev].into_iter()).as_deref(),
            Some("The approver decided 96 calls in the last 24h and sent you\u{a0}4.")
        );
        assert_eq!(approver_line([&dev].into_iter()), None);
    }

    #[test]
    fn links_light_up_for_their_pages() {
        assert!(current("/board", "/tickets/GAME-3"));
        assert!(current("/", "/"));
        assert!(!current("/", "/goals"));
        assert!(!current("/", "/board"));
        assert!(current("/rules", "/rules"));
    }

    #[test]
    fn the_theme_switch_flips_what_is_shown() {
        // No choice yet: flip away from the system's theme.
        assert_eq!(theme_switch(None, "dark").0, "moon");
        assert_eq!(theme_switch(None, "dark").1, "light");
        assert_eq!(theme_switch(None, "light").1, "dark");
        // Flipping back to the system's theme clears the choice.
        assert_eq!(theme_switch(Some("light"), "dark").1, "system");
        assert_eq!(theme_switch(Some("dark"), "light").1, "system");
        // A choice that matches the system still flips to the other.
        assert_eq!(theme_switch(Some("dark"), "dark").1, "light");
    }

    #[test]
    fn going_back_stays_on_this_site() {
        assert_eq!(back_to("/goals?x=1"), "/goals?x=1");
        assert_eq!(back_to("//evil.example"), "/");
        assert_eq!(back_to("https://evil.example"), "/");
        assert_eq!(back_to("/\\evil.example"), "/");
        assert_eq!(back_to("/\t/evil.example"), "/");
        assert_eq!(back_to("/\r\n/evil.example"), "/");
        assert_eq!(back_to(""), "/");
    }
}
