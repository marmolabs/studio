//! How an ask reads on HQ and in a conversation: a plain question for its
//! title, and why it came to the owner in a few words.

use studiod::api::AskView;

use crate::pages::ask_card::{reasons, summarise};
use crate::summary::truncate;
use crate::words::{self, Wanted};

/// The most characters of a command or path a title shows.
const SHOWN: usize = 60;

/// The question an ask puts to the owner: the words the lead or approver wrote
/// for it, else a plain one made from what it is, such as "Change
/// src/payments/retry.rs?" or "Run cargo run -- --port 8080?".
pub fn title(ask: &AskView) -> String {
    let written = ask.title.trim();
    if !written.is_empty() {
        return written.to_string();
    }
    let Wanted { verb, what, target } = words::wanted(&ask.kind, &ask.tool, &ask.input);
    let target = target.map(|target| truncate(&target, SHOWN));
    match (verb, target) {
        // A question the agent asks is already one.
        ("ask", Some(question)) => question,
        ("ask", None) => "Answer a question?".to_string(),
        ("edit", Some(path)) => format!("Change {path}?"),
        (verb, Some(target)) => format!("{} {target}?", capital(verb)),
        (verb, None) => format!("{} {what}?", capital(verb)),
    }
}

/// Why the ask reached the owner, in a line: the rules' own words, or the
/// lead's, without who said them or the id of the rule (the ask's details have
/// that).
pub fn why(ask: &AskView) -> String {
    reasons(ask, &summarise(ask))
        .iter()
        .map(|reason| plain(reason))
        .filter(|reason| !reason.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One reason as the owner reads it.
fn plain(reason: &str) -> String {
    let mut reason = reason.trim();
    // The lead's and the approver's reasons say who said them, and go on in
    // lower case.
    let mut spoken = false;
    for speaker in ["Lead: ", "Approver: "] {
        if let Some(rest) = reason.strip_prefix(speaker) {
            reason = rest.trim_start();
            spoken = true;
        }
    }
    // A rule's finding ends with the rule's id: "why (project/shop/protect)".
    if let Some((before, id)) = reason.rsplit_once(" (")
        && let Some(id) = id.strip_suffix(')')
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_/.".contains(c))
    {
        reason = before.trim_end();
    }
    if spoken {
        capital(reason)
    } else {
        reason.to_string()
    }
}

/// `text` with its first letter a capital.
fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .chain(chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn ask(kind: &str, tool: &str, input: Value, reasons: &[&str], title: &str) -> AskView {
        serde_json::from_value(json!({
            "id": "a1", "ticket": "S-1", "hire": "dev", "kind": kind, "tool": tool,
            "input": input, "reasons": reasons, "title": title,
        }))
        .unwrap()
    }

    #[test]
    fn a_written_question_is_used_as_it_is() {
        let ask = ask(
            "permission",
            "Bash",
            json!({}),
            &[],
            "  Send the launch email? ",
        );
        assert_eq!(title(&ask), "Send the launch email?");
    }

    #[test]
    fn an_ask_without_a_title_gets_a_plain_question() {
        let edit = ask(
            "permission",
            "Edit",
            json!({"file_path": "src/payments/retry.rs"}),
            &[],
            "",
        );
        assert_eq!(title(&edit), "Change src/payments/retry.rs?");
        let run = ask(
            "permission",
            "Bash",
            json!({"command": "cargo run -- --port 8080"}),
            &[],
            "",
        );
        assert_eq!(title(&run), "Run cargo run -- --port 8080?");
        let write = ask(
            "permission",
            "Write",
            json!({"file_path": "notes.md"}),
            &[],
            "",
        );
        assert_eq!(title(&write), "Write notes.md?");
        let gate = ask("gate", "art", json!({}), &[], "");
        assert_eq!(title(&gate), "Pass the art gate?");
        let other = ask("permission", "Deploy", json!({}), &[], "");
        assert_eq!(title(&other), "Use Deploy?");
        let question = ask(
            "question",
            "AskUserQuestion",
            json!([{"question": "Which palette?", "options": ["Warm", "Cool"]}]),
            &[],
            "",
        );
        assert_eq!(title(&question), "Which palette?");
    }

    #[test]
    fn a_long_command_is_cut() {
        let command = format!("cargo run -- --port 8080 {}", "--flag ".repeat(20));
        let run = ask("permission", "Bash", json!({"command": command}), &[], "");
        let title = title(&run);
        assert!(
            title.starts_with("Run cargo run -- --port 8080 --flag"),
            "{title}"
        );
        assert!(title.ends_with("…?"), "{title}");
        assert!(title.chars().count() <= SHOWN + 6, "{title}");
    }

    #[test]
    fn a_reason_says_neither_who_said_it_nor_the_rules_id() {
        let ask = ask(
            "permission",
            "Edit",
            json!({"file_path": "src/payments/retry.rs"}),
            &[
                "Lead: sending the launch email goes to a real address, so it is yours to decide",
                "src/payments/retry.rs is protected: Payment code moves real money. Ask before \
                 changing anything under src/payments. (project/shop/protect-payments)",
            ],
            "",
        );
        assert_eq!(
            why(&ask),
            "Sending the launch email goes to a real address, so it is yours to decide · \
             src/payments/retry.rs is protected: Payment code moves real money. Ask before \
             changing anything under src/payments."
        );
    }

    #[test]
    fn a_reason_keeps_words_that_only_look_like_an_id() {
        let ask = ask(
            "permission",
            "Bash",
            json!({}),
            &["it publishes the crate (to the public index)"],
            "",
        );
        assert_eq!(why(&ask), "it publishes the crate (to the public index)");
        // Without reasons the page says what is missing.
        let none = self::ask("permission", "Bash", json!({}), &[], "");
        assert_eq!(why(&none), "No rule allows this yet");
    }
}
