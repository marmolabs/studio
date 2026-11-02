#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod draft;
pub mod frontmatter;
mod loader;
mod pack;
mod paths;
mod project;
mod project_file;
mod settings;
mod starter;

pub use config::{ConfigError, StudioConfig};
pub use loader::{
    LoadedRules, Problem, check_hire, check_rule, load_hires, load_pack_hires, load_rules,
    parse_studio_rule,
};
pub use paths::StudioPaths;
pub use project::{Gate, ProjectConfig, ProjectConfigError, check_glob, matches_any};
pub use project_file::{Checks, with_checks, write_checks};
pub use settings::{Setting, SettingsError, set_project_daily_usd, set_project_lead, set_values};
pub use starter::{
    AddedPack, AddedProject, NewProject, ProjectPlan, StarterError, add_pack, add_project,
    add_project_entry, check_project, init,
};
