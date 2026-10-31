use std::fs;
use std::path::{Path, PathBuf};
use studio_core::rule::{Level, Rule, RuleKind, Scope};

pub fn write_rule(dir: impl AsRef<Path>, rule: &Rule) -> std::io::Result<PathBuf> {
    let id = rule.id.to_string();
    let name = id.rsplit_once('/').map(|(_, n)| n).unwrap_or(&id);
    let path = dir.as_ref().join(format!("{name}.md"));
    fs::create_dir_all(dir.as_ref())?;
    let frontmatter = RuleFrontmatter::from(rule);
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(
        &serde_yaml_ng::to_string(&frontmatter)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?,
    );
    output.push_str("---\n");
    output.push_str(&rule.why);
    if !rule.why.ends_with('\n') {
        output.push('\n');
    }
    fs::write(&path, output)?;
    Ok(path)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct RuleFrontmatter<'a> {
    id: String,
    kind: &'a RuleKind,
    level: &'a Level,
    #[serde(skip_serializing_if = "is_default_scope", default)]
    when: &'a Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: &'a Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: &'a Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: &'a Option<String>,
}

fn is_default_scope(scope: &Scope) -> bool {
    scope.projects.is_empty()
        && scope.paths.is_empty()
        && scope.hires.is_empty()
        && scope.roles.is_empty()
}

impl<'a> From<&'a Rule> for RuleFrontmatter<'a> {
    fn from(rule: &'a Rule) -> Self {
        let id = rule.id.to_string();
        let name = id
            .rsplit_once('/')
            .map(|(_, n)| n)
            .unwrap_or(&id)
            .to_string();
        Self {
            id: name,
            kind: &rule.kind,
            level: &rule.level,
            when: &rule.when,
            pattern: &rule.pattern,
            command: &rule.command,
            action: &rule.action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BudgetConfig, ConcurrencyConfig, RulesConfig, StudioConfig};
    use crate::loader::load_rules;
    use crate::paths::StudioPaths;
    use studio_core::ids::RuleId;
    use studio_core::rule::{Rule, RuleKind};
    use tempfile::TempDir;

    #[test]
    fn round_trip_rule() {
        let tmp = TempDir::new().unwrap();
        let rule = Rule {
            id: RuleId::new("studio/keep-app-name"),
            kind: RuleKind::Lint,
            level: Level::Block,
            when: Scope {
                paths: vec!["delivery.toml".to_string()],
                ..Default::default()
            },
            pattern: Some(r"store\.[^.]+\.name".to_string()),
            command: None,
            action: None,
            why: "Never change the App Store name.".to_string(),
        };
        let rule_dir = tmp.path().join("rules");
        write_rule(&rule_dir, &rule).unwrap();

        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        let config = StudioConfig {
            budget: BudgetConfig::default(),
            concurrency: ConcurrencyConfig { max_runs: 1 },
            projects: Vec::new(),
            providers: std::collections::HashMap::new(),
            rules: RulesConfig::default(),
            packs: Vec::new(),
            prices: Default::default(),
            goals: Default::default(),
            watchers: Default::default(),
            notify: Default::default(),
        };
        let loaded = load_rules(&config, &paths);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].1, rule);
    }
}
