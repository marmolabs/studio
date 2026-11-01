//! Settings the owner changes at run time: open or closed, desks, limits, projects.

use super::{Studio, StudioError, lock, other};
use crate::api::{MAX_RUNS_LIMIT, Response};
use serde_json::json;
use std::sync::Arc;
use studio_config::{Setting, StudioConfig, set_values};

/// `path` with a leading `~` turned into the home folder.
fn expand_home(path: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match (path.strip_prefix("~/"), path, home) {
        (Some(rest), _, Some(home)) => home.join(rest),
        (None, "~", Some(home)) => home,
        _ => std::path::PathBuf::from(path),
    }
}

impl Studio {
    /// The file whose presence means the owner closed the office.
    fn closed_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("closed")
    }

    /// Whether the owner closed the office: nothing new starts.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed_path().exists()
    }

    pub(super) fn set_open(&self, open: bool) -> Result<Response, StudioError> {
        let path = self.closed_path();
        if open {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(other(format!("could not reopen the office: {e}"))),
            }
            // Queued work may start now.
            self.wake.notify_one();
        } else {
            std::fs::create_dir_all(&self.paths.data_dir)
                .and_then(|()| std::fs::write(&path, "closed by the owner\n"))
                .map_err(|e| other(format!("could not close the office: {e}")))?;
        }
        Ok(Response::ok(json!({ "closed": self.is_closed() })))
    }

    /// Writes studio.toml values, then reads the file back so they apply now.
    fn change_settings(
        &self,
        changes: &[(&str, &str, Setting)],
    ) -> Result<Arc<StudioConfig>, StudioError> {
        let _writing = lock(&self.settings);
        set_values(&self.paths, changes).map_err(|e| other(e.to_string()))?;
        Ok(self.reload_config())
    }

    /// Adds a repository as a project, as `studio add` does, and applies it
    /// at once. With `check`, only finds out what would be added.
    pub(super) fn add_project(
        &self,
        path: &str,
        name: Option<String>,
        branch: Option<String>,
        daily_usd: Option<f64>,
        pack: Option<String>,
        check: bool,
    ) -> Result<Response, StudioError> {
        let path = expand_home(path.trim());
        if !path.is_dir() {
            return Err(other(format!("{} is not a folder", path.display())));
        }
        let repo = studio_git::Repo::open(&path)?;
        let branch = match branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
        {
            Some(branch) => branch,
            None => repo
                .current_branch()?
                .ok_or_else(|| other("that repository has no branch checked out; name one"))?,
        };
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let project = studio_config::NewProject {
            path: repo.root(),
            name: name.as_deref(),
            integration_branch: Some(&branch),
            daily_usd,
            pack: pack.as_deref(),
        };
        let (name, path, checks) = if check {
            let plan = studio_config::check_project(&self.paths, &project)
                .map_err(|e| other(e.to_string()))?;
            (plan.name, plan.path, plan.new_checks)
        } else {
            let _writing = lock(&self.settings);
            let added = studio_config::add_project(&self.paths, project)
                .map_err(|e| other(e.to_string()))?;
            self.reload_config();
            (added.name, added.path, added.created_checks)
        };
        if !check {
            self.wake.notify_one();
        }
        Ok(Response::ok(json!({
            "name": name,
            "path": path,
            "branch": branch,
            "checks": checks,
        })))
    }

    pub(super) fn set_max_runs(&self, max_runs: u64) -> Result<Response, StudioError> {
        if !(1..=MAX_RUNS_LIMIT).contains(&max_runs) {
            return Err(other(format!(
                "max_runs must be from 1 to {MAX_RUNS_LIMIT}, not {max_runs}"
            )));
        }
        let config =
            self.change_settings(&[("concurrency", "max_runs", Setting::Whole(max_runs))])?;
        // More desks may let queued work start.
        self.wake.notify_one();
        Ok(Response::ok(
            json!({ "max_runs": config.concurrency.max_runs }),
        ))
    }

    pub(super) fn set_limits(
        &self,
        daily_usd: Option<f64>,
        ticket_usd: Option<f64>,
    ) -> Result<Response, StudioError> {
        let mut changes = Vec::new();
        for (key, value) in [("daily_usd", daily_usd), ("ticket_usd", ticket_usd)] {
            let Some(usd) = value else {
                continue;
            };
            if !usd.is_finite() || usd <= 0.0 {
                return Err(other(format!("{key} must be more than zero, not {usd}")));
            }
            changes.push(("budget", key, Setting::Decimal(usd)));
        }
        if changes.is_empty() {
            return Err(other("give a daily_usd or a ticket_usd to change"));
        }
        let config = self.change_settings(&changes)?;
        self.wake.notify_one();
        Ok(Response::ok(json!({
            "daily_usd": config.budget.daily_usd,
            "ticket_usd": config.budget.ticket_usd,
        })))
    }

    /// How many agents work at once: `max_runs`, up to `MAX_RUNS_LIMIT`.
    pub(super) fn desks(&self) -> u64 {
        u64::from(self.config().concurrency.max_runs).min(MAX_RUNS_LIMIT)
    }
}
