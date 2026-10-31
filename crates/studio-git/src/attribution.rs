/// Removes AI attribution lines (co-author trailers, "Generated with Claude"
/// footers, session links) from a commit message.
pub fn strip_attribution(message: &str) -> String {
    let kept: Vec<&str> = message
        .lines()
        .filter(|line| !is_attribution(line))
        .collect();
    kept.join("\n").trim_end().to_string()
}

fn is_attribution(line: &str) -> bool {
    let line = line.trim().to_lowercase();
    line.starts_with("co-authored-by:")
        || line.contains("generated with claude")
        || line.contains("generated with [claude")
        || line.contains("noreply@anthropic.com")
        || line.contains("claude.ai/code")
        || line.contains("claude-session:")
}

#[cfg(test)]
mod tests {
    use super::strip_attribution;

    #[test]
    fn keeps_plain_messages() {
        let msg = "Shorten the retry delay\n\nPlayers waited too long after a fall.";
        assert_eq!(strip_attribution(msg), msg);
    }

    #[test]
    fn removes_trailers_and_footers() {
        let msg = "Fix the menu click\n\nSofter attack.\n\n\
                   🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\n\
                   Co-Authored-By: Claude <noreply@anthropic.com>\n\
                   co-authored-by: someone@example.com\n\
                   Claude-Session: https://claude.ai/code/session_1\n\n";
        assert_eq!(
            strip_attribution(msg),
            "Fix the menu click\n\nSofter attack."
        );
    }

    #[test]
    fn matches_case_insensitively() {
        assert_eq!(
            strip_attribution("Title\nGENERATED WITH CLAUDE CODE"),
            "Title"
        );
    }
}
