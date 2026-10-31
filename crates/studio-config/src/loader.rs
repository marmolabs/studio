use crate::config::StudioConfig;
use crate::frontmatter;
use crate::paths::StudioPaths;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use studio_core::ids::HireId;
use studio_core::rule::{Hire, Layer, Level, Rule, RuleKind, Scope};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedRules {
    pub rules: Vec<(Layer, Rule)>,
    pub problems: Vec<Problem>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RuleFrontmatter {
    #[serde(default)]
    id: Option<String>,
    kind: RuleKind,
    level: Level,
    #[serde(default)]
    when: Scope,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    command: Option<Vec<String>>,
    #[serde(default)]
    action: Option<String>,
}

pub fn load_rules(config: &StudioConfig, paths: &StudioPaths) -> LoadedRules {
    let mut result = LoadedRules {
        rules: Vec::new(),
        problems: Vec::new(),
    };

    load_layer(
        &mut result,
        Layer::Studio,
        &paths.config_dir.join("rules"),
        &config.rules.disable,
    );

    for pack in &config.packs {
        load_layer(
            &mut result,
            Layer::Pack(pack.name.clone()),
            &pack.path.join("rules"),
            &config.rules.disable,
        );
    }

    for project in &config.projects {
        load_layer(
            &mut result,
            Layer::Project(project.name.clone()),
            &project.path.join(".studio").join("rules"),
            &config.rules.disable,
        );
    }

    result
}

fn load_layer(result: &mut LoadedRules, layer: Layer, dir: &Path, disable: &[String]) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            result.problems.push(Problem {
                path: dir.to_path_buf(),
                message: format!("cannot read rules directory: {e}"),
            });
            return;
        }
    };

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    files.sort();

    for path in files {
        match load_rule_file(&layer, &path) {
            Ok(rule) => {
                if !layer.is_pack() && disable.contains(&rule.id.to_string()) {
                    result.problems.push(Problem {
                        path: path.clone(),
                        message: format!(
                            "rule {} is disabled but only pack rules may be disabled",
                            rule.id
                        ),
                    });
                    result.rules.push((layer.clone(), rule));
                } else if layer.is_pack() && disable.contains(&rule.id.to_string()) {
                    continue;
                } else {
                    result.rules.push((layer.clone(), rule));
                }
            }
            Err(message) => result.problems.push(Problem { path, message }),
        }
    }
}

fn load_rule_file(layer: &Layer, path: &Path) -> Result<Rule, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;
    parse_rule(layer, name, &text)
}

/// Checks that `text` would load as the studio rule called `name`, and that
/// a lint pattern and the path globs compile. Loading keeps a rule with a bad
/// pattern so the policy can ask about it; saving refuses one outright.
pub fn check_rule(name: &str, text: &str) -> Result<(), String> {
    let rule = parse_rule(&Layer::Studio, name, text)?;
    if let Some(pattern) = &rule.pattern {
        regex::Regex::new(pattern).map_err(|e| format!("pattern is not a valid regex: {e}"))?;
    }
    for glob in &rule.when.paths {
        globset::Glob::new(glob).map_err(|e| format!("path {glob:?} is not a valid glob: {e}"))?;
    }
    Ok(())
}

/// Parses `text` as the studio rule called `name`.
pub fn parse_studio_rule(name: &str, text: &str) -> Result<Rule, String> {
    parse_rule(&Layer::Studio, name, text)
}

/// Checks that `text` would load as the hire called `name`.
pub fn check_hire(name: &str, text: &str) -> Result<(), String> {
    parse_hire(name, text).map(|_| ())
}

fn parse_rule(layer: &Layer, name: &str, text: &str) -> Result<Rule, String> {
    let (yaml, body) = frontmatter::split(text)
        .map_err(|e| format!("frontmatter error: {e}"))?
        .ok_or_else(|| "missing frontmatter".to_string())?;
    let fm: RuleFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("yaml parse error: {e}"))?;
    if let Some(id) = &fm.id
        && id != name
    {
        return Err(format!(
            "frontmatter id `{id}` does not match file stem `{name}`"
        ));
    }
    let rule = Rule {
        id: layer.rule_id(name),
        kind: fm.kind,
        level: fm.level,
        when: fm.when,
        pattern: fm.pattern,
        command: fm.command,
        action: fm.action,
        why: body.trim().to_string(),
    };
    rule.validate()
        .map_err(|e| format!("validation error: {e}"))?;
    Ok(rule)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct HireFrontmatter {
    name: String,
    role: String,
    runtime: String,
    model: String,
    #[serde(default)]
    escalate: Vec<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    budget_per_ticket_usd: Option<f64>,
    #[serde(default)]
    daily_usd: Option<f64>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    user_mcp: bool,
}

pub fn load_hires(paths: &StudioPaths) -> Vec<(PathBuf, Result<Hire, String>)> {
    load_hires_in(&paths.config_dir.join("hires"))
}

/// Hires from each pack's `hires/` folder, in pack order. Pack hires are
/// read-only: edit them in the pack. A studio hire with the same id wins.
pub fn load_pack_hires(config: &StudioConfig) -> Vec<(PathBuf, Result<Hire, String>)> {
    config
        .packs
        .iter()
        .flat_map(|pack| load_hires_in(&pack.path.join("hires")))
        .collect()
}

fn load_hires_in(dir: &Path) -> Vec<(PathBuf, Result<Hire, String>)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    files.sort();

    files
        .into_iter()
        .map(|path| {
            let result = load_hire_file(&path);
            (path, result)
        })
        .collect()
}

fn load_hire_file(path: &Path) -> Result<Hire, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid file name".to_string())?;
    parse_hire(name, &text)
}

/// The effort levels Claude Code takes.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn parse_hire(name: &str, text: &str) -> Result<Hire, String> {
    let (yaml, body) = frontmatter::split(text)
        .map_err(|e| format!("frontmatter error: {e}"))?
        .ok_or_else(|| "missing frontmatter".to_string())?;
    let fm: HireFrontmatter =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("yaml parse error: {e}"))?;
    if fm.escalate.iter().any(|m| m.trim().is_empty()) {
        return Err("escalate entries must not be empty".to_string());
    }
    if let Some(effort) = &fm.effort
        && !EFFORTS.contains(&effort.as_str())
    {
        return Err(format!("effort must be one of {}", EFFORTS.join(", ")));
    }
    Ok(Hire {
        id: HireId::new(name),
        name: fm.name,
        role: fm.role,
        runtime: fm.runtime,
        model: fm.model,
        escalate: fm.escalate,
        effort: fm.effort,
        budget_per_ticket_usd: fm.budget_per_ticket_usd,
        daily_usd: fm.daily_usd,
        tools: fm.tools,
        user_mcp: fm.user_mcp,
        prompt: body.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn check_rule_refuses_a_bad_regex_or_glob() {
        let lint = "---\nkind: lint\nlevel: warn\npattern: \"(\"\n---\nNo.\n";
        let protect = "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"[\"]\n---\nNo.\n";
        assert!(check_rule("a", lint).is_err());
        assert!(check_rule("a", protect).is_err());
    }

    fn write_file(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn empty_config() -> StudioConfig {
        StudioConfig {
            budget: crate::config::BudgetConfig::default(),
            concurrency: crate::config::ConcurrencyConfig { max_runs: 1 },
            projects: Vec::new(),
            providers: std::collections::HashMap::new(),
            rules: crate::config::RulesConfig::default(),
            packs: Vec::new(),
            prices: Default::default(),
            goals: Default::default(),
            watchers: Default::default(),
            notify: Default::default(),
        }
    }

    #[test]
    fn loads_studio_rules() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_file(
            &tmp.path().join("rules/keep-app-name.md"),
            "---\nkind: lint\nlevel: block\nwhen:\n  paths:\n    - delivery.toml\npattern: 'store\\.[^.]+\\.name'\n---\nNever change the App Store name.\n",
        );
        let config = empty_config();
        let loaded = load_rules(&config, &paths);
        assert_eq!(loaded.problems.len(), 0);
        assert_eq!(loaded.rules.len(), 1);
        let (layer, rule) = &loaded.rules[0];
        assert_eq!(layer, &Layer::Studio);
        assert_eq!(rule.id.to_string(), "studio/keep-app-name");
        assert_eq!(rule.why, "Never change the App Store name.");
    }

    #[test]
    fn disable_applies_only_to_pack_rules() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        let pack_dir = tmp.path().join("packs/bevy-ios");
        write_file(
            &pack_dir.join("rules/no-unwrap.md"),
            "---\nkind: lint\nlevel: warn\npattern: unwrap\n---\nAvoid unwrap.\n",
        );
        write_file(
            &tmp.path().join("rules/keep-app-name.md"),
            "---\nkind: lint\nlevel: block\npattern: 'x'\n---\nKeep.\n",
        );
        let config = StudioConfig {
            packs: vec![crate::config::PackEntry {
                name: "bevy-ios".to_string(),
                path: pack_dir,
            }],
            rules: crate::config::RulesConfig {
                disable: vec!["pack/bevy-ios/no-unwrap".to_string()],
            },
            ..empty_config()
        };
        let loaded = load_rules(&config, &paths);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].1.id.to_string(), "studio/keep-app-name");
        assert_eq!(loaded.problems.len(), 0);
    }

    #[test]
    fn disabling_non_pack_rule_is_problem() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_file(
            &tmp.path().join("rules/keep-app-name.md"),
            "---\nkind: lint\nlevel: block\npattern: 'x'\n---\nKeep.\n",
        );
        let config = StudioConfig {
            rules: crate::config::RulesConfig {
                disable: vec!["studio/keep-app-name".to_string()],
            },
            ..empty_config()
        };
        let loaded = load_rules(&config, &paths);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.problems.len(), 1);
    }

    #[test]
    fn bad_rule_is_problem_not_failure() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_file(
            &tmp.path().join("rules/invalid.md"),
            "---\nkind: check\nlevel: block\n---\nMissing command.\n",
        );
        write_file(
            &tmp.path().join("rules/valid.md"),
            "---\nkind: lint\nlevel: block\npattern: x\n---\nValid.\n",
        );
        let config = empty_config();
        let loaded = load_rules(&config, &paths);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.problems.len(), 1);
    }

    #[test]
    fn loads_hires() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        write_file(
            &tmp.path().join("hires/engineer.md"),
            "---\nname: Engineer\nrole: engineer\nruntime: claude\nmodel: claude-sonnet-4\ntools:\n  - bash\n---\nYou are a Rust engineer.\n",
        );
        let hires = load_hires(&paths);
        assert_eq!(hires.len(), 1);
        let hire = hires[0].1.as_ref().unwrap();
        assert_eq!(hire.id.to_string(), "engineer");
        assert_eq!(hire.prompt, "You are a Rust engineer.");
        assert!(hire.escalate.is_empty());
    }

    #[test]
    fn pack_hires_load_from_pack_folders() {
        let tmp = TempDir::new().unwrap();
        let pack_dir = tmp.path().join("pack");
        write_file(
            &pack_dir.join("hires/artist.md"),
            "---\nname: Artist\nrole: artist\nruntime: claude-code\nmodel: sonnet\n---\nDraw.\n",
        );
        let config = StudioConfig {
            packs: vec![crate::config::PackEntry {
                name: "p".to_string(),
                path: pack_dir,
            }],
            ..empty_config()
        };
        let hires = load_pack_hires(&config);
        assert_eq!(hires.len(), 1);
        assert_eq!(hires[0].1.as_ref().unwrap().id.to_string(), "artist");
        assert!(load_pack_hires(&empty_config()).is_empty());
    }

    #[test]
    fn hire_escalate_list() {
        let head = "---\nname: E\nrole: engineer\nruntime: claude\nmodel: a\n";
        let ok = format!("{head}escalate: [b, c]\n---\nBody\n");
        assert!(check_hire("e", &ok).is_ok());
        let empty = format!("{head}escalate: [b, \"\"]\n---\nBody\n");
        assert!(check_hire("e", &empty).is_err());
        let wrong = format!("{head}escalate: b\n---\nBody\n");
        assert!(check_hire("e", &wrong).is_err());
    }
}
