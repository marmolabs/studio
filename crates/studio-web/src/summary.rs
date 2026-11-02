//! Short, readable summaries of what a tool call does, shared by the ask
//! cards, the run log and `words`.

use serde_json::Value;

/// Where an agent's worktree lives under its project.
const WORKTREES: &str = ".studio/worktrees/";

/// `text` cut to at most `max` characters, with an ellipsis if it was cut.
pub fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// A file path as the agent sees it: relative when it is in the ticket's
/// worktree, and whether it is somewhere else.
pub fn project_path(path: &str) -> (String, bool) {
    if let Some((_, rest)) = path.split_once(WORKTREES) {
        // Drop the worktree's own folder name.
        let relative = rest.split_once('/').map_or("", |(_, file)| file);
        return (relative.to_string(), false);
    }
    (path.to_string(), path.starts_with('/'))
}

/// The path a file tool works on, if the input has one.
pub fn file_path(input: &Value) -> Option<&str> {
    ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|key| input[key].as_str())
}

/// One line saying what a tool call does: the command, the path, or the
/// pattern searched for. Empty when the input says nothing useful.
pub fn tool_arg(tool: &str, input: &Value) -> String {
    let text = match tool {
        "Bash" => input["command"].as_str().map(str::to_string),
        "Glob" | "Grep" => input["pattern"].as_str().map(str::to_string),
        "WebFetch" => input["url"].as_str().map(str::to_string),
        "WebSearch" => input["query"].as_str().map(str::to_string),
        _ => file_path(input).map(|p| project_path(p).0),
    };
    match text {
        Some(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        None if input.is_null() => String::new(),
        None => input.to_string(),
    }
}

/// Who is judging a ticket in review: its reviewer, or a gate's critic.
#[derive(Clone, Debug, PartialEq)]
pub struct Judge {
    pub hire: String,
    pub model: String,
    /// The gate, for a critic.
    pub gate: Option<String>,
    /// When the judge started, in epoch milliseconds.
    pub at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_in_a_worktree_are_relative() {
        let inside = "/p/game/.studio/worktrees/GAME-3/src/hud.rs";
        assert_eq!(project_path(inside), ("src/hud.rs".into(), false));
        assert_eq!(project_path("src/a.rs"), ("src/a.rs".into(), false));
        assert_eq!(project_path("/etc/hosts"), ("/etc/hosts".into(), true));
    }
}
