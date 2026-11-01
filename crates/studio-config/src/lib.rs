#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod draft;
pub mod frontmatter;
mod loader;
mod pack;
mod paths;
mod project;
mod settings;
mod starter;

pub use config::{ConfigError, StudioConfig};
pub use loader::{
    LoadedRules, Problem, check_hire, check_rule, load_hires, load_pack_hires, load_rules,
    parse_studio_rule,
};
pub use paths::StudioPaths;
pub use project::{Gate, ProjectConfig, ProjectConfigError, matches_any};
pub use settings::{Setting, SettingsError, set_values};
pub use starter::{
    AddedPack, AddedProject, NewProject, ProjectPlan, StarterError, add_pack, add_project,
    check_project, init,
};
