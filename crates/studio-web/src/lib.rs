#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![recursion_limit = "256"]

//! A local web UI for a running studio. Like the CLI, it is a client of
//! studiod: every read and action goes through the daemon's socket.

use std::path::PathBuf;
use topcoat::{
    cookie::RouterBuilderCookieExt,
    router::{Router, RouterBuilderDiscoverExt},
};

mod auth;
mod client;
mod diffstat;
mod layout;
mod live;
mod markdown;
mod pages;
mod statics;
mod summary;
mod ui;
mod words;

pub use auth::load_token;

/// What every request needs: where studiod is, and the token that guards
/// the page.
#[derive(Clone)]
pub struct AppConfig {
    pub socket: PathBuf,
    pub data_dir: PathBuf,
    pub token: String,
    /// `http://127.0.0.1:<port>`; POSTs from any other origin are refused.
    pub base_url: String,
}

pub fn router(config: AppConfig) -> Router {
    Router::builder()
        .discover()
        .cookies()
        .app_context(config)
        .build()
}
