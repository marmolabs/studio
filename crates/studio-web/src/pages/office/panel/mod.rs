//! The office's side panel: what waits for you, every project, the company,
//! money and the log as tabs; a project's own page; and the panel of
//! whatever the owner picked on the floor.

use serde::Deserialize;
use std::collections::BTreeMap;
use studiod::api::{DaySpend, ProjectCheck, Request, RuleFile, Trophy};
use topcoat::view::{BoxView, Child, View, ViewExt, component, view};

mod company;
mod elements;
#[cfg(test)]
mod fixtures;
mod log;
mod money;
mod needs;
mod page;
mod projects;
mod queues;
mod release;
mod rules;
mod standing;
mod team;

use super::facts::{self, dot_class};
use super::inspector::{self, Detail};
use super::model::{Element, Phase, Role};
use super::{Office, key, select};
use crate::{AppConfig, client, ui};
use company::company_pane;
use elements::element_view;
use log::log_pane;
use money::money_pane;
use needs::needs_pane;
use page::project_page;
use projects::projects_pane;
pub use projects::{HISTORY_STEP, history_rows};
use standing::Person;

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
    let project = sel
        .strip_prefix("projects/")
        .and_then(|id| standing::find(&office, id));
    // Boxed, so the panels' futures live on the heap, not the stack.
    let body: BoxView = if let Some(element) = element {
        let inner = element_view(__cx, element, office, extra);
        view! { <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">(inner)</div> }
            .boxed()
    } else if let Some(project) = project {
        let (needs, count) = (needs_count(&office), standing::standings(&office).len());
        view! {
            <div id="inspector" class=(inspector::INSPECTOR) aria-live="polite">
                tab_bar(tab: "projects".to_string(), needs: needs, projects: count, away: true)
                <div class="pane">
                    project_page(office: office, project: project)
                </div>
            </div>
        }
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
    #[into] label: String,
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

/// A card in the panel. (The `.card` class is not used where a card needs
/// its own colours: it outranks the utilities.)
const CARD: &str =
    "flex min-w-0 flex-col gap-2 rounded-xl border border-line bg-bg-3 px-3 py-[11px]";
/// What a card that waits for the owner adds.
const CARD_ATTENTION: &str = "border-attn-line border-l-4 border-l-attn bg-attn-bg";

/// A quiet sentence.
#[component]
async fn quiet(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! { <p class="m-0 text-[13px] leading-5 text-fg-3">(text)</p> })
}

/// A task in a list: its project's dot, id, title and a note. Picking it
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

/// A project's name with its colour's dot.
#[component]
async fn project_tag(name: String, tint: usize) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="inline-flex min-w-0 items-center gap-2 text-[13px] font-semibold">
            <i class=(dot_class(tint))></i>
            <span class="truncate">(name)</span>
        </span>
    })
}

/// The most faces a row shows; the rest are counted.
const FACES: usize = 3;

/// Avatars side by side, none over another so every one's initials read,
/// and "+2" for the ones past the first few.
#[component]
async fn faces(people: Vec<Person>) -> topcoat::Result<impl View> {
    let more = people.len().saturating_sub(FACES);
    let shown: Vec<Person> = people.into_iter().take(FACES).collect();
    Ok(view! {
        <span class="inline-flex items-center gap-1">
            for person in shown { ui::avatar(hire: person.id, name: person.name) }
            if more > 0 {
                <span class="text-[11.5px] font-semibold tabular-nums text-fg-3" title=(format!("and {more} more"))>"+" (more)</span>
            }
        </span>
    })
}

/// The pipeline a project moves through, Planning to Live, with the step it
/// is at lit and the ones before it marked done. A project at none of them (idle
/// or paused) has none lit.
#[component]
async fn pipeline(phase: Phase) -> topcoat::Result<impl View> {
    let at = standing::phase_step(phase);
    let steps: Vec<(&str, &str)> = standing::PIPELINE
        .iter()
        .enumerate()
        .map(|(i, (_, word))| {
            let class = match at {
                Some(now) if i < now => "done",
                Some(now) if i == now => "on",
                _ => "",
            };
            (class, *word)
        })
        .collect();
    // Five equal columns, each a bar over its word, so the row is as wide as
    // its card and no word is cut at any width: the ones passed are green,
    // the one the project is at takes the project's colour.
    Ok(view! {
        <span class="@container grid grid-cols-[repeat(5,minmax(max-content,1fr))] gap-1" role="list" aria-label="Where the project is">
            for (class, word) in steps {
                <span role="listitem" data-step=(class) aria-current=((class == "on").then_some("step")) class="flex flex-col gap-1.5">
                    <i class=(match class {
                        "on" => "block h-1 rounded-full bg-[var(--pj)] shadow-[0_0_0_2px_color-mix(in_srgb,var(--pj)_25%,transparent)]",
                        "done" => "block h-1 rounded-full bg-ok",
                        _ => "block h-1 rounded-full bg-line-2",
                    })></i>
                    <span class=(match class {
                        "on" => "whitespace-nowrap text-[10px] font-semibold text-fg @min-[276px]:text-[11px]",
                        "done" => "whitespace-nowrap text-[10px] text-fg-2 @min-[276px]:text-[11px]",
                        _ => "whitespace-nowrap text-[10px] text-fg-3 @min-[276px]:text-[11px]",
                    })>(word)</span>
                </span>
            }
        </span>
    })
}

/// A phase's pill.
#[component]
async fn phase_pill(phase: Phase) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class=(standing::phase_chip(phase))>
            <i class="size-1.5 rounded-full bg-current"></i>
            (standing::phase_word(phase))
        </span>
    })
}

// ------------------------------------------------------------------ tabs

/// The tab names and their words, in order.
pub const TABS: [(&str, &str); 5] = [
    ("needs", "For you"),
    ("projects", "Projects"),
    ("company", "Company"),
    ("money", "Money"),
    ("log", "Log"),
];

/// `name` if it is a tab, else the first tab. The old Team and Rules tabs
/// are part of Company now.
pub fn tab(name: &str) -> &'static str {
    let name = match name {
        "team" | "rules" => "company",
        other => other,
    };
    TABS.iter()
        .map(|(tab, _)| *tab)
        .find(|tab| *tab == name)
        .unwrap_or(TABS[0].0)
}

/// How many things wait for the owner: asks, held work and releases that
/// wait for a go-ahead.
fn needs_count(office: &Office) -> usize {
    let waiting = facts::needs_you(office);
    waiting.asks.len()
        + waiting.held.len()
        + office
            .projects
            .iter()
            .filter(|p| {
                p.release
                    .as_ref()
                    .is_some_and(|r| r.state == studiod::api::ReleaseState::Waiting)
            })
            .count()
}

/// The tab bar. Over the tabs, a click shows a tab's pane without asking
/// the server; `away` is for a page that stands in for the panes, where it
/// goes back to the tabs and then to the one picked.
#[component]
async fn tab_bar(
    tab: String,
    needs: usize,
    projects: usize,
    away: bool,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <nav class="tabs" role="tablist" aria-label="The studio">
            for (name, label) in TABS {
                <button type="button" class="tab" role="tab" aria-selected=(if name == tab { "true" } else { "false" })
                    data-attr:aria-selected=((!away).then(|| format!("$tab == '{name}' ? 'true' : 'false'")))
                    data-on:click=(if away { format!("$tab = '{name}'; $sel = ''; @get('/office/panel')") } else { format!("$tab = '{name}'") })>
                    (label)
                    if name == "needs" && needs > 0 {
                        <span class="count needs">(needs)</span>
                    }
                    if name == "projects" && projects > 0 {
                        <span class="count">(projects)</span>
                    }
                </button>
            }
        </nav>
    })
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
    let needs = needs_count(&office);
    let count = office.status.projects.len();
    Ok(view! {
        tab_bar(tab: tab, needs: needs, projects: count, away: false)
        <div class="pane" role="tabpanel" style=(hidden("needs")) data-show="$tab == 'needs'">
            needs_pane(office: office.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("projects")) data-show="$tab == 'projects'">
            projects_pane(office: office.clone(), history: history)
        </div>
        <div class="pane" role="tabpanel" style=(hidden("company")) data-show="$tab == 'company'">
            company_pane(office: office.clone(), extra: extra.clone())
        </div>
        <div class="pane" role="tabpanel" style=(hidden("money")) data-show="$tab == 'money'">
            money_pane(office: office.clone(), extra: extra)
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
        // Team and Rules are in Company now.
        assert_eq!(tab("team"), "company");
        assert_eq!(tab("rules"), "company");
    }

    #[tokio::test]
    async fn the_tabs_switch_without_the_server() {
        let html = html_of(office(), extra(), "").await;
        let names: Vec<&str> = TABS.iter().map(|(_, label)| *label).collect();
        assert_eq!(names, ["For you", "Projects", "Company", "Money", "Log"]);
        for (name, label) in TABS {
            let tab = find(&html, "data-on:click", &format!("$tab = '{name}'"));
            assert!(tab.attr_is("role", "tab"), "{name}: {html}");
            assert!(tab.text().starts_with(label), "{name}: {html}");
            // Each tab has its pane, which the tab shows.
            find(&html, "data-show", &format!("$tab == '{name}'"));
        }
        // The one that waits for the owner shows first, with how many: the
        // ask, the held task and the release that waits for a go-ahead.
        let needs = find(&html, "data-on:click", "$tab = 'needs'");
        assert_eq!(needs.text(), "For you 3", "{html}");
        assert!(needs.attr_is("aria-selected", "true"), "{html}");
        let projects = find(&html, "data-on:click", "$tab = 'projects'");
        assert_eq!(projects.text(), "Projects 2", "{html}");
        let company = find(&html, "data-on:click", "$tab = 'company'");
        assert_eq!(company.text(), "Company", "{html}");
        // Team and Rules have no tabs of their own.
        assert!(with(&html, "data-on:click", "$tab = 'team'").is_empty());
        assert!(with(&html, "data-on:click", "$tab = 'rules'").is_empty());
    }

    #[tokio::test]
    async fn a_picked_task_gets_its_panel_with_a_way_back() {
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
            "reserved by running work",
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
        assert!(all.contains("No trophies to show yet."), "{all}");
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
            for made_up in ["reserved by running work", "Passed", "3h ago", "25 tasks"] {
                assert!(
                    !html.contains(made_up),
                    "{}: {made_up}: {html}",
                    element.key()
                );
            }
        }
        let racks = html_of(office(), empty, "elements/racks").await;
        assert!(racks.contains("No project checks to show."), "{racks}");
    }

    #[tokio::test]
    async fn a_few_faces_show_with_a_count_of_the_rest() {
        let cx = topcoat::context::Cx::default();
        let person = |name: &str| Person {
            id: name.to_lowercase(),
            name: name.to_string(),
        };
        let people: Vec<Person> = ["Kit", "Leo", "Mei", "Ola", "Pia"]
            .into_iter()
            .map(person)
            .collect();
        let html = ui::html(&cx, view! { cx => faces(people: people) })
            .await
            .unwrap();
        // Three avatars, none over another, and "+2" for the others.
        assert_eq!(
            with(&html, "class", "av cyan").len()
                + with(&html, "class", "av violet").len()
                + with(&html, "class", "av pink").len()
                + with(&html, "class", "av teal").len()
                + with(&html, "class", "av gold").len(),
            3,
            "{html}"
        );
        assert!(html.contains("+2"), "{html}");
        assert!(!html.contains("-ml-"), "{html}");
        let few = ui::html(&cx, view! { cx => faces(people: vec![person("Kit")]) })
            .await
            .unwrap();
        assert!(!few.contains('+'), "{few}");
    }

    #[tokio::test]
    async fn people_show_by_name_in_the_panel_and_by_id_without_one() {
        let html = html_on(office(), extra(), "", "company").await;
        assert!(html.contains("Nora"), "{html}");
        // dev has no name in its file, so its id stands in.
        assert!(html.contains("<b>dev</b>"), "{html}");
        // A picked hire opens as their name.
        let nora = html_of(office(), extra(), "hires/nora").await;
        assert!(nora.contains("Nora"), "{nora}");
        assert!(!nora.contains(">nora<"), "{nora}");
    }

    #[tokio::test]
    async fn a_name_cannot_break_out_of_the_page() {
        let mut office = office();
        office.status.team[0].id = "d<b>ev".into();
        let html = html_of(office, extra(), "").await;
        assert!(!html.contains("<b>ev"), "{html}");
    }
}
