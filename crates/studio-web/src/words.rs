//! How an ask and an agent's latest action read, so they say the same on
//! the office, the board, the ticket page and the asks page.

use serde_json::Value;
use studiod::api::AskView;

use crate::pages::ask_card::questions;
use crate::summary::{Action, file_path, project_path, truncate};

/// What an ask wants, in parts every page words the same way.
#[derive(Debug, PartialEq)]
pub struct Wanted {
    /// "run", "edit", "ask".
    pub verb: &'static str,
    /// What it acts on, in general: "a command", "a file", "the art gate".
    pub what: String,
    /// The exact command, path, commit message or question, on one line.
    pub target: Option<String>,
}

/// What an ask of `kind` to use `tool` with `input` wants.
pub fn wanted(kind: &str, tool: &str, input: &Value) -> Wanted {
    let line = |text: &str| {
        let words = text.split_whitespace().collect::<Vec<_>>();
        (!words.is_empty()).then(|| words.join(" "))
    };
    let first = |text: &str| text.lines().find_map(line);
    let want = |verb, what: &str, target| Wanted {
        verb,
        what: what.to_string(),
        target,
    };
    match (kind, tool) {
        ("question", _) => {
            let question = questions(input).into_iter().next();
            want("ask", "a question", question.and_then(|(q, _)| first(&q)))
        }
        ("gate", gate) => want("pass", &format!("the {gate} gate"), None),
        ("change", _) => want(
            "commit",
            "a change",
            input["message"].as_str().and_then(first),
        ),
        (_, "Bash") if input["command"].is_string() => {
            want("run", "a command", input["command"].as_str().and_then(line))
        }
        (_, "Read" | "Write" | "Edit" | "MultiEdit") if file_path(input).is_some() => {
            let verb = match tool {
                "Read" => "read",
                "Write" => "write",
                _ => "edit",
            };
            let path = file_path(input).map(|p| project_path(p).0);
            want(verb, "a file", path)
        }
        (_, tool) => want("use", tool, None),
    }
}

fn of(ask: &AskView) -> Wanted {
    wanted(&ask.kind, &ask.tool, &ask.input)
}

/// What an ask is for, short: "Run a command", "Edit a file", "Pass the art
/// gate". It sits in a bubble over the agent and heads a small card.
pub fn ask_title(ask: &AskView) -> String {
    let Wanted { verb, what, .. } = of(ask);
    let mut chars = verb.chars();
    let first: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    format!("{first}{} {what}", chars.as_str())
}

/// The head of an ask's card: "Wants to run a command", "Has a question".
pub fn ask_head(ask: &AskView) -> String {
    match of(ask) {
        Wanted { verb: "ask", .. } => "Has a question".into(),
        Wanted { verb, what, .. } => format!("Wants to {verb} {what}"),
    }
}

/// An ask in one line: "Wants to run" and the command, cut to `max`, or the
/// question itself.
pub fn ask_line(ask: &AskView, max: usize) -> (String, Option<String>) {
    match of(ask) {
        Wanted {
            verb: "ask",
            target: Some(question),
            ..
        } => (truncate(&question, max), None),
        Wanted { verb: "ask", .. } => ("Has a question".into(), None),
        Wanted {
            verb,
            target: Some(target),
            ..
        } => (format!("Wants to {verb}"), Some(truncate(&target, max))),
        Wanted { verb, what, .. } => (format!("Wants to {verb} {what}"), None),
    }
}

/// An ask in a list of what happened, after the ticket's id: "asked to run
/// a command".
pub fn asked(ask: &AskView) -> String {
    match of(ask) {
        Wanted { verb: "ask", .. } => "asked a question".into(),
        Wanted { verb, what, .. } => format!("asked to {verb} {what}"),
    }
}

/// What a tool call is doing, as the start of a sentence, for the tools
/// whose argument reads well after it.
fn doing(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "Bash" => "Running",
        "Read" => "Reading",
        "Edit" | "MultiEdit" | "NotebookEdit" => "Editing",
        "Write" => "Writing",
        "Grep" | "Glob" => "Searching for",
        "WebFetch" => "Fetching",
        "WebSearch" => "Searching the web for",
        _ => return None,
    })
}

/// An agent's latest action: "Reading" and the path, "Running" and the
/// command, or its own words.
pub fn action_line(action: &Action) -> (String, Option<String>) {
    let Some(tool) = action.tool.as_deref() else {
        return (action.text.clone(), None);
    };
    match doing(tool) {
        Some(words) => {
            let arg = action
                .text
                .strip_prefix(tool)
                .and_then(|t| t.strip_prefix(": "));
            (words.to_string(), arg.map(str::to_string))
        }
        // MCP tools already read as words; other tools' input is JSON, so
        // their name says enough.
        None if tool.starts_with("mcp__") => (action.text.clone(), None),
        None => (format!("Using {tool}"), None),
    }
}

/// A tool's name for a list of steps: the studio's own tools in words ("File
/// ticket"), any other MCP tool by its name ("Render scene"), the rest as
/// they are.
pub fn tool_name(tool: &str) -> String {
    let Some(rest) = tool.strip_prefix("mcp__") else {
        return tool.to_string();
    };
    let name = rest.split_once("__").map_or(rest, |(_, name)| name);
    let words = match name {
        "ask_owner" => "ask you".to_string(),
        name => name.replace('_', " "),
    };
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// `action_line` as one string: "Reading src/a.rs".
pub fn action_text(action: &Action) -> String {
    match action_line(action) {
        (words, Some(arg)) => format!("{words} {arg}"),
        (words, None) => words,
    }
}

/// A running ticket's line: what its agent is doing now, from its state
/// and latest action.
pub fn now_line(kind: &str, action: Option<Action>) -> (String, Option<String>) {
    match action {
        _ if kind == "checking" => ("Running the checks".into(), None),
        Some(action) => action_line(&action),
        None if kind == "reviewing" => ("Under review".into(), None),
        None => ("Starting".into(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ask(kind: &str, tool: &str, input: Value) -> AskView {
        AskView {
            id: "ASK-1".into(),
            ticket: "G-1".into(),
            hire: "dev".into(),
            kind: kind.into(),
            tool: tool.into(),
            input,
            reasons: Vec::new(),
            at_ms: 0,
        }
    }

    #[test]
    fn tools_have_plain_names() {
        assert_eq!(tool_name("mcp__studio__file_ticket"), "File ticket");
        assert_eq!(tool_name("mcp__studio__ask_owner"), "Ask you");
        assert_eq!(tool_name("mcp__blender__render_scene"), "Render scene");
        assert_eq!(tool_name("Bash"), "Bash");
    }

    #[test]
    fn asks_read_the_same_everywhere() {
        let run = ask(
            "permission",
            "Bash",
            json!({"command": "cargo  run\n--example waves"}),
        );
        assert_eq!(ask_title(&run), "Run a command");
        assert_eq!(ask_head(&run), "Wants to run a command");
        assert_eq!(
            ask_line(&run, 80),
            (
                "Wants to run".into(),
                Some("cargo run --example waves".into())
            )
        );
        assert_eq!(asked(&run), "asked to run a command");

        let edit = ask("permission", "Edit", json!({"file_path": "src/a.rs"}));
        assert_eq!(ask_title(&edit), "Edit a file");
        assert_eq!(ask_line(&edit, 80).1.as_deref(), Some("src/a.rs"));

        let gate = ask("gate", "design", json!({"rounds": 3}));
        assert_eq!(ask_title(&gate), "Pass the design gate");
        assert_eq!(
            ask_line(&gate, 80),
            ("Wants to pass the design gate".into(), None)
        );

        let change = ask(
            "change",
            "commit",
            json!({"message": "Add waves\n\nMore.", "files": []}),
        );
        assert_eq!(ask_head(&change), "Wants to commit a change");
        assert_eq!(ask_line(&change, 80).1.as_deref(), Some("Add waves"));

        let question = ask(
            "question",
            "AskUserQuestion",
            json!([{"question": "Which palette?", "options": ["A", "B"]}]),
        );
        assert_eq!(ask_title(&question), "Ask a question");
        assert_eq!(ask_head(&question), "Has a question");
        assert_eq!(ask_line(&question, 80), ("Which palette?".into(), None));
        assert_eq!(asked(&question), "asked a question");

        let other = ask("permission", "WebFetch", json!({"url": "https://x"}));
        assert_eq!(ask_line(&other, 80), ("Wants to use WebFetch".into(), None));
    }

    #[test]
    fn actions_read_as_what_the_agent_is_doing() {
        let action = |tool: Option<&str>, text: &str| Action {
            tool: tool.map(str::to_string),
            text: text.into(),
            at_ms: 0,
        };
        let said = |tool, text| action_text(&action(tool, text));
        assert_eq!(said(Some("Bash"), "Bash: cargo test"), "Running cargo test");
        assert_eq!(said(Some("Read"), "Read: src/a.rs"), "Reading src/a.rs");
        assert_eq!(said(Some("Bash"), "Bash"), "Running");
        assert_eq!(said(Some("Task"), "Task: {}"), "Using Task");
        assert_eq!(
            said(Some("mcp__studio__read_rules"), "Reading the rules"),
            "Reading the rules"
        );
        assert_eq!(said(None, "Looking around"), "Looking around");
        assert_eq!(
            now_line("checking", Some(action(None, "x"))),
            ("Running the checks".into(), None)
        );
        assert_eq!(now_line("reviewing", None), ("Under review".into(), None));
    }
}
