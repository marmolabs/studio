#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod draft;
pub mod frontmatter;
pub mod loader;
pub mod pack;
pub mod paths;
pub mod project;
pub mod propose;
pub mod settings;
pub mod starter;
pub mod writer;

pub use config::{ConfigError, StudioConfig};
pub use loader::{
    LoadedRules, Problem, check_hire, check_rule, load_hires, load_pack_hires, load_rules,
    parse_studio_rule,
};
pub use paths::StudioPaths;
pub use project::{Gate, ProjectConfig, ProjectConfigError, matches_any};
pub use propose::propose_rules;
pub use settings::{Setting, SettingsError, set_values};
pub use starter::{AddedPack, AddedProject, NewProject, StarterError, add_pack, add_project, init};
pub use writer::write_rule;
