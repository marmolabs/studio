//! The owner's files: rules, hires and skills, and the rules a project implies.

use super::{Studio, StudioError, load_all_hires, lock, other};
use crate::api::{FileKind, FileView, Response, RuleFile};
use serde_json::json;
use std::collections::BTreeMap;
use studio_config::config::ProjectEntry;
use studio_config::{check_hire, check_rule};

const MAX_FILE_BYTES: usize = 64 * 1024;

/// The first line of a skill's `description`, or of its text if it has none.
fn skill_summary(text: &str) -> String {
    let first = |s: &str| {
        s.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    };
    let Ok(Some((yaml, body))) = studio_config::frontmatter::split(text) else {
        return first(text).unwrap_or_default();
    };
    let mut lines = yaml.lines();
    while let Some(line) = lines.next() {
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() && !value.starts_with(['>', '|']) {
                return value.to_string();
            }
            // A folded or block value starts on the next line.
            return lines
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .to_string();
        }
    }
    first(body).unwrap_or_default()
}

/// File names are `[a-z0-9][a-z0-9_-]{0,63}`, which also rules out path tricks.
fn valid_name(name: &str) -> Result<(), StudioError> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(other(
            "name must be lowercase letters, digits, - or _, start with a letter or digit, and be at most 64 characters",
        ))
    }
}

fn check(kind: FileKind, name: &str, text: &str) -> Result<(), String> {
    match kind {
        FileKind::Hires => check_hire(name, text),
        FileKind::Rules => check_rule(name, text),
    }
}

/// The `*.md` files in a rules folder, sorted; none if it can't be read.
fn rule_paths(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    paths
}

/// Rules a project's `.studio.toml` implies: its `protected` paths ask, and
/// its `verify` commands (program and subcommand) run without asking.
fn project_rules(project: &ProjectEntry) -> Vec<studio_core::rule::Rule> {
    use studio_core::ids::RuleId;
    use studio_core::rule::{Level, Rule, RuleKind, Scope};
    let Ok(Some(config)) = studio_config::ProjectConfig::load(&project.path) else {
        return Vec::new();
    };
    let scope = |paths: Vec<String>| Scope {
        projects: vec![project.name.clone()],
        paths,
        ..Scope::default()
    };
    let mut rules = Vec::new();
    if !config.protected.is_empty() {
        rules.push(Rule {
            id: RuleId::new(format!("project/{}/protected", project.name)),
            kind: RuleKind::Protect,
            level: Level::Ask,
            when: scope(config.protected),
            pattern: None,
            command: None,
            action: None,
            why: "Listed under protected in .studio.toml.".to_string(),
        });
    }
    for command in &config.verify {
        let prefix: Vec<&str> = command
            .iter()
            .take(2)
            .take_while(|word| !word.starts_with('-'))
            .map(String::as_str)
            .collect();
        if prefix.is_empty() {
            continue;
        }
        let prefix = prefix.join(" ");
        rules.push(Rule {
            id: RuleId::new(format!("project/{}/verify/{prefix}", project.name)),
            kind: RuleKind::Gate,
            level: Level::Allow,
            when: scope(Vec::new()),
            pattern: None,
            command: None,
            action: Some(format!("bash:{prefix}")),
            why: "A verify command in .studio.toml.".to_string(),
        });
    }
    rules
}

impl Studio {
    /// The owner's rules as they are on disk right now, so edits apply to the
    /// next run without a restart.
    pub(crate) fn rules(&self) -> Vec<studio_core::rule::Rule> {
        let loaded = studio_config::load_rules(&self.config(), &self.paths);
        for problem in &loaded.problems {
            eprintln!("rule {}: {}", problem.path.display(), problem.message);
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let mut rules: Vec<_> = loaded
            .rules
            .into_iter()
            .map(|(_, mut rule)| {
                // `~/` in a path glob means the owner's home folder.
                if let Some(home) = &home {
                    for path in &mut rule.when.paths {
                        if let Some(rest) = path.strip_prefix("~/") {
                            *path = home.join(rest).to_string_lossy().into_owned();
                        }
                    }
                }
                rule
            })
            .collect();
        rules.extend(self.config().projects.iter().flat_map(project_rules));
        rules
    }

    /// How many rule files the studio loads: the ones `RuleFiles` lists. It
    /// only lists the folders, since `Status` asks for it every second.
    pub(super) fn rule_files(&self) -> usize {
        self.rule_folders()
            .iter()
            .map(|(_, _, dir)| rule_paths(dir).len())
            .sum()
    }

    /// Where rules live: the studio's folder, each pack's, each project's,
    /// as (origin, owner, folder).
    fn rule_folders(&self) -> Vec<(&'static str, String, std::path::PathBuf)> {
        let config = self.config();
        let mut folders = vec![(
            "studio",
            String::new(),
            self.paths.config_dir.join(FileKind::Rules.dir()),
        )];
        folders.extend(
            config
                .packs
                .iter()
                .map(|p| ("pack", p.name.clone(), p.path.join("rules"))),
        );
        folders.extend(config.projects.iter().map(|p| {
            let dir = p.path.join(".studio").join("rules");
            ("project", p.name.clone(), dir)
        }));
        folders
    }

    /// Every rule file, in the order `load_rules` reads them.
    pub(super) fn rule_file_list(&self) -> Vec<RuleFile> {
        let config = self.config();
        let loaded = studio_config::load_rules(&config, &self.paths);
        let mut files = Vec::new();
        for (origin, owner, dir) in self.rule_folders() {
            for path in rule_paths(&dir) {
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let (text, problem) = match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let problem = loaded.problems.iter().find(|p| p.path == path);
                        (text, problem.map(|p| p.message.clone()))
                    }
                    Err(e) => (String::new(), Some(format!("read error: {e}"))),
                };
                let id = format!("pack/{owner}/{name}");
                files.push(RuleFile {
                    origin: origin.to_string(),
                    disabled: origin == "pack" && config.rules.disable.contains(&id),
                    owner: owner.clone(),
                    name: name.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    text,
                    problem,
                });
            }
        }
        files
    }

    pub(super) fn files(&self, kind: FileKind) -> Result<Response, StudioError> {
        let dir = self.paths.config_dir.join(kind.dir());
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let (text, problem) = match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let problem = check(kind, name, &text).err();
                        (text, problem)
                    }
                    Err(e) => (String::new(), Some(format!("read error: {e}"))),
                };
                files.push(FileView {
                    name: name.to_string(),
                    text,
                    problem,
                });
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Response::ok(files))
    }

    pub(super) fn save_file(
        &self,
        kind: FileKind,
        name: &str,
        text: &str,
    ) -> Result<Response, StudioError> {
        valid_name(name)?;
        if text.len() > MAX_FILE_BYTES {
            return Err(other("file is over 64 KiB"));
        }
        check(kind, name, text).map_err(other)?;
        let dir = self.paths.config_dir.join(kind.dir());
        std::fs::create_dir_all(&dir).map_err(|e| other(format!("cannot create folder: {e}")))?;
        let tmp = dir.join(format!(".{name}.tmp"));
        let written = std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, dir.join(format!("{name}.md"))));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(other(format!("cannot write: {e}")));
        }
        if kind == FileKind::Hires {
            *lock(&self.hires) = load_all_hires(&self.config(), &self.paths);
        }
        Ok(Response::ok(json!({ "name": name })))
    }

    pub(super) fn delete_file(&self, kind: FileKind, name: &str) -> Result<Response, StudioError> {
        valid_name(name)?;
        let path = self
            .paths
            .config_dir
            .join(kind.dir())
            .join(format!("{name}.md"));
        std::fs::remove_file(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => other(format!("no {} file named {name}", kind.dir())),
            _ => other(format!("cannot delete: {e}")),
        })?;
        if kind == FileKind::Hires {
            *lock(&self.hires) = load_all_hires(&self.config(), &self.paths);
        }
        Ok(Response::ok(json!({ "name": name })))
    }

    /// Skill folders: the studio's own, then each pack's. Earlier ones win.
    fn skill_dirs(&self) -> Vec<std::path::PathBuf> {
        let mut dirs = vec![self.paths.config_dir.join("skills")];
        dirs.extend(self.config().packs.iter().map(|p| p.path.join("skills")));
        dirs
    }

    pub(super) fn load_skill(&self, name: Option<&str>) -> Result<String, StudioError> {
        if let Some(name) = name {
            valid_name(name)?;
            return self
                .skill_dirs()
                .iter()
                .find_map(|dir| std::fs::read_to_string(dir.join(name).join("SKILL.md")).ok())
                .ok_or_else(|| other(format!("no skill named {name}")));
        }
        let mut skills = BTreeMap::new();
        for dir in self.skill_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if let Ok(text) = std::fs::read_to_string(entry.path().join("SKILL.md")) {
                    skills.entry(name).or_insert_with(|| skill_summary(&text));
                }
            }
        }
        if skills.is_empty() {
            return Ok("No skills are installed.".to_string());
        }
        Ok(skills
            .iter()
            .map(|(name, summary)| format!("{name}: {summary}"))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
