//! Short, readable summaries of what a tool call does, shared by the ask
//! cards, the run log and `words`.

use serde_json::Value;
use studiod::api::StepView;

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

/// What a call to an MCP tool does, in words: the studio's own tools say
/// what they are for ("Filing a ticket: Add undo"), any other tool only its
/// name, never its JSON input. `None` for a tool that isn't an MCP tool.
pub fn mcp_words(tool: &str, input: &Value) -> Option<String> {
    let rest = tool.strip_prefix("mcp__")?;
    let (server, name) = rest.split_once("__").unwrap_or(("", rest));
    let text = |key: &str| {
        let words = input[key].as_str()?.split_whitespace().collect::<Vec<_>>();
        (!words.is_empty()).then(|| words.join(" "))
    };
    let with = |words: &str, what: Option<String>| match what {
        Some(what) => format!("{words}: {what}"),
        None => words.to_string(),
    };
    Some(match (server, name) {
        ("studio", "file_ticket") => with("Filing a ticket", text("title")),
        ("studio", "read_rules") => "Reading the rules".into(),
        ("studio", "load_skill") => match text("name") {
            Some(skill) => format!("Loading the {skill} skill"),
            None => "Listing the skills".into(),
        },
        ("studio", "ask_owner") => with("Asking you", text("question")),
        _ => format!("Using {}", name.replace('_', " ")),
    })
}

/// "Bash: cargo test --locked", cut for a single line.
pub fn tool_line(tool: &str, input: &Value, max: usize) -> String {
    if let Some(words) = mcp_words(tool, input) {
        return truncate(&words, max);
    }
    let arg = tool_arg(tool, input);
    if arg.is_empty() {
        tool.to_string()
    } else {
        truncate(&format!("{tool}: {arg}"), max)
    }
}

/// What an agent did last: a tool call or a message.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    /// The tool's name, or `None` for a message.
    pub tool: Option<String>,
    /// One line: "Bash: cargo test", or the message cut to fit.
    pub text: String,
    /// When it was logged, in epoch milliseconds; 0 for an old log without times.
    pub at_ms: u64,
}

impl Action {
    /// What a step of a live run says, cut to `max` characters.
    pub fn of(step: &StepView, max: usize) -> Action {
        match &step.tool {
            Some(tool) => Action {
                tool: Some(tool.clone()),
                text: tool_line(tool, &step.input, max),
                at_ms: step.at,
            },
            None => Action {
                tool: None,
                text: truncate(&step.text, max),
                at_ms: step.at,
            },
        }
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

impl Judge {
    /// What the judge did last: `latest`, if it came after the judge started.
    pub fn action(&self, latest: Option<Action>) -> Option<Action> {
        latest.filter(|a| a.at_ms >= self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_judge_owns_only_what_came_after_it_started() {
        let judge = Judge {
            hire: "designer".into(),
            model: "sonnet".into(),
            gate: Some("art".into()),
            at: 4,
        };
        let step = |at| StepView {
            tool: Some("Read".into()),
            input: json!({"file_path": "art.png"}),
            text: String::new(),
            at,
        };
        let action = judge.action(Some(Action::of(&step(5), 60))).unwrap();
        assert_eq!(crate::words::action_text(&action), "Reading art.png");
        // What the author did before the judge started is not the judge's.
        assert_eq!(judge.action(Some(Action::of(&step(3), 60))), None);
        assert_eq!(judge.action(None), None);
    }

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
    fn mcp_calls_read_as_words_not_json() {
        let filed = json!({"title": "Add undo and redo", "detail": "{...}"});
        assert_eq!(
            tool_line("mcp__studio__file_ticket", &filed, 80),
            "Filing a ticket: Add undo and redo"
        );
        assert_eq!(
            tool_line("mcp__studio__read_rules", &json!({}), 80),
            "Reading the rules"
        );
        assert_eq!(
            tool_line("mcp__blender__render_scene", &json!({"x": 1}), 80),
            "Using render scene"
        );
        assert_eq!(mcp_words("Bash", &json!({})), None);
    }

    #[test]
    fn an_action_is_a_call_or_a_message_cut_to_fit() {
        let call = StepView {
            tool: Some("Bash".into()),
            input: json!({"command": "cargo  test\n--locked"}),
            text: String::new(),
            at: 5,
        };
        assert_eq!(
            Action::of(&call, 80),
            Action {
                tool: Some("Bash".into()),
                text: "Bash: cargo test --locked".into(),
                at_ms: 5
            }
        );
        let said = StepView {
            tool: None,
            input: Value::Null,
            text: "Reading it".into(),
            at: 9,
        };
        let action = Action::of(&said, 7);
        assert_eq!(
            (action.tool, action.text.as_str(), action.at_ms),
            (None, "Reading…", 9)
        );
    }
}
