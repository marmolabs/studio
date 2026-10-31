use serde::de::DeserializeOwned;
use std::path::Path;
use studiod::api::{Estimate, Request};

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

/// Quotes and finish times for open tickets; empty if studiod can't say.
pub async fn estimates(socket: &Path) -> Vec<Estimate> {
    call(socket, Request::Estimates).await.unwrap_or_default()
}

pub fn error(message: impl Into<String>) -> topcoat::Error {
    std::io::Error::other(message.into()).into()
}

/// One event of a ticket's history, from `Request::Events`.
#[derive(Debug, serde::Deserialize)]
pub struct Stamped {
    pub at_ms: u64,
    pub event: studio_core::event::Event,
}

/// The parts of `Request::Status` the pages use.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Status {
    #[serde(default)]
    pub asks: u64,
    #[serde(default)]
    pub running: u64,
    #[serde(default)]
    pub daily_usd: f64,
    #[serde(default)]
    pub spent_24h: f64,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub hires: Vec<String>,
}
