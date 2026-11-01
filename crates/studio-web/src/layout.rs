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

use studiod::api::Request;

use crate::{AppConfig, client, statics, ui};

/// The studio's mark on a dark tile, inline so no request is made for it.
const FAVICON: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Crect width='16' height='16' rx='4' fill='%2318181b'/%3E%3Crect x='3.5' y='5.8' width='6.7' height='6.7' rx='1.6' fill='none' stroke='%23f4f4f5' stroke-width='1.5'/%3E%3Cpath d='M6.2 3.3h5a1.5 1.5 0 0 1 1.5 1.5v5' fill='none' stroke='%23f4f4f5' stroke-width='1.5' stroke-linecap='round'/%3E%3C/svg%3E";

/// Sidebar links: (href, label, icon), the office first.
const WORK: [(&str, &str, &str); 1] = [OFFICE];
/// The office is the home page.
const OFFICE: (&str, &str, &str) = ("/", "Office", "building-2");
const STUDIO: [(&str, &str, &str); 2] = [
    ("/hires", "Hires", "users"),
    ("/rules", "Rules", "shield-check"),
];
/// Where the money meter leads: the office's Money tab.
const MONEY: &str = "/?tab=money";

const NAV_LINK: &str = "flex h-8 shrink-0 items-center gap-2.5 rounded-md px-2 font-medium text-fg-2 transition-colors hover:bg-bg-3 hover:text-fg aria-[current=page]:bg-bg-3 aria-[current=page]:text-fg max-[760px]:h-[30px] max-[760px]:whitespace-nowrap max-[760px]:px-2.5 max-[760px]:[&>.i]:hidden";

/// The cookie that forces a theme; without it the system decides.
const THEME_COOKIE: &str = "theme";

/// The top bar's title: the page name, plus what is open inside it, shown
/// as a breadcrumb: the ticket on a ticket page ("Office › SHOP-3"), the hire
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
        ("tickets", Some(id)) => ("Office".into(), Some(id.to_owned())),
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

/// Whether the link to `href` is the page at `path`.
fn current(href: &str, path: &str) -> bool {
    match href {
        // A ticket is seen from the office.
        "/" => path == "/" || path.starts_with("/tickets"),
        _ => path.starts_with(href),
    }
}

/// The count beside a sidebar link, when there is one to show.
fn count(label: &str, status: &client::Status) -> Option<u64> {
    let n = match label {
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

/// The two theme buttons, one for each system preference; CSS shows the one
/// that applies.
pub fn theme_switches(cx: &Cx) -> [Switch; 2] {
    let theme = chosen_theme(cx);
    [
        (
            theme_switch(theme, "dark"),
            "[@media(prefers-color-scheme:light)]:hidden",
        ),
        (
            theme_switch(theme, "light"),
            "[@media(prefers-color-scheme:dark)]:hidden",
        ),
    ]
}

/// A theme button's (icon, value, label) and the class that hides it.
pub type Switch = ((&'static str, &'static str, String), &'static str);

/// The theme switch for a page's own top bar; it comes back to the page.
#[component]
pub async fn theme_form(switches: [Switch; 2]) -> topcoat::Result<impl View> {
    Ok(view! {
        <form method="post" action="/theme" class="flex">
            <input type="hidden" name="back" value="/">
            for ((icon, value, label), hide) in switches {
                <button type="submit" name="theme" value=(value) class=(format!("{} {hide}", ui::ICON_BUTTON_SM)) aria-label=(label.clone()) title=(label)>
                    ui::icon(name: icon)
                </button>
            }
        </form>
    })
}

#[layout("/")]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let path = uri(cx).path().to_owned();
    // The office has its own top bar, so it needs none of the sidebar's.
    let home = path == OFFICE.0;
    // The shell still renders when studiod is down; the page says why.
    let status: client::Status = if home {
        client::Status::default()
    } else {
        client::call(&config.socket, Request::Status)
            .await
            .unwrap_or_default()
    };
    let here = uri(cx)
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    let query = uri(cx).query().unwrap_or_default().to_owned();
    let (page, crumb) = title(&path, &query);
    // A crumb leads back to the page it is in.
    let crumb_home = if path.starts_with("/tickets") {
        OFFICE.0
    } else {
        "/hires"
    };
    let queued = status.tickets.get("queued").copied().unwrap_or(0);
    let running = status.at_work();
    let (spent, daily) = (status.spent_24h, status.daily_usd);
    let theme = chosen_theme(cx);
    let switches = theme_switches(cx);
    let link = |(href, label, icon): (&'static str, &'static str, &'static str)| NavLink {
        href,
        label,
        icon,
        current: current(href, &path),
        count: count(label, &status),
    };
    let work: Vec<NavLink> = WORK.into_iter().map(link).collect();
    let studio: Vec<NavLink> = STUDIO.into_iter().map(link).collect();

    Ok(view! {
        <!DOCTYPE html>
        <html lang="en" data-theme=(theme)>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"studio"</title>
                <link rel="icon" href=(FAVICON)>
                <link rel="preload" href=(statics::SANS) as="font" type="font/woff2" crossorigin="">
                <link rel="stylesheet" href=(statics::STYLESHEET)>
                <script type="module" src=(statics::DATASTAR)></script>
            </head>
            <body>
                (Unescaped::new_unchecked(statics::ICONS))
                (Unescaped::new_unchecked(ui::GLYPHS))
                if home {
                    // The office draws its own top bar and side panel; a
                    // reply to an action shows over the floor.
                    <div id="notice" class="toast fixed bottom-4 left-1/2 z-20 w-[min(560px,calc(100vw-32px))] -translate-x-1/2 text-[13px] empty:hidden" aria-live="polite"></div>
                    (slot)
                } else {
                <div class="flex min-h-screen max-[760px]:flex-col">
                    <div class="contents">
                    <aside class="sticky top-0 flex h-screen w-58 shrink-0 flex-col gap-1 border-r border-line bg-bg px-3 pb-3 pt-3.5 max-[760px]:z-[6] max-[760px]:h-auto max-[760px]:w-full max-[760px]:gap-0 max-[760px]:border-b max-[760px]:border-r-0 max-[760px]:px-4 max-[760px]:pb-2 max-[760px]:pt-2.5" aria-label="Studio">
                        <div class="mb-2.5 flex h-8 items-center gap-2.5 px-1.5 max-[760px]:mb-1.5 max-[760px]:px-0">
                            <a href="/" class="flex items-center gap-2.5">
                                <span class="grid size-[22px] shrink-0 place-items-center rounded-md bg-primary text-on-primary">
                                    <svg class="size-3.5" aria-hidden="true"><use href="#logo"></use></svg>
                                </span>
                                <b class="text-[14.5px] font-semibold tracking-[-0.01em]">"studio"</b>
                            </a>
                            <a href=(MONEY) class="ml-auto hidden items-center gap-1.5 text-[12.5px] tabular-nums text-fg-2 max-[760px]:inline-flex">
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
                            <a href=(MONEY) class="flex flex-col gap-1.5" title="Spend over the last 24 hours, against the daily limit">
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
                            }
                        </header>
                        <main class="mx-auto flex w-full max-w-[1200px] flex-col has-[#office]:max-w-[1480px] gap-7 px-8 pb-18 pt-7 max-[760px]:gap-6 max-[760px]:px-4 max-[760px]:pb-14 max-[760px]:pt-5">
                            <div id="notice" class="empty:hidden"></div>
                            (slot)
                        </main>
                    </div>
                </div>
                }
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
                <span class="ml-auto text-[12px] font-medium leading-none tabular-nums text-fg-3">(n)</span>
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
        assert_eq!(title("/hires", ""), ("Hires".into(), None));
        assert_eq!(
            title("/hires", "file=dev"),
            ("Hires".into(), Some("dev".into()))
        );
        assert_eq!(
            title("/tickets/GAME-3", ""),
            ("Office".into(), Some("GAME-3".into()))
        );
    }

    #[test]
    fn links_light_up_for_their_pages() {
        assert!(current("/", "/tickets/GAME-3"));
        assert!(current("/", "/"));
        assert!(!current("/", "/hires"));
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
        assert_eq!(back_to("/rules?x=1"), "/rules?x=1");
        assert_eq!(back_to("//evil.example"), "/");
        assert_eq!(back_to("https://evil.example"), "/");
        assert_eq!(back_to("/\\evil.example"), "/");
        assert_eq!(back_to("/\t/evil.example"), "/");
        assert_eq!(back_to("/\r\n/evil.example"), "/");
        assert_eq!(back_to(""), "/");
    }
}
