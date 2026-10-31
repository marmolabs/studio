use serde::de::DeserializeOwned;
use std::path::Path;
use studiod::api::Request;

/// Sends `request` to studiod and decodes the reply's data as `T`.
pub async fn call<T: DeserializeOwned>(socket: &Path, request: Request) -> topcoat::Result<T> {
    let socket = socket.to_path_buf();
    let response = tokio::task::spawn_blocking(move || studiod::api::call(&socket, &request))
        .await
        .map_err(|e| error(format!("studiod call: {e}")))?
        .map_err(|e| error(format!("can't reach studiod: {e}")))?;
    if !response.ok {
        return Err(error(
            response.error.unwrap_or_else(|| "request failed".into()),
        ));
    }
    serde_json::from_value(response.data).map_err(|e| error(format!("bad reply: {e}")))
}

pub fn error(message: impl Into<String>) -> topcoat::Error {
    std::io::Error::other(message.into()).into()
}

/// One event of a ticket's history, from `Request::Events`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Stamped {
    pub at_ms: u64,
    pub event: studio_core::event::Event,
}

/// One event of any ticket, from `Request::RecentEvents` or
/// `Request::Activity`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Happened {
    pub ticket_id: String,
    pub at_ms: u64,
    pub event: studio_core::event::Event,
}

/// The parts of `Request::Status` the pages use.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct Status {
    #[serde(default)]
    pub asks: u64,
    #[serde(default)]
    pub running: u64,
    /// Plan tickets, which the Goals page lists.
    #[serde(default)]
    pub goals: u64,
    /// The owner's rule files.
    #[serde(default)]
    pub rules: u64,
    /// How many tickets are in each state.
    #[serde(default)]
    pub tickets: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub daily_usd: f64,
    #[serde(default)]
    pub spent_24h: f64,
    /// The studio's 24-hour budget can't fund another run, so nothing starts.
    #[serde(default)]
    pub out_of_budget: bool,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub hires: Vec<String>,
    /// How many runs may go at once.
    #[serde(default)]
    pub max_runs: u64,
    #[serde(default)]
    pub team: Vec<Member>,
}

impl Status {
    /// Tickets at work: claimed, working, in checks or in review. A run
    /// paused on an ask holds a slot (`running`) but isn't at work.
    pub fn at_work(&self) -> u64 {
        ["claimed", "working", "checking", "reviewing"]
            .iter()
            .filter_map(|state| self.tickets.get(*state))
            .sum()
    }
}

/// A loaded hire, as `Request::Status` lists it.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct Member {
    pub id: String,
    pub role: String,
    pub model: String,
}
