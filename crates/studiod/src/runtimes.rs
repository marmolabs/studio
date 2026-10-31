//! The runtimes a studio's hires can name: claude-code, plus one for each
//! provider in studio.toml.

use std::sync::Arc;
use studio_config::StudioConfig;
use studio_runtime::{ClaudeCode, Runtime};

pub struct Built {
    /// By name, `claude-code` first and the providers in name order.
    pub runtimes: Vec<(String, Arc<dyn Runtime>)>,
    /// Providers that could not be built, one line each.
    pub problems: Vec<String>,
}

/// Builds the runtimes for `config`; without one, only claude-code.
pub fn build(config: Option<&StudioConfig>) -> Built {
    let mut built = Built {
        runtimes: vec![("claude-code".to_string(), Arc::new(ClaudeCode::default()))],
        problems: Vec::new(),
    };
    let Some(config) = config else {
        return built;
    };
    let mut providers: Vec<_> = config.providers.iter().collect();
    providers.sort_by(|a, b| a.0.cmp(b.0));
    for (name, provider) in providers {
        // One arm per supported kind.
        let runtime: Arc<dyn Runtime> = match provider.kind.as_str() {
            "claude-code" => Arc::new(ClaudeCode::default()),
            kind => {
                built
                    .problems
                    .push(format!("provider {name}: unsupported runtime kind {kind}"));
                continue;
            }
        };
        built.runtimes.push((name.clone(), runtime));
    }
    built
}
