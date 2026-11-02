//! The company floor and what the team pages read of the office. `plan`
//! decides who is where, `hq_scene` draws the floor, `hq` draws its pieces
//! and `controls` is what the studio settings and the cards post to.

pub(crate) mod controls;
pub(crate) mod facts;
pub(crate) mod floor;
pub(crate) mod hq;
pub(crate) mod hq_scene;
pub(crate) mod iso;
pub(crate) mod model;
pub(crate) mod pins;
pub(crate) mod plan;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::Ticket;
use studiod::api::{AskView, OfficeView, ProjectView, Request, SetupView, ShipCheck, Standup};
use topcoat::router::{error::redirect_permanent, response::Response, route};

use crate::summary::Judge;
use crate::{AppConfig, client};

/// Everything the team pages and the floor show, read from studiod at once.
#[derive(Clone, Debug, Default)]
pub struct Office {
    pub status: client::Status,
    pub tickets: Vec<Ticket>,
    pub asks: Vec<AskView>,
    /// Every ticket's `OFFICE_KINDS` events over the last 24 hours, oldest
    /// first.
    pub recent: Vec<client::Happened>,
    /// Lines added and removed by each ready ticket, by id.
    pub sizes: HashMap<String, (u64, u64)>,
    /// Who is judging each ticket in review.
    pub judges: HashMap<String, Judge>,
    /// Ready tickets that no reviewer on another model read.
    pub unreviewed: Vec<String>,
    /// Landed tickets whose goal branch is merged.
    pub merged: Vec<String>,
    /// Every project with its lead and its releases, in the studio's order.
    pub projects: Vec<ProjectView>,
    /// A line on what each finished task came to, by id.
    pub results: BTreeMap<String, String>,
    /// Each lead's latest standup, by project.
    pub standups: BTreeMap<String, Standup>,
    /// The checks of each goal whose tasks are all done, by the goal's plan
    /// id.
    pub ship_checks: BTreeMap<String, ShipCheck>,
    /// How each project's setup through its lead stands, by project.
    pub setups: BTreeMap<String, SetupView>,
    pub now: u64,
}

impl Office {
    pub fn ticket(&self, id: &str) -> Option<&Ticket> {
        self.tickets.iter().find(|t| t.id.to_string() == id)
    }

    /// When the latest of `ticket`'s events that `pick` accepts happened.
    pub fn last(&self, ticket: &str, pick: impl Fn(&TicketEvent) -> bool) -> Option<u64> {
        self.recent
            .iter()
            .rev()
            .find(|e| e.ticket_id == ticket && pick(&e.event))
            .map(|e| e.at_ms)
    }

    /// The studio's projects in the order of `studio.toml`.
    pub fn project_names(&self) -> Vec<String> {
        let mut names = self.status.projects.clone();
        for view in &self.projects {
            if !names.contains(&view.name) {
                names.push(view.name.clone());
            }
        }
        names
    }

    /// What studiod says of a project's lead and releases.
    pub fn project(&self, name: &str) -> Option<&ProjectView> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// A hire's role and model, when it is loaded.
    pub fn member(&self, hire: &str) -> Option<&client::Member> {
        self.status.team.iter().find(|m| m.id == hire)
    }

    /// What a hire is called: the name in its file, else its id.
    pub fn name_of(&self, hire: &str) -> String {
        self.member(hire)
            .map(|m| m.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(hire)
            .to_string()
    }
}

/// How long streams share one read of studiod: every open office asks
/// each second, and they all use the one that is at most this old.
pub(crate) const SHARED_FOR: Duration = Duration::from_secs(1);

/// What studiod last said about the office to one socket.
struct Read {
    at: Instant,
    /// Counts up whenever what studiod said changes, apart from its clock.
    version: u64,
    /// The reply with its clock zeroed and its runs left out, to tell whether
    /// the next differs: a run's steps change all the time, and no page shows
    /// them.
    said: OfficeView,
    office: Office,
}

type Slot = Arc<tokio::sync::Mutex<Option<Read>>>;

/// Reads what the office shows, in one request to studiod. Only the status
/// and the tickets must come back; anything else studiod can't say right now
/// is left empty.
pub(crate) async fn gather(config: &AppConfig) -> topcoat::Result<Office> {
    Ok(shared(config, Duration::ZERO).await?.1)
}

/// The office as studiod last said it, if that is under `max_age` old, else
/// as it says now. With the number that changes whenever the office does,
/// apart from the time. Streams of every open page share one read, and
/// those who wait for it use it rather than asking again.
pub(crate) async fn shared(
    config: &AppConfig,
    max_age: Duration,
) -> topcoat::Result<(u64, Office)> {
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
    said.runs.clear();
    let version = match read.as_ref() {
        Some(last) if last.said == said => last.version,
        Some(last) => last.version + 1,
        None => 0,
    };
    let office = office_of(said.clone(), now)?;
    *read = Some(Read {
        at: Instant::now(),
        version,
        said,
        office: office.clone(),
    });
    Ok((version, office))
}

/// `Office` from what studiod said at `now`.
fn office_of(said: OfficeView, now: u64) -> topcoat::Result<Office> {
    let status = serde_json::from_value(said.status)
        .map_err(|e| client::error(format!("bad reply: {e}")))?;
    let recent: Vec<client::Happened> = said.events.into_iter().map(Into::into).collect();
    Ok(Office {
        status,
        tickets: said.tickets,
        asks: said.asks,
        recent,
        sizes: said.sizes.into_iter().collect(),
        judges: said
            .judges
            .into_iter()
            .map(|(id, judge)| (id, judge.into()))
            .collect(),
        unreviewed: said.unreviewed,
        merged: said.merged,
        projects: said.projects,
        results: said.results,
        standups: said.standups,
        ship_checks: said.ship_checks,
        setups: said.setups,
        now,
    })
}

/// `id` kept to characters that are safe inside a quoted Datastar
/// expression and a URL.
pub fn safe(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// A pick on the floor, such as `tickets/SHOP-3`.
pub fn key(kind: &str, id: &str) -> String {
    format!("{kind}/{}", safe(id))
}

/// The Live page is gone; its address leads home.
#[route(GET "/live")]
async fn old_live() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The Board and Goals pages are gone; HQ has what waits on each project.
#[route(GET "/board")]
async fn old_board() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

#[route(GET "/goals")]
async fn old_goals() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The Asks page is gone; HQ has the questions.
#[route(GET "/asks")]
async fn old_asks() -> topcoat::Result<Response> {
    Err(redirect_permanent("/").into())
}

/// The Budget page is gone; the studio settings have the money.
#[route(GET "/budget")]
async fn old_budget() -> topcoat::Result<Response> {
    Err(redirect_permanent("/studio").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_are_safe_to_quote() {
        assert_eq!(key("tickets", "GAME-3"), "tickets/GAME-3");
        assert_eq!(key("hires", "dev'; alert(1)"), "hires/devalert1");
        assert_eq!(safe("my shop/../x"), "myshop..x");
    }
}
