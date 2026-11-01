//! The office's side panel: what needs you, every project, the team, money,
//! rules and the log as tabs, and the panel of whatever the owner picked on
//! the floor.

use serde::Deserialize;
use std::collections::BTreeMap;
use studiod::api::{DaySpend, ProjectCheck, Request, RuleFile, Trophy};
use topcoat::view::{BoxView, Child, View, ViewExt, component, view};

mod elements;
#[cfg(test)]
mod fixtures;
mod log;
mod money;
mod projects;
mod queues;
mod rules;
mod team;

use super::facts::{self, dot_class};
use super::inspector::{self, Detail};
use super::model::{Element, Role};
use super::{Office, key, select};
use crate::{AppConfig, client};
use elements::element_view;
use log::log_pane;
use money::money_pane;
use projects::projects_pane;
pub use projects::{HISTORY_STEP, history_rows};
use rules::rules_pane;
use team::team_pane;

/// What the panel reads from studiod beyond what the office gathers. Each
/// read that fails is left empty, and the panel then shows nothing for it.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    pub budget: Option<Budget>,
    pub rules: Vec<RuleFile>,
    pub trophies: Vec<Trophy>,
    pub checks: Vec<ProjectCheck>,
    /// Spend on each of the last 14 days, oldest first.
    pub days: Vec<DaySpend>,
}

/// The parts of `Request::Budget` the panel shows.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub spent_24h: Spent,
    /// What live runs hold, by ticket.
    #[serde(default)]
    pub reserved: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Limits {
    /// What one ticket may spend.
    pub ticket_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Spent {
    #[serde(default)]
    pub by_hire: BTreeMap<String, f64>,
    #[serde(default)]
    pub by_ticket: BTreeMap<String, f64>,
}

impl Budget {
    /// What live runs hold.
    pub fn held(&self) -> f64 {
        self.reserved.values().sum()
    }
}

impl Extra {
    /// Reads it; anything studiod can't say right now is left empty.
    pub async fn gather(config: &AppConfig) -> Extra {
        let socket = &config.socket;
        let (budget, rules, trophies, checks, days) = tokio::join!(
            client::call::<Budget>(socket, Request::Budget),
            client::call::<Vec<RuleFile>>(socket, Request::RuleFiles),
            client::call::<Vec<Trophy>>(socket, Request::Trophies),
            client::call::<Vec<ProjectCheck>>(socket, Request::ProjectChecks),
            client::call::<Vec<DaySpend>>(socket, Request::SpendByDay { days: 14 }),
        );
        Extra {
            budget: budget.ok(),
            rules: rules.unwrap_or_default(),
            trophies: trophies.unwrap_or_default(),
            checks: checks.unwrap_or_default(),
            days: days.unwrap_or_default(),
        }
    }
}

/// The panel: the tabs when `sel` is empty, with `tab` shown and `history`
/// rows of the Projects tab's History, else the picked thing's panel.
#[component]
pub async fn panel(
    office: Office,
    detail: Detail,
    extra: Extra,
    sel: String,
    tab: String,
    history: usize,
) -> topcoat::Result<impl View> {
    let element = sel.strip_prefix("elements/").and_then(Element::from_key);
    // Boxed, so the panels' futures live on the heap, not the stack.
    let body: BoxView = if let Some(element) = element {
        let inner = element_view(__cx, element, office, extra);
        view! { <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">(inner)</div> }
            .boxed()
    } else if sel.is_empty() || !inspector::finds(&office, &sel) {
        view! {
            <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">
                tabs(office: office, extra: extra, tab: tab, history: history)
            </div>
        }
        .boxed()
    } else {
        view! { inspector::inspector(office: office, detail: detail, sel: sel) }.boxed()
    };
    Ok(view! { (body) })
}

// ---------------------------------------------------------------- shared

/// A project's tint: its place in the list of projects.
fn tint(office: &Office, project: &str) -> usize {
    facts::tint(&office.status.projects, project)
}

/// A skin and a hair tone, from a name, as the floor gives its figures.
fn looks(seed: &str) -> (char, char) {
    let h = seed
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let pick = |n: u32| "abcd".chars().nth((n % 4) as usize).unwrap_or('a');
    (pick(h), pick(h / 4))
}

/// A hire's classes for its portrait: shirt, skin and hair.
fn portrait_class(hire: &str, role: &str) -> String {
    let (skin, hair) = looks(hire);
    format!("{} s-{skin} h-{hair}", Role::of(role).class())
}

/// "Engineer" for `engineer`.
fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A share of a whole, in percent, from 0 to 100.
fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 && part > 0.0 {
        (part / whole * 100.0).min(100.0)
    } else {
        0.0
    }
}

/// A heading over a block of the panel, with a quiet note at its right.
#[component]
async fn section(
    label: &'static str,
    #[into]
    #[default]
    note: String,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <section class="sec">
            <div class="sec-h"><span>(label)</span><span>(note)</span></div>
            (child)
        </section>
    })
}

/// A quiet sentence.
#[component]
async fn quiet(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! { <p class="m-0 text-[13px] leading-5 text-fg-3">(text)</p> })
}

/// A ticket in a list: its project's dot, id, title and a note. Picking it
/// opens its panel.
#[component]
async fn line(
    id: String,
    title: String,
    tint: usize,
    #[into]
    #[default]
    note: String,
) -> topcoat::Result<impl View> {
    let pick = select(&key("tickets", &id));
    Ok(view! {
        <button type="button" class="grid w-full min-w-0 cursor-pointer grid-cols-[9px_58px_minmax(0,1fr)_auto] items-center gap-2 rounded-[7px] border-0 bg-transparent px-1 py-[5px] text-left text-[12.5px] hover:bg-bg-4" data-on:click=(pick)>
            <i class=(dot_class(tint))></i>
            <span class="font-mono text-[11px] text-fg-3">(id)</span>
            <span class="truncate text-fg">(title)</span>
            <span class="whitespace-nowrap text-[11.5px] text-fg-3">(note)</span>
        </button>
    })
}

// ------------------------------------------------------------------ tabs

/// The tab names and their words, in order.
pub const TABS: [(&str, &str); 6] = [
    ("needs", "Needs you"),
    ("projects", "Projects"),
    ("team", "Team"),
    ("money", "Money"),
    ("rules", "Rules"),
    ("log", "Log"),
];

/// `name` if it is a tab, else the first tab.
pub fn tab(name: &str) -> &'static str {
    TABS.iter()
        .map(|(tab, _)| *tab)
        .find(|tab| *tab == name)
        .unwrap_or(TABS[0].0)
}

/// The tab bar and its panes, switched by `$tab` without asking the server;
/// `tab` is the one shown before Datastar runs.
#[component]
async fn tabs(
    office: Office,
    extra: Extra,
    tab: String,
    history: usize,
) -> topcoat::Result<impl View> {
    let shown = tab.clone();
    let hidden = move |name: &str| (name != shown).then_some("display: none");
    let needs = facts::needs_you(&office).count();
    let rules = extra.rules.clone();
    let money = (extra.clone(), office.clone());
    Ok(view! {
        <nav class="tabs" role="tablist" aria-label="The studio">
            for (name, label) in TABS {
                <button type="button" class="tab" role="tab" aria-selected=(if name == tab { "true" } else { "false" })
                    data-attr:aria-selected=(format!("$tab == '{name}' ? 'true' : 'false'")) data-on:click=(format!("$tab = '{name}'"))>
                    (label)
                    if name == "needs" && needs > 0 {
                        <span class="count needs">(needs)</span>
                    }
                </button>
            }
        </nav>
        <div class="pane" role="tabpanel" style=(hidden("needs")) data-show="$tab == 'needs'">
            inspector::needs_you(office: office.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("projects")) data-show="$tab == 'projects'">
            projects_pane(office: office.clone(), history: history)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("team")) data-show="$tab == 'team'">
            team_pane(office: office.clone(), extra: extra.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("money")) data-show="$tab == 'money'">
            money_pane(office: money.1, extra: money.0)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("rules")) data-show="$tab == 'rules'">
            rules_pane(rules: rules)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("log")) data-show="$tab == 'log'">
            log_pane(office: office)
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::fixtures::{extra, html_of, html_on, office};
    use super::*;
    use crate::probe::{find, with};

    #[tokio::test]
    async fn a_link_can_open_another_tab() {
        let html = html_on(office(), extra(), "", tab("money")).await;
        let pane = |name: &str| find(&html, "data-show", &format!("$tab == '{name}'"));
        assert!(pane("money").attr_is("role", "tabpanel"), "{html}");
        assert!(!pane("money").attr_has("style", "display: none"), "{html}");
        assert!(pane("needs").attr_has("style", "display: none"), "{html}");
        assert_eq!(tab("nonsense"), "needs");
    }

    #[tokio::test]
    async fn the_tabs_switch_without_the_server() {
        let html = html_of(office(), extra(), "").await;
        for (name, label) in TABS {
            let tab = find(&html, "data-on:click", &format!("$tab = '{name}'"));
            assert!(tab.attr_is("role", "tab"), "{name}: {html}");
            assert!(tab.text().starts_with(label), "{name}: {html}");
            // Each tab has its pane, which the tab shows.
            find(&html, "data-show", &format!("$tab == '{name}'"));
        }
        // The one that needs the owner shows first, with how many.
        let needs = find(&html, "data-on:click", "$tab = 'needs'");
        assert_eq!(needs.text(), "Needs you 2", "{html}");
        assert!(needs.attr_is("aria-selected", "true"), "{html}");
        let team = find(&html, "data-on:click", "$tab = 'team'");
        assert_eq!(team.text(), "Team", "{html}");
    }

    #[tokio::test]
    async fn needs_you_answers_an_ask_in_place() {
        let html = html_of(office(), extra(), "").await;
        assert!(html.contains("@post('/asks/a1'"), "{html}");
        for word in [
            "Allow",
            "Deny",
            "Always allow",
            "Note for the agent",
            "Ready to land",
        ] {
            assert!(html.contains(word), "{word}: {html}");
        }
        assert!(html.contains("@post('/tickets/SHOP-5/land')"), "{html}");
        assert!(html.contains("@post('/tickets/API-2/resume')"), "{html}");
    }

    #[tokio::test]
    async fn a_picked_ticket_gets_its_panel_with_a_way_back() {
        let html = html_of(office(), extra(), "tickets/SHOP-5").await;
        assert!(html.contains("$sel = ''; @get('/office/panel')"), "{html}");
        assert!(with(&html, "role", "tablist").is_empty(), "{html}");
    }

    #[tokio::test]
    async fn without_extra_nothing_is_made_up() {
        let empty = Extra::default();
        let all = html_of(office(), empty.clone(), "").await;
        // Only what the office knows: no held, left, per-hire or per-day
        // figures, no rules.
        for made_up in [
            "held by live runs",
            " left",
            "By hire",
            "By project",
            "14 days",
            "$4.40",
        ] {
            assert!(!all.contains(made_up), "{made_up}: {all}");
        }
        assert!(all.contains("$6.40"), "{all}");
        assert!(all.contains("No rules yet."), "{all}");
        for element in Element::ALL {
            let html = html_of(
                office(),
                empty.clone(),
                &format!("elements/{}", element.key()),
            )
            .await;
            assert!(
                html.contains("$sel = ''; @get('/office/panel')"),
                "{}: {html}",
                element.key()
            );
            for made_up in ["held by live runs", "Passed", "3h ago", "25 tickets"] {
                assert!(
                    !html.contains(made_up),
                    "{}: {made_up}: {html}",
                    element.key()
                );
            }
        }
        let trophies = html_of(office(), empty.clone(), "elements/trophies").await;
        assert!(trophies.contains("No trophies to show yet."), "{trophies}");
        let racks = html_of(office(), empty, "elements/racks").await;
        assert!(racks.contains("No project checks to show."), "{racks}");
    }

    #[tokio::test]
    async fn a_name_cannot_break_out_of_the_page() {
        let mut office = office();
        office.status.team[0].id = "d<b>ev".into();
        let html = html_of(office, extra(), "").await;
        assert!(!html.contains("<b>ev"), "{html}");
    }
}
