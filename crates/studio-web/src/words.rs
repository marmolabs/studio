//! How an ask reads, so it says the same on the team pages and the ticket page.

use serde_json::Value;
use studiod::api::AskView;

use crate::pages::ask_card::questions;
use crate::summary::{file_path, project_path};

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
            title: String::new(),
        }
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

        let edit = ask("permission", "Edit", json!({"file_path": "src/a.rs"}));
        assert_eq!(ask_title(&edit), "Edit a file");

        let gate = ask("gate", "design", json!({"rounds": 3}));
        assert_eq!(ask_title(&gate), "Pass the design gate");

        let change = ask(
            "change",
            "commit",
            json!({"message": "Add waves\n\nMore.", "files": []}),
        );
        assert_eq!(ask_head(&change), "Wants to commit a change");

        let question = ask(
            "question",
            "AskUserQuestion",
            json!([{"question": "Which palette?", "options": ["A", "B"]}]),
        );
        assert_eq!(ask_title(&question), "Ask a question");
        assert_eq!(ask_head(&question), "Has a question");
    }
}
