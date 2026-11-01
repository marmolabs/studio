use crate::paths::StudioPaths;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioConfig {
    pub budget: BudgetConfig,
    pub concurrency: ConcurrencyConfig,
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
    #[serde(default)]
    pub rules: RulesConfig,
    #[serde(default)]
    pub packs: Vec<PackEntry>,
    /// USD per million tokens, by model id, for live spend estimates.
    #[serde(default)]
    pub prices: HashMap<String, ModelPrice>,
    #[serde(default)]
    pub goals: GoalsConfig,
    #[serde(default)]
    pub watchers: WatchersConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
}

/// Desktop notifications sent by studiod.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    #[serde(default)]
    pub desktop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchersConfig {
    /// Minutes between CI checks; values under 2 count as 2.
    #[serde(default = "default_ci_minutes")]
    pub ci_minutes: u64,
}

impl WatchersConfig {
    pub fn ci_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.ci_minutes.max(2) * 60)
    }
}

impl Default for WatchersConfig {
    fn default() -> Self {
        WatchersConfig {
            ci_minutes: default_ci_minutes(),
        }
    }
}

fn default_ci_minutes() -> u64 {
    10
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalsConfig {
    /// Most tickets one ticket's agent may file.
    #[serde(default = "default_max_filed")]
    pub max_filed: u32,
}

impl Default for GoalsConfig {
    fn default() -> Self {
        GoalsConfig {
            max_filed: default_max_filed(),
        }
    }
}

fn default_max_filed() -> u32 {
    8
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    /// Studio-wide cap over any rolling 24 hours.
    pub daily_usd: f64,
    /// Cap for one ticket across all its runs; a hire's own cap can only lower it.
    #[serde(default = "default_ticket_usd")]
    pub ticket_usd: f64,
    /// Optional cap for a single run.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub run_usd: Option<f64>,
    /// A run never starts with less allowance than this.
    #[serde(default = "default_min_run_usd")]
    pub min_run_usd: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        BudgetConfig {
            daily_usd: 0.0,
            ticket_usd: default_ticket_usd(),
            run_usd: None,
            min_run_usd: default_min_run_usd(),
        }
    }
}

fn default_ticket_usd() -> f64 {
    2.0
}

fn default_min_run_usd() -> f64 {
    0.05
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_read_per_mtok: f64,
    #[serde(default)]
    pub cache_write_per_mtok: f64,
}

/// The most agents that work at once, one at each desk the office holds. A
/// larger `max_runs` runs this many.
pub const MAX_RUNS_LIMIT: u64 = 9;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcurrencyConfig {
    pub max_runs: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectEntry {
    pub name: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub integration_branch: Option<String>,
    /// Cap for this project over any rolling 24 hours.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub daily_usd: Option<f64>,
    /// File a fix ticket when CI fails on the integration branch.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub watch_ci: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackEntry {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesConfig {
    #[serde(default)]
    pub disable: Vec<String>,
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("io error reading config: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    /// Well-formed TOML that says something studio.toml no longer allows.
    #[error("{0}")]
    Invalid(String),
}

impl StudioConfig {
    pub fn load(paths: &StudioPaths) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(paths.config_dir.join("studio.toml"))?;
        let mut config = Self::parse(&text)?;
        for project in &mut config.projects {
            project.path = resolve_project_path(&project.path, paths);
        }
        for pack in &mut config.packs {
            pack.path = resolve_project_path(&pack.path, paths);
        }
        Ok(config)
    }

    /// Reads studio.toml's text as written, without resolving any path.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let table: toml::Table = toml::from_str(text)?;
        if table.contains_key("providers") {
            return Err(ConfigError::Invalid(
                "the [providers] table in studio.toml is gone: remove it, and name \
                 `runtime: claude-code` in each hire"
                    .into(),
            ));
        }
        let config: StudioConfig = toml::from_str(text)?;
        if config.concurrency.max_runs == 0 {
            return Err(ConfigError::Invalid(format!(
                "max_runs in studio.toml must be from 1 to {MAX_RUNS_LIMIT}, not 0: \
                 to pause the studio, run `studio close`"
            )));
        }
        Ok(config)
    }
}

fn resolve_project_path(path: &Path, paths: &StudioPaths) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        paths.config_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_config(dir: &Path, text: &str) {
        fs::create_dir_all(dir).unwrap();
        let mut file = fs::File::create(dir.join("studio.toml")).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn loads_minimal_config() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            r#"
[budget]
daily_usd = 25.0

[concurrency]
max_runs = 3
"#,
        );
        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.budget.daily_usd, 25.0);
        assert_eq!(config.concurrency.max_runs, 3);
        assert!(config.projects.is_empty());
        assert_eq!(config.goals.max_filed, 8);
        assert_eq!(config.watchers.ci_minutes, 10);
    }

    #[test]
    fn watchers_can_be_set_and_have_a_floor() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n\n\
             [watchers]\nci_minutes = 1\n\n[[projects]]\nname = \"g\"\npath = \"/g\"\nwatch_ci = true\n",
        );
        let config = StudioConfig::load(&paths).unwrap();
        assert!(config.projects[0].watch_ci);
        assert_eq!(config.watchers.ci_interval().as_secs(), 120);
    }

    #[test]
    fn goals_cap_can_be_set() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n\n[goals]\nmax_filed = 3\n",
        );
        assert_eq!(StudioConfig::load(&paths).unwrap().goals.max_filed, 3);
    }

    #[test]
    fn desktop_notifications_are_off_unless_asked() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        let base = "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n";
        write_config(tmp.path(), base);
        assert!(!StudioConfig::load(&paths).unwrap().notify.desktop);
        write_config(tmp.path(), &format!("{base}\n[notify]\ndesktop = true\n"));
        assert!(StudioConfig::load(&paths).unwrap().notify.desktop);
    }

    #[test]
    fn resolves_relative_project_paths() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            r#"
[budget]
daily_usd = 10.0

[concurrency]
max_runs = 1

[[projects]]
name = "shop"
path = "../code/shop"
integration_branch = "master"

[[packs]]
name = "acme"
path = "packs/acme"
"#,
        );
        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.projects[0].path, tmp.path().join("../code/shop"));
        assert_eq!(config.packs[0].path, tmp.path().join("packs/acme"));
    }

    #[test]
    fn a_providers_table_is_refused_with_a_plain_message() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n\n\
             [providers.work]\nkind = \"claude-code\"\n",
        );
        let message = StudioConfig::load(&paths).unwrap_err().to_string();
        assert!(message.contains("[providers] table"), "{message}");
        assert!(message.contains("runtime: claude-code"), "{message}");
    }

    #[test]
    fn zero_max_runs_is_refused_and_points_to_studio_close() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 0\n",
        );
        let message = StudioConfig::load(&paths).unwrap_err().to_string();
        assert!(message.contains("from 1 to 9"), "{message}");
        assert!(message.contains("studio close"), "{message}");
    }

    #[test]
    fn unknown_key_is_error() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_config(
            tmp.path(),
            r#"
[budget]
daily_usd = 25.0
unknown = 1

[concurrency]
max_runs = 3
"#,
        );
        assert!(StudioConfig::load(&paths).is_err());
    }
}
