//! Where each project stands: its phase, its milestone and who is working on
//! it, read from the office's tasks and the releases studiod reports. The
//! Projects tab, the project's page and the company chart all start here.

use studio_core::ticket::Ticket;
use studiod::api::{ReleaseState, ReleaseView};

use crate::pages::office::facts;
use crate::pages::office::floor::{Room, room};
use crate::pages::office::model::Phase;
use crate::pages::office::{Office, key, safe};
use crate::pages::timeline::relative;

/// The phases a project moves through, in order, with their words.
pub(super) const PIPELINE: [(Phase, &str); 5] = [
    (Phase::Planning, "Planning"),
    (Phase::Building, "Building"),
    (Phase::Testing, "Testing"),
    (Phase::Releasing, "Releasing"),
    (Phase::Live, "Live"),
];

/// A phase in a word.
pub(super) fn phase_word(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Idle",
        Phase::Planning => "Planning",
        Phase::Building => "Building",
        Phase::Testing => "Testing",
        Phase::Releasing => "Releasing",
        Phase::Live => "Live",
        Phase::Paused => "Paused",
    }
}

/// A phase's place in `PIPELINE`; idle and paused projects are at none.
pub(super) fn phase_step(phase: Phase) -> Option<usize> {
    PIPELINE.iter().position(|(step, _)| *step == phase)
}

/// The classes of a phase's pill.
pub(super) fn phase_chip(phase: Phase) -> &'static str {
    match phase {
        Phase::Building | Phase::Testing => "chip run",
        Phase::Releasing => "chip needs",
        Phase::Live => "chip ok",
        Phase::Idle | Phase::Planning | Phase::Paused => "chip",
    }
}

/// The goal a project is working towards.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Mile {
    /// Its selection key, `goals/<plan id>`.
    pub(super) key: String,
    pub(super) title: String,
    pub(super) version: Option<String>,
    /// The tasks its plan filed, cancelled ones left out.
    pub(super) tasks: Vec<Ticket>,
}

impl Mile {
    pub(super) fn done(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| t.state.kind() == "landed")
            .count()
    }

    pub(super) fn total(&self) -> usize {
        self.tasks.len()
    }

    /// "v0.4 Boss fight", or just the title when it has no version.
    pub(super) fn name(&self) -> String {
        match &self.version {
            Some(v) => format!("v{} {}", v.trim_start_matches('v'), self.title),
            None => self.title.clone(),
        }
    }

    /// `name`, short enough for a heading: the title cut to a few words.
    pub(super) fn short(&self) -> String {
        match &self.version {
            Some(v) => format!(
                "v{} {}",
                v.trim_start_matches('v'),
                short_title(&self.title)
            ),
            None => short_title(&self.title),
        }
    }
}

/// The most words and letters a short title keeps.
const TITLE_WORDS: usize = 5;
const TITLE_LETTERS: usize = 36;

/// The first line of `text` cut to its first few words, with an ellipsis
/// when something was left out: "Give the store its look: logo, product
/// cards, launch email" is "Give the store its look…".
pub(super) fn short_title(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out = String::new();
    for (words, word) in line.split_whitespace().enumerate() {
        let next = out.chars().count() + usize::from(!out.is_empty()) + word.chars().count();
        if words == TITLE_WORDS || (words > 0 && next > TITLE_LETTERS) {
            let kept = out.trim_end_matches([':', ',', ';', '-', '.']).len();
            out.truncate(kept);
            out.push('…');
            return out;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// A hire the panels name: its id, and what to call it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Person {
    pub(super) id: String,
    /// The name its file gives; the id when it gives none.
    pub(super) name: String,
}

impl Person {
    pub(super) fn of(office: &Office, id: &str) -> Person {
        Person {
            id: id.to_string(),
            name: office.name_of(id),
        }
    }
}

/// One project, as the panels show it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Standing {
    pub(super) name: String,
    /// Its colour index.
    pub(super) tint: usize,
    pub(super) phase: Phase,
    pub(super) milestone: Option<Mile>,
    /// Who leads it.
    pub(super) lead: Option<Person>,
    /// Whether it has release commands.
    pub(super) releasable: bool,
    /// Its folder, empty when studiod doesn't say.
    pub(super) path: String,
    /// The release studiod reports as the current one; see `under_way`.
    pub(super) release: Option<ReleaseView>,
    /// The latest release that published.
    pub(super) last: Option<ReleaseView>,
    /// Everyone working on it now, the lead left out.
    pub(super) team: Vec<Person>,
    /// Things in it that wait for the owner.
    pub(super) needs: usize,
}

impl Standing {
    /// The release that is checking, waiting or publishing: nothing else
    /// can start while it is.
    pub(super) fn under_way(&self) -> Option<&ReleaseView> {
        self.release.as_ref().filter(|r| {
            matches!(
                r.state,
                ReleaseState::Checking | ReleaseState::Waiting | ReleaseState::Publishing
            )
        })
    }

    /// The release that waits for the owner's go-ahead.
    pub(super) fn waiting(&self) -> Option<&ReleaseView> {
        self.release
            .as_ref()
            .filter(|r| r.state == ReleaseState::Waiting)
    }

    /// What picking the project selects.
    pub(super) fn key(&self) -> String {
        key("projects", &self.name)
    }
}

/// Every project, in the order of the status.
pub(super) fn standings(office: &Office) -> Vec<Standing> {
    office
        .status
        .projects
        .iter()
        .map(|name| standing(office, name))
        .collect()
}

/// The project whose selection id is `id` (`projects/<id>`), if there is one.
pub(super) fn find(office: &Office, id: &str) -> Option<Standing> {
    let name = office.status.projects.iter().find(|p| safe(p) == id)?;
    Some(standing(office, name))
}

/// `name`'s standing.
pub(super) fn standing(office: &Office, name: &str) -> Standing {
    let view = office.projects.iter().find(|p| p.name == name);
    let release = view.and_then(|p| p.release.clone());
    let last = view.and_then(|p| p.last_release.clone());
    let lead = view
        .and_then(|p| p.lead.clone())
        .filter(|l| !l.is_empty())
        .map(|id| Person::of(office, &id));
    let milestone = milestone(office, name);
    Standing {
        name: name.to_string(),
        tint: facts::tint(&office.status.projects, name),
        phase: facts::phase(office, name),
        milestone,
        releasable: view.is_some_and(|p| p.releasable),
        path: view.map(|p| p.path.clone()).unwrap_or_default(),
        release,
        last,
        team: team(office, name, lead.as_ref().map(|l| l.id.as_str())),
        needs: facts::project_needs(office, name),
        lead,
    }
}

/// The open goal a project is working towards: one with a version first,
/// else the oldest.
fn milestone(office: &Office, project: &str) -> Option<Mile> {
    let goal = facts::milestone(office, project)?;
    Some(Mile {
        key: key("goals", &goal.plan.id.to_string()),
        title: goal.name(),
        version: goal.plan.version.clone().filter(|v| !v.is_empty()),
        tasks: goal.filed.clone(),
    })
}

/// Hires at work on `project` now: at a desk, at the planning table or
/// reading a change in review. The lead is left out.
fn team(office: &Office, project: &str, lead: Option<&str>) -> Vec<Person> {
    let mut hires: Vec<String> = Vec::new();
    for ticket in office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project)
    {
        let hire = match room(ticket) {
            Room::Desk | Room::Planning => Some(ticket.hire.to_string()),
            Room::Review => office
                .judges
                .get(&ticket.id.to_string())
                .map(|j| j.hire.clone()),
            _ => None,
        };
        if let Some(hire) = hire
            && Some(hire.as_str()) != lead
            && !hires.contains(&hire)
        {
            hires.push(hire);
        }
    }
    hires.iter().map(|id| Person::of(office, id)).collect()
}

/// A release's progress, "4 of 5 steps".
pub(super) fn steps_of(release: &ReleaseView) -> String {
    let done = release.steps.iter().filter(|s| s.done).count();
    format!("{done} of {} steps", release.steps.len())
}

/// When a release last moved: its latest finished step, else its start.
pub(super) fn moved_at(release: &ReleaseView) -> u64 {
    release
        .steps
        .iter()
        .filter(|s| s.done)
        .filter_map(|s| s.at)
        .max()
        .unwrap_or(release.started_at)
}

/// The release line of a project, as the Projects tab words it, with a tone
/// (`wait`, `run`, `bad`, or empty).
pub(super) fn release_line(project: &Standing, now: u64) -> (String, &'static str) {
    if let Some(release) = &project.release {
        let v = release.version.trim_start_matches('v');
        match release.state {
            ReleaseState::Waiting => {
                return (format!("v{v} waits for your OK"), "wait");
            }
            ReleaseState::Checking | ReleaseState::Publishing => {
                return (format!("Releasing v{v} · {}", steps_of(release)), "run");
            }
            ReleaseState::Failed => return (format!("The release of v{v} failed"), "bad"),
            ReleaseState::Released | ReleaseState::Cancelled => {}
        }
    }
    match &project.last {
        Some(last) => {
            let at = last.finished_at.unwrap_or(last.started_at);
            (
                format!(
                    "Last release v{}, {}",
                    last.version.trim_start_matches('v'),
                    relative(now, at)
                ),
                "",
            )
        }
        None => ("Never released".to_string(), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::panel::fixtures::{office, project_view as view, release, ticket};
    use serde_json::json;
    use studiod::api::ProjectView;

    #[test]
    fn a_milestone_with_a_version_is_named_by_it() {
        let office = office();
        let shop = standing(&office, "shop");
        let mile = shop.milestone.unwrap();
        assert_eq!(mile.name(), "v0.4 Checkout flow");
        assert_eq!((mile.done(), mile.total()), (1, 3));
        assert_eq!(mile.key, "goals/SHOP-1");
        assert_eq!(standing(&office, "api").milestone, None);
    }

    #[test]
    fn a_project_is_where_its_work_and_releases_put_it() {
        let mut office = office();
        office.projects = vec![view("shop", None, None), view("api", None, None)];
        // Tasks still to do or at a desk: building.
        assert_eq!(standing(&office, "shop").phase, Phase::Building);
        // Everything left is being checked: testing.
        office.tickets[2] = ticket(
            "SHOP-3",
            "shop",
            json!("checking"),
            json!({"parent": "SHOP-1"}),
        );
        office.tickets[3] = ticket(
            "SHOP-4",
            "shop",
            json!("ready"),
            json!({"parent": "SHOP-1"}),
        );
        assert_eq!(standing(&office, "shop").phase, Phase::Testing);
        // A plan that has not landed is still planning.
        office.tickets[0] = ticket(
            "SHOP-1",
            "shop",
            json!("working"),
            json!({"kind": "plan", "goal": "Checkout flow"}),
        );
        assert_eq!(standing(&office, "shop").phase, Phase::Planning);
        // A release under way beats everything else; one that was cancelled
        // does not.
        office.projects[0].release = Some(release(ReleaseState::Checking, 1));
        assert_eq!(standing(&office, "shop").phase, Phase::Releasing);
        office.projects[0].release = Some(release(ReleaseState::Cancelled, 1));
        assert_eq!(standing(&office, "shop").phase, Phase::Planning);
        // Nothing open and a release out: live. Never released: idle.
        office.tickets.retain(|t| t.project.to_string() != "api");
        assert_eq!(standing(&office, "api").phase, Phase::Idle);
        office.projects[1].last_release = Some(release(ReleaseState::Released, 5));
        assert_eq!(standing(&office, "api").phase, Phase::Live);
        // A closed office pauses the work; a live project stays live.
        office.status.closed = true;
        assert_eq!(standing(&office, "shop").phase, Phase::Paused);
        assert_eq!(standing(&office, "api").phase, Phase::Live);
    }

    #[test]
    fn the_pipeline_lights_one_step() {
        assert_eq!(phase_step(Phase::Planning), Some(0));
        assert_eq!(phase_step(Phase::Releasing), Some(3));
        assert_eq!(phase_step(Phase::Live), Some(4));
        assert_eq!(phase_step(Phase::Idle), None);
        assert_eq!(phase_step(Phase::Paused), None);
    }

    #[test]
    fn who_is_working_on_a_project() {
        let mut office = office();
        office.projects = vec![ProjectView {
            lead: Some("dev".into()),
            ..view("shop", None, None)
        }];
        // dev works at a desk of shop but leads it, so it is left out.
        assert!(standing(&office, "shop").team.is_empty());
        office.projects[0].lead = Some("nora".into());
        let shop = standing(&office, "shop");
        let ids: Vec<&str> = shop.team.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["dev"]);
        // People go by the name their file gives, else by their id.
        let lead = shop.lead.unwrap();
        assert_eq!((lead.id.as_str(), lead.name.as_str()), ("nora", "Nora"));
        assert_eq!(shop.team[0].name, "dev");
    }

    #[test]
    fn a_long_goal_gets_a_short_title() {
        assert_eq!(short_title("Checkout flow"), "Checkout flow");
        assert_eq!(
            short_title("Give the store its look: logo, product cards, launch email"),
            "Give the store its look…"
        );
        // Only the first line counts, and a long run of words stops early.
        assert_eq!(short_title("Fix the cart\nand the footer"), "Fix the cart");
        assert_eq!(
            short_title("Reconcile ledgers nightly automatically somehow"),
            "Reconcile ledgers nightly…"
        );
        assert_eq!(short_title(""), "");
        let mile = Mile {
            key: String::new(),
            title: "Give the store its look: logo, product cards, launch email".into(),
            version: Some("0.4".into()),
            tasks: vec![],
        };
        assert_eq!(mile.short(), "v0.4 Give the store its look…");
    }

    #[test]
    fn the_release_line_says_where_the_release_is() {
        let mut office = office();
        let now = office.now;
        office.projects = vec![view(
            "shop",
            Some(release(ReleaseState::Waiting, 4)),
            Some(release(ReleaseState::Released, 5)),
        )];
        let shop = standing(&office, "shop");
        assert_eq!(
            release_line(&shop, now),
            ("v1.2 waits for your OK".into(), "wait")
        );
        let mut checking = shop.clone();
        checking.release = Some(release(ReleaseState::Checking, 1));
        assert_eq!(
            release_line(&checking, now),
            ("Releasing v1.2 · 1 of 5 steps".into(), "run")
        );
        let mut done = shop.clone();
        done.release = None;
        let mut last = release(ReleaseState::Released, 5);
        last.finished_at = Some(now - 4 * 24 * 3_600_000);
        done.last = Some(last);
        assert_eq!(
            release_line(&done, now),
            ("Last release v1.2, 4d ago".into(), "")
        );
        done.last = None;
        assert_eq!(release_line(&done, now).0, "Never released");
    }
}
