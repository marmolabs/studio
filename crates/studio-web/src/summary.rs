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

/// What a run log says the agent did last: its latest tool call or message,
/// and when.
pub fn latest_action(log: &str, max: usize) -> Option<Action> {
    log.lines().rev().find_map(|raw| {
        let entry: Value = serde_json::from_str(raw).ok()?;
        let e = &entry["event"];
        let at_ms = entry["at"].as_u64().unwrap_or(0);
        match e["type"].as_str()? {
            "tool_call" => {
                let tool = e["name"].as_str()?;
                Some(Action {
                    tool: Some(tool.to_string()),
                    text: tool_line(tool, &e["input"], max),
                    at_ms,
                })
            }
            "text" => {
                let text = e["text"].as_str()?.split_whitespace().collect::<Vec<_>>();
                (!text.is_empty()).then(|| Action {
                    tool: None,
                    text: truncate(&text.join(" "), max),
                    at_ms,
                })
            }
            _ => None,
        }
    })
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
    /// What the judge did last, from a run log's tail: only what came after
    /// the judge started.
    pub fn action(&self, log: &str, max: usize) -> Option<Action> {
        latest_action(log, max).filter(|a| a.at_ms >= self.at)
    }
}

/// The judge a run log entry names: a "review by" or "gate <name> by" note.
pub fn judge_of(entry: &Value) -> Option<Judge> {
    let note = entry["note"].as_str()?;
    let (gate, who) = match note.strip_prefix("review by ") {
        Some(who) => (None, who),
        None => {
            let (name, who) = note.strip_prefix("gate ")?.split_once(" by ")?;
            (!name.contains(char::is_whitespace)).then_some((Some(name.to_string()), who))?
        }
    };
    let (hire, model) = who.strip_suffix(')')?.split_once(" (")?;
    if hire.contains(char::is_whitespace) {
        return None;
    }
    Some(Judge {
        hire: hire.to_string(),
        model: model.to_string(),
        gate,
        at: entry["at"].as_u64().unwrap_or(0),
    })
}

/// Scanning a run log back for the judge of the current review: a judge
/// line, or `Some(None)` at the "checks passed" note that began this review,
/// so a judge from an earlier try is never taken for the current one.
pub fn review_line(raw: &str) -> Option<Option<Judge>> {
    let entry: Value = serde_json::from_str(raw).ok()?;
    if entry["note"] == "checks passed" {
        return Some(None);
    }
    judge_of(&entry).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_judge_is_the_latest_reviewer_or_critic() {
        let log = [
            r#"{"at":1,"note":"review by reviewer (opus)"}"#,
            r#"{"at":2,"event":{"type":"tool_call","name":"Read","input":{"file_path":"src/a.rs"}}}"#,
            r#"{"at":3,"note":"review passed"}"#,
            r#"{"at":4,"note":"gate art by designer (sonnet)"}"#,
            r#"{"at":5,"event":{"type":"tool_call","name":"Read","input":{"file_path":"art.png"}}}"#,
        ]
        .join("\n");
        let judge = log.lines().rev().find_map(review_line).flatten().unwrap();
        assert_eq!(
            judge,
            Judge {
                hire: "designer".into(),
                model: "sonnet".into(),
                gate: Some("art".into()),
                at: 4
            }
        );
        let action = judge.action(&log, 60).unwrap();
        assert_eq!(crate::words::action_text(&action), "Reading art.png");
        // What the author did before the judge started is not the judge's.
        let before = log.lines().take(4).collect::<Vec<_>>().join("\n");
        assert_eq!(judge.action(&before, 60), None);
        // A critic's notes that happen to say "by" are not a judge.
        let notes = r#"{"at":1,"note":"gate art failed: blurry by far (see x)"}"#;
        assert_eq!(review_line(notes), None);
        assert_eq!(review_line(""), None);
        // The checks passing begins a new review: no judge yet.
        let next = r#"{"at":9,"note":"checks passed"}"#;
        assert_eq!(review_line(next), Some(None));
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
    fn the_latest_action_is_the_last_call_or_message() {
        let log = [
            r#"{"event":{"type":"text","text":"Looking around"}}"#,
            r#"{"event":{"type":"tool_call","name":"Bash","input":{"command":"ls"}}}"#,
            r#"{"event":{"type":"tool_result","output":"x"}}"#,
        ]
        .join("\n");
        let text = |log: &str| latest_action(log, 60).map(|a| a.text);
        assert_eq!(text(&log).as_deref(), Some("Bash: ls"));
        assert_eq!(text(""), None);
    }

    #[test]
    fn latest_action_says_when_it_began() {
        let log = [
            r#"{"at":5,"event":{"type":"tool_call","name":"Read","input":{"path":"a.rs"}}}"#,
            r#"{"at":9,"event":{"type":"text","text":"  Reading  it "}}"#,
            r#"{"at":12,"note":"checks passed"}"#,
        ]
        .join("\n");
        let action = latest_action(&log, 60).unwrap();
        assert_eq!(
            action,
            Action {
                tool: None,
                text: "Reading it".into(),
                at_ms: 9
            }
        );
        let call = latest_action(log.lines().next().unwrap(), 60).unwrap();
        assert_eq!(
            (call.tool.as_deref(), call.text.as_str(), call.at_ms),
            (Some("Read"), "Read: a.rs", 5)
        );
    }
}
