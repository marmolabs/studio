//! The runtimes a studio's hires can name. Only claude-code today.

use std::sync::Arc;
use studio_runtime::{ClaudeCode, Runtime};

/// The runtimes by name.
pub fn build() -> Vec<(String, Arc<dyn Runtime>)> {
    vec![("claude-code".to_string(), Arc::new(ClaudeCode::default()))]
}
