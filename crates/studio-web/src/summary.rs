//! Short, readable summaries of what a tool call does, shared by the ask
//! cards, the run log and the board's "now" line.

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

/// "Bash: cargo test --locked", cut for a single line.
pub fn tool_line(tool: &str, input: &Value, max: usize) -> String {
    let arg = tool_arg(tool, input);
    if arg.is_empty() {
        tool.to_string()
    } else {
        truncate(&format!("{tool}: {arg}"), max)
    }
}

/// What a run log says the agent did last: its latest tool call or message.
pub fn latest_activity(log: &str, max: usize) -> Option<String> {
    log.lines().rev().find_map(|raw| {
        let entry: Value = serde_json::from_str(raw).ok()?;
        let e = &entry["event"];
        match e["type"].as_str()? {
            "tool_call" => Some(tool_line(e["name"].as_str()?, &e["input"], max)),
            "text" => {
                let text = e["text"].as_str()?.split_whitespace().collect::<Vec<_>>();
                (!text.is_empty()).then(|| truncate(&text.join(" "), max))
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_in_a_worktree_are_relative() {
        let inside = "/p/game/.studio/worktrees/GAME-3/src/hud.rs";
        assert_eq!(project_path(inside), ("src/hud.rs".into(), false));
        assert_eq!(project_path("src/a.rs"), ("src/a.rs".into(), false));
        assert_eq!(project_path("/etc/hosts"), ("/etc/hosts".into(), true));
    }

    #[test]
    fn tool_calls_read_as_one_line() {
        let bash = json!({"command": "cargo  test\n--locked"});
        assert_eq!(tool_line("Bash", &bash, 80), "Bash: cargo test --locked");
        let edit = json!({"file_path": "/p/.studio/worktrees/T-1/src/hud.rs"});
        assert_eq!(tool_line("Edit", &edit, 80), "Edit: src/hud.rs");
        assert_eq!(tool_line("Bash", &bash, 8), "Bash: ca…");
        assert_eq!(tool_line("Task", &Value::Null, 80), "Task");
    }

    #[test]
    fn latest_activity_is_the_last_call_or_message() {
        let log = [
            r#"{"event":{"type":"text","text":"Looking around"}}"#,
            r#"{"event":{"type":"tool_call","name":"Bash","input":{"command":"ls"}}}"#,
            r#"{"event":{"type":"tool_result","output":"x"}}"#,
        ]
        .join("\n");
        assert_eq!(latest_activity(&log, 60).as_deref(), Some("Bash: ls"));
        assert_eq!(latest_activity("", 60), None);
    }
}
