//! Rule files drafted from the owner's corrections: a denied ask or a ticket
//! sent back. The owner edits the draft before saving it.

use serde_json::Value;

const WORKTREES: &str = "/.studio/worktrees/";
const NO_REASON: &str = "Write why this is not allowed.";
const NAME_MAX: usize = 64;
/// Tools that edit or read a file, which a protect rule can guard.
const FILE_TOOLS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "Read"];

/// A rule file someone can still edit before saving: its file name (no .md) and text.
pub struct Draft {
    pub name: String,
    pub text: String,
}

/// Lowercase letters, digits and dashes, for use in a file name.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// `name` cut to a file name's length.
fn short(name: &str) -> String {
    let cut: String = name.chars().take(NAME_MAX).collect();
    cut.trim_end_matches('-').to_string()
}

/// The path `input` names, made relative to the worktree it is in.
fn repo_path(input: &Value) -> Option<String> {
    let path = ["file_path", "path", "notebook_path"]
        .iter()
        .find_map(|key| input[*key].as_str())?;
    let rest = path.split_once(WORKTREES).map(|(_, rest)| rest);
    match rest.and_then(|rest| rest.split_once('/')) {
        Some((_, inside)) if !inside.is_empty() => Some(inside.to_string()),
        _ => Some(path.to_string()),
    }
}

/// A rule for something the owner denied. `reason` may be empty.
pub fn from_denial(project: &str, tool: &str, input: &Value, reason: &str) -> Draft {
    let why = match reason.trim() {
        "" => NO_REASON,
        reason => reason,
    };
    let scope = format!("projects: [{}]", quote(project));
    let who = slug(project);
    let prefix = (tool == "Bash")
        .then(|| input["command"].as_str())
        .flatten()
        .and_then(studio_core::policy::allow_prefix);
    let (name, rule) = if let Some(prefix) = prefix {
        let action = quote(&format!("bash:{prefix}"));
        (
            format!("block-{who}-{}", slug(&prefix)),
            format!("kind: gate\nlevel: block\naction: {action}\nwhen:\n  {scope}"),
        )
    } else if let Some(path) = FILE_TOOLS
        .contains(&tool)
        .then(|| repo_path(input))
        .flatten()
    {
        let file = path.rsplit('/').next().unwrap_or(&path);
        (
            format!("protect-{who}-{}", slug(file)),
            format!(
                "kind: protect\nlevel: ask\nwhen:\n  {scope}\n  paths: [{}]",
                quote(&path)
            ),
        )
    } else {
        (
            format!("ask-{who}-{}", slug(tool)),
            format!(
                "kind: gate\nlevel: ask\naction: {}\nwhen:\n  {scope}",
                quote(tool)
            ),
        )
    };
    Draft {
        name: short(&name),
        text: format!("---\n{rule}\n---\n{why}\n"),
    }
}

/// A guide for a ticket the owner sent back with `feedback`.
pub fn from_feedback(project: &str, feedback: &str) -> Draft {
    let feedback = feedback.trim();
    let words: Vec<&str> = feedback.split_whitespace().take(6).collect();
    let name = format!("guide-{}-{}", slug(project), slug(&words.join(" ")));
    Draft {
        name: short(&name),
        text: format!(
            "---\nkind: guide\nlevel: warn\nwhen:\n  projects: [{}]\n---\n{feedback}\n",
            quote(project)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::check_rule;
    use serde_json::json;

    fn loads(draft: &Draft) {
        check_rule(&draft.name, &draft.text).unwrap();
    }

    #[test]
    fn a_denied_command_becomes_a_block_gate() {
        let input = json!({"command": "git push origin main"});
        let d = from_denial("My Game", "Bash", &input, " Never push. ");
        loads(&d);
        assert_eq!(d.name, "block-my-game-git-push");
        assert!(d.text.contains("kind: gate\nlevel: block"));
        assert!(d.text.contains(r#"action: "bash:git push""#));
        assert!(d.text.contains(r#"projects: ["My Game"]"#));
        assert!(d.text.ends_with("---\nNever push.\n"));
    }

    #[test]
    fn a_chained_command_falls_back_to_the_tool_gate() {
        let input = json!({"command": "cargo test && rm -rf x"});
        let d = from_denial("game", "Bash", &input, "");
        loads(&d);
        assert_eq!(d.name, "ask-game-bash");
        assert!(d.text.contains("level: ask\naction: \"Bash\""));
        assert!(d.text.contains(NO_REASON));
    }

    #[test]
    fn a_denied_file_becomes_a_protect_rule_inside_the_repo() {
        let input = json!({"file_path": "/p/game/.studio/worktrees/game-1/src/main.rs"});
        let d = from_denial("game", "Edit", &input, "Hands off.");
        loads(&d);
        assert_eq!(d.name, "protect-game-main-rs");
        assert!(d.text.contains("kind: protect\nlevel: ask"));
        assert!(d.text.contains(r#"paths: ["src/main.rs"]"#));
    }

    #[test]
    fn a_path_outside_a_worktree_is_kept() {
        let input = json!({"notebook_path": "/etc/hosts"});
        let d = from_denial("game", "NotebookEdit", &input, "");
        loads(&d);
        assert!(d.text.contains(r#"paths: ["/etc/hosts"]"#));
    }

    #[test]
    fn other_tools_get_a_tool_gate() {
        let d = from_denial("game", "WebFetch", &json!({"url": "https://x.test"}), "No.");
        loads(&d);
        assert_eq!(d.name, "ask-game-webfetch");
        assert!(d.text.contains(r#"action: "WebFetch""#));
    }

    #[test]
    fn a_search_with_a_path_gets_a_tool_gate() {
        let input = json!({"pattern": "todo", "path": "/p/game/src"});
        for tool in ["Grep", "Glob"] {
            let d = from_denial("game", tool, &input, "No.");
            loads(&d);
            assert!(d.text.contains("kind: gate\nlevel: ask"), "{}", d.text);
        }
    }

    #[test]
    fn names_are_cut_to_64_characters() {
        let project = "p".repeat(80);
        let d = from_denial(&project, "WebFetch", &json!({}), "");
        assert!(d.name.len() <= NAME_MAX && !d.name.ends_with('-'));
    }

    #[test]
    fn feedback_becomes_a_guide() {
        let text = "Keep the HUD out of the pause menu, it hides the buttons. Thanks";
        let d = from_feedback("My Game", &format!("  {text}\n"));
        loads(&d);
        assert_eq!(d.name, "guide-my-game-keep-the-hud-out-of-the");
        assert!(d.text.contains("kind: guide\nlevel: warn"));
        assert!(d.text.contains(r#"projects: ["My Game"]"#));
        assert!(d.text.ends_with(&format!("---\n{text}\n")));
    }
}
