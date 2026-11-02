//! The studio daemon: owns the board, runs tickets in worktrees with their
//! hire's runtime, and waits for the owner where it must.

pub mod api;
mod approver;
mod budget;
mod checks;
pub mod doctor;
mod evidence;
pub mod export;
mod gc;
mod notify;
mod quick;
mod router;
mod ruletest;
mod runlog;
pub mod runtimes;
mod runview;
mod server;
mod studio;
mod today;
mod trophies;
mod watch;
mod worker;

pub use server::serve;
pub use studio::{Studio, StudioError};
pub use watch::CiWatcher;

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
