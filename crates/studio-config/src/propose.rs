use std::collections::HashMap;
use std::fs;
use std::path::Path;
use studio_core::ids::RuleId;
use studio_core::rule::{Level, Rule, RuleKind, Scope};

pub fn propose_rules(project_dir: impl AsRef<Path>) -> Vec<Rule> {
    let mut rules = Vec::new();
    let paths = [
        project_dir.as_ref().join("CLAUDE.md"),
        project_dir.as_ref().join("AGENTS.md"),
    ];
    for path in &paths {
        if let Ok(text) = fs::read_to_string(path) {
            rules.extend(propose_from_text(&text));
        }
    }
    rules
}

fn propose_from_text(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut seen_slugs: HashMap<String, u32> = HashMap::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut in_section = false;
    let mut in_code_block = false;
    let mut code_lines: Vec<String> = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') && trimmed.to_ascii_lowercase().contains("before a pr") {
            in_section = true;
            continue;
        }
        if in_section && trimmed.starts_with("```") {
            if in_code_block {
                rules.extend(parse_check_rules(&code_lines, &mut seen_slugs));
                code_lines.clear();
                in_code_block = false;
                in_section = false;
            } else {
                in_code_block = true;
            }
            continue;
        }
        if in_code_block {
            code_lines.push(line.to_string());
        }
        if in_section && trimmed.starts_with('#') {
            in_section = false;
        }
    }

    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("owner") || lower.contains("out of bounds") || lower.contains("never") {
            rules.extend(parse_protect_rules(line, &mut seen_slugs));
        }
    }

    rules
}

fn parse_check_rules(lines: &[String], seen: &mut HashMap<String, u32>) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut joined = String::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(idx) = line.trim_end().rfind('\\') {
            joined.push_str(line[..idx].trim());
            joined.push(' ');
            continue;
        }
        joined.push_str(line);
        let command_line = joined.trim().to_string();
        joined.clear();
        let command = strip_trailing_comment(&command_line);
        let parts: Vec<String> = command.split_whitespace().map(String::from).collect();
        if parts.is_empty() {
            continue;
        }
        let name = unique_slug(&parts.join("-"), seen);
        rules.push(Rule {
            id: RuleId::new(format!("project/{name}")),
            kind: RuleKind::Check,
            level: Level::Block,
            when: Scope::default(),
            pattern: None,
            command: Some(parts),
            action: None,
            why: command_line,
        });
    }
    rules
}

fn strip_trailing_comment(line: &str) -> String {
    let mut in_quote = false;
    for (i, c) in line.char_indices() {
        if c == '\'' || c == '"' {
            in_quote = !in_quote;
        }
        if c == '#' && !in_quote {
            return line[..i].trim().to_string();
        }
    }
    line.trim().to_string()
}

fn parse_protect_rules(line: &str, seen: &mut HashMap<String, u32>) -> Vec<Rule> {
    let mut rules = Vec::new();
    for token in line.split('`').skip(1).step_by(2) {
        if is_path_token(token) {
            let name = unique_slug(token, seen);
            rules.push(Rule {
                id: RuleId::new(format!("project/{name}")),
                kind: RuleKind::Protect,
                level: Level::Ask,
                when: Scope {
                    paths: vec![token.to_string()],
                    ..Default::default()
                },
                pattern: None,
                command: None,
                action: None,
                why: line.trim().to_string(),
            });
        }
    }
    rules
}

fn is_path_token(token: &str) -> bool {
    if token.contains(' ') {
        return false;
    }
    if token.contains('/') {
        return true;
    }
    if token.chars().filter(|&c| c == '.').count() > 1 {
        return false;
    }
    if let Some(dot) = token.rfind('.') {
        let ext = &token[dot + 1..];
        !ext.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}

fn unique_slug(text: &str, seen: &mut HashMap<String, u32>) -> String {
    let base = slug(text);
    let count = seen.entry(base.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base
    } else {
        format!("{base}-{count}")
    }
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' => c,
            '/' | '\\' | ' ' | '_' => '-',
            _ => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposes_check_rules_from_before_pr_block() {
        let text = r#"
# Before a PR
```sh
cargo fmt --check
cargo clippy --locked --workspace --all-targets # ensure clean
cargo test --locked --workspace
```
"#;
        let rules = propose_from_text(text);
        assert_eq!(rules.len(), 3);
        assert!(rules.iter().all(|r| matches!(r.kind, RuleKind::Check)));
        assert_eq!(
            rules[2].command,
            Some(vec![
                "cargo".to_string(),
                "test".to_string(),
                "--locked".to_string(),
                "--workspace".to_string()
            ])
        );
    }

    #[test]
    fn joins_continuation_lines() {
        let text = r#"
# Before a PR
```sh
cargo clippy --locked \
    --workspace
```
"#;
        let rules = propose_from_text(text);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].command.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn proposes_protect_rules_from_owner_lines() {
        let text = "Only the owner may edit `crates/engine/src/rules.rs` or `delivery.toml`.";
        let rules = propose_from_text(text);
        assert_eq!(rules.len(), 2);
        assert!(
            rules
                .iter()
                .all(|r| matches!(r.kind, RuleKind::Protect) && matches!(r.level, Level::Ask))
        );
        assert_eq!(
            rules[0].when.paths,
            vec!["crates/engine/src/rules.rs".to_string()]
        );
    }

    #[test]
    fn fixture_loads_without_error() {
        let rules = propose_rules("tests/fixtures");
        assert!(!rules.is_empty());
    }

    #[test]
    fn ignores_backticked_non_paths() {
        let text = "Out of bounds without the owner: save formats, bundle `com.example.Game` (never renamed), and the tuning in `rules.rs`.";
        let rules = propose_from_text(text);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].kind, RuleKind::Protect);
        assert_eq!(rules[0].when.paths, vec!["rules.rs".to_string()]);
    }

    #[test]
    fn strip_trailing_comment_keeps_non_ascii() {
        assert_eq!(
            strip_trailing_comment("echo héllo # a comment"),
            "echo héllo"
        );
    }

    #[test]
    fn unique_slugs_on_collision() {
        let text = r#"
# Before a PR
```sh
cargo fmt --check
cargo fmt-check
```
"#;
        let rules = propose_from_text(text);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id.to_string(), "project/cargo-fmt-check");
        assert_eq!(rules[1].id.to_string(), "project/cargo-fmt-check-2");
    }
}
