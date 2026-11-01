//! Files `studio init` and `studio project add` write. Nothing here
//! overwrites a file that already exists.

use crate::config::{ConfigError, StudioConfig};
use crate::pack::load_manifest;
use crate::paths::StudioPaths;
use crate::project::ProjectConfig;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

const STUDIO_TOML: &str = r#"# Studio-wide limits. A run only starts with an allowance that fits under
# every limit at once: studio, project and hire over any rolling 24 hours,
# then ticket and run.
[budget]
daily_usd = 10.0
ticket_usd = 2.0
min_run_usd = 0.05
# run_usd = 1.0

# Agents running at once. 0 pauses the studio.
[concurrency]
max_runs = 2

# A desktop notification when a ticket needs you or is ready to land.
[notify]
desktop = true

# Add projects with `studio project add <path>`.

# Hires run on Claude Code (`runtime: claude-code`) with your own `claude`
# login. Other runtimes are planned.

# USD per million tokens, by model id, for live spend estimates during a
# run. Claude Code reports the final cost itself.
# [prices."claude-sonnet-5-5"]
# input_per_mtok = 0.0
# output_per_mtok = 0.0
# cache_read_per_mtok = 0.0
"#;

const ENGINEER: &str = "---
name: Engineer
role: engineer
runtime: claude-code
model: sonnet
effort: medium
escalate: [opus]
---
You are an engineer at a small software studio. Make the change the ticket asks
for, keep it small, and follow the project's own conventions. Run the
project's checks before you finish, and end with a short summary of what you
changed and why.
";

const PRODUCER: &str = "---
name: Producer
role: producer
runtime: claude-code
model: sonnet
effort: medium
tools: [file_ticket]
---
You are the producer at a small software studio. Given a goal, read the project's
code, then split the goal into tickets with file_ticket: one per independent
change that a single agent can finish and a reviewer can read in one sitting.
Put acceptance criteria in each ticket's detail. Don't write code yourself.
End with a short summary of the plan.
";

const REVIEWER: &str = "---
name: Reviewer
role: reviewer
runtime: claude-code
model: opus
escalate: [sonnet]
effort: medium
budget_per_ticket_usd: 1.5
---
You review changes at a small software studio. Look for bugs, missed edge cases
and anything the ticket didn't ask for. Be specific and brief.

Work economically: start from the diff, read only the files and lines you
need to judge it, and don't re-read what the diff already shows. Give your
verdict as soon as you have decided; don't keep exploring after that.
";

const APPROVER: &str = "---
name: Approver
role: approver
runtime: claude-code
model: haiku
effort: low
---
You decide quickly whether an agent's action is fine for its ticket. You
protect the owner's machine and time.
";

const ROUTER: &str = "---
name: Router
role: router
runtime: claude-code
model: haiku
effort: low
---
You pick the hire best suited to a ticket.
";

const NO_PUSH: &str = "---
kind: gate
level: block
action: \"bash:git push\"
---
The studio lands work; agents never push.
";

const READ_DEPENDENCIES: &str = "---
kind: gate
level: allow
action: \"Read|Grep|Glob\"
when:
  paths:
    - \"~/.cargo/registry/**\"
    - \"~/.cargo/git/**\"
    - \"~/.rustup/toolchains/**\"
---
Agents may read the source of Rust dependencies and the standard library
without asking.
";

#[derive(Error, Debug)]
pub enum StarterError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("studio.toml: {0}")]
    Config(#[from] ConfigError),
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("a project called {0} already exists")]
    DuplicateName(String),
    #[error("{0} is already a project")]
    DuplicatePath(PathBuf),
    #[error("project names use letters, digits, - and _ only: {0}")]
    BadName(String),
    #[error("{0}")]
    BadPack(String),
    #[error("a pack called {0} is already added")]
    DuplicatePack(String),
    #[error("{0} is already a pack")]
    DuplicatePackPath(PathBuf),
    #[error("no pack called {0} in studio.toml; add it with `studio pack add`")]
    UnknownPack(String),
}

/// Writes the starter studio.toml, five hires and two rules. Returns the files
/// it created; existing files are left alone.
pub fn init(paths: &StudioPaths) -> Result<Vec<PathBuf>, StarterError> {
    let dir = &paths.config_dir;
    let files = [
        (dir.join("studio.toml"), STUDIO_TOML),
        (dir.join("hires/engineer.md"), ENGINEER),
        (dir.join("hires/producer.md"), PRODUCER),
        (dir.join("hires/reviewer.md"), REVIEWER),
        (dir.join("hires/approver.md"), APPROVER),
        (dir.join("hires/router.md"), ROUTER),
        (dir.join("rules/no-push.md"), NO_PUSH),
        (dir.join("rules/read-dependencies.md"), READ_DEPENDENCIES),
    ];
    let mut created = Vec::new();
    for (path, text) in files {
        if write_new(&path, text)? {
            created.push(path);
        }
    }
    fs::create_dir_all(&paths.data_dir)?;
    Ok(created)
}

pub struct NewProject<'a> {
    pub path: &'a Path,
    pub name: Option<&'a str>,
    pub integration_branch: Option<&'a str>,
    pub daily_usd: Option<f64>,
    /// A pack (by name) whose `project.toml` becomes the project's `.studio.toml`.
    pub pack: Option<&'a str>,
}

pub struct AddedProject {
    pub name: String,
    pub path: PathBuf,
    /// The project's `.studio.toml`, when this call created it.
    pub created_checks: Option<PathBuf>,
}

/// Appends a project to studio.toml, keeping the file's comments, and gives
/// the project a `.studio.toml` with guessed checks if it has none.
pub fn add_project(
    paths: &StudioPaths,
    project: NewProject<'_>,
) -> Result<AddedProject, StarterError> {
    if !project.path.is_dir() {
        return Err(StarterError::NotADirectory(project.path.to_path_buf()));
    }
    let path = project.path.canonicalize()?;
    let name = match project.name {
        Some(name) => name.to_string(),
        // A folder's name may hold characters a project name can't ("my.game",
        // "my game"); those become dashes.
        None => {
            let folder = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let name: String = folder
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            match name.trim_matches('-') {
                "" => "project".to_string(),
                trimmed => trimmed.to_string(),
            }
        }
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(StarterError::BadName(name));
    }
    let config = StudioConfig::load(paths)?;
    // Read the pack's template before touching studio.toml.
    let template = match project.pack {
        Some(pack) => {
            let entry = config
                .packs
                .iter()
                .find(|p| p.name == pack)
                .ok_or_else(|| StarterError::UnknownPack(pack.to_string()))?;
            match fs::read_to_string(entry.path.join("project.toml")) {
                Ok(text) => {
                    ProjectConfig::parse(&text).map_err(|e| {
                        StarterError::BadPack(format!("{pack}'s project.toml: {e}"))
                    })?;
                    Some(text)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            }
        }
        None => None,
    };
    for existing in &config.projects {
        if existing.name == name {
            return Err(StarterError::DuplicateName(name));
        }
        if existing.path.canonicalize().ok().as_deref() == Some(path.as_path()) {
            return Err(StarterError::DuplicatePath(path));
        }
    }

    let mut entry = format!(
        "\n[[projects]]\nname = {}\npath = {}\n",
        toml_string(&name),
        toml_string(&path.to_string_lossy())
    );
    if let Some(branch) = project.integration_branch {
        entry.push_str(&format!("integration_branch = {}\n", toml_string(branch)));
    }
    if let Some(usd) = project.daily_usd {
        entry.push_str(&format!("daily_usd = {usd:?}\n"));
    }
    let config_path = paths.config_dir.join("studio.toml");
    fs::OpenOptions::new()
        .append(true)
        .open(&config_path)?
        .write_all(entry.as_bytes())?;
    // Fail loudly now rather than when the daemon next starts.
    StudioConfig::load(paths)?;

    let checks = path.join(".studio.toml");
    let text = template.unwrap_or_else(|| project_checks(&path));
    let created_checks = write_new(&checks, &text)?.then_some(checks);
    Ok(AddedProject {
        name,
        path,
        created_checks,
    })
}

pub struct AddedPack {
    pub name: String,
    pub path: PathBuf,
}

/// Appends a pack to studio.toml, keeping the file's comments, after checking
/// its `pack.toml`.
pub fn add_pack(paths: &StudioPaths, dir: &Path) -> Result<AddedPack, StarterError> {
    if !dir.is_dir() {
        return Err(StarterError::NotADirectory(dir.to_path_buf()));
    }
    let path = dir.canonicalize()?;
    let manifest = load_manifest(&path).map_err(StarterError::BadPack)?;
    let config = StudioConfig::load(paths)?;
    for existing in &config.packs {
        if existing.name == manifest.name {
            return Err(StarterError::DuplicatePack(manifest.name));
        }
        if existing.path.canonicalize().ok().as_deref() == Some(path.as_path()) {
            return Err(StarterError::DuplicatePackPath(path));
        }
    }
    let entry = format!(
        "\n[[packs]]\nname = {}\npath = {}\n",
        toml_string(&manifest.name),
        toml_string(&path.to_string_lossy())
    );
    fs::OpenOptions::new()
        .append(true)
        .open(paths.config_dir.join("studio.toml"))?
        .write_all(entry.as_bytes())?;
    StudioConfig::load(paths)?;
    Ok(AddedPack {
        name: manifest.name,
        path,
    })
}

fn project_checks(project: &Path) -> String {
    let cargo = project.join("Cargo.toml").is_file();
    let verify = if cargo {
        "verify = [\n  [\"cargo\", \"build\", \"--all-targets\"],\n  [\"cargo\", \"test\"],\n]\n"
    } else {
        "verify = []\n"
    };
    // Every ticket works in its own worktree; one build folder for all of them
    // saves each ticket a build from scratch.
    let env = if cargo {
        "\n# Environment for agents, checks and captures; {repo} is this checkout.\n\
         # One build folder for every ticket's worktree, so none starts from scratch.\n\
         [env]\n\
         CARGO_TARGET_DIR = \"{repo}/.studio/target\"\n"
    } else {
        ""
    };
    format!(
        "# Commands that must pass before a change can land. Each runs in the\n\
         # ticket's worktree; a failure goes back to the agent with its output.\n\
         {verify}\n\
         # Paths agents may not change without asking you.\n\
         protected = []\n\
         {env}"
    )
}

/// Writes `text` to `path` unless it exists. True when it wrote.
fn write_new(path: &Path, text: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProjectConfig, load_hires, load_rules};
    use tempfile::TempDir;

    fn home() -> (TempDir, StudioPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        (tmp, paths)
    }

    #[test]
    fn init_writes_a_working_studio_once() {
        let (_tmp, paths) = home();
        let created = init(&paths).unwrap();
        assert_eq!(created.len(), 8);

        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.budget.daily_usd, 10.0);
        let hires = load_hires(&paths);
        assert_eq!(hires.len(), 5);
        let hires: Vec<_> = hires.into_iter().map(|(_, h)| h.unwrap()).collect();
        let hire = |role: &str| hires.iter().find(|h| h.role == role).unwrap();
        // The reviewer only reviews a hire on a different model.
        assert_ne!(hire("engineer").model, hire("reviewer").model);
        assert_eq!(hire("approver").model, "haiku");
        assert_eq!(hire("router").effort.as_deref(), Some("low"));
        assert_eq!(hire("producer").tools, vec!["file_ticket"]);
        let rules = load_rules(&config, &paths);
        assert!(rules.problems.is_empty());
        assert_eq!(rules.rules.len(), 2);

        fs::write(paths.config_dir.join("hires/engineer.md"), "mine").unwrap();
        assert!(init(&paths).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(paths.config_dir.join("hires/engineer.md")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn adding_a_project_keeps_comments_and_writes_checks() {
        let (_tmp, paths) = home();
        init(&paths).unwrap();
        let game = TempDir::new().unwrap();
        let game_dir = game.path().join("my-game");
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join("Cargo.toml"), "[package]\n").unwrap();

        let added = add_project(
            &paths,
            NewProject {
                path: &game_dir,
                name: None,
                integration_branch: Some("master"),
                daily_usd: Some(3.0),
                pack: None,
            },
        )
        .unwrap();
        assert_eq!(added.name, "my-game");
        assert!(added.created_checks.is_some());

        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.projects.len(), 1);
        assert_eq!(
            config.projects[0].integration_branch.as_deref(),
            Some("master")
        );
        assert_eq!(config.projects[0].daily_usd, Some(3.0));
        let text = fs::read_to_string(paths.config_dir.join("studio.toml")).unwrap();
        assert!(text.contains("# Add projects with"));

        let checks = ProjectConfig::load(&game_dir).unwrap().unwrap();
        assert_eq!(checks.verify[1], vec!["cargo", "test"]);
        assert_eq!(
            checks.env_for(&game_dir),
            [(
                "CARGO_TARGET_DIR".to_string(),
                format!("{}/.studio/target", game_dir.display())
            )]
        );

        let again = |name: Option<&str>| {
            add_project(
                &paths,
                NewProject {
                    path: &game_dir,
                    name,
                    integration_branch: None,
                    daily_usd: None,
                    pack: None,
                },
            )
        };
        assert!(matches!(again(None), Err(StarterError::DuplicateName(_))));
        assert!(matches!(
            again(Some("other")),
            Err(StarterError::DuplicatePath(_))
        ));
        assert!(matches!(
            again(Some("bad name")),
            Err(StarterError::BadName(_))
        ));
    }

    #[test]
    fn existing_project_checks_are_kept() {
        let (_tmp, paths) = home();
        init(&paths).unwrap();
        let game = TempDir::new().unwrap();
        fs::write(game.path().join(".studio.toml"), "verify = [[\"make\"]]\n").unwrap();
        let added = add_project(
            &paths,
            NewProject {
                path: game.path(),
                name: Some("game"),
                integration_branch: None,
                daily_usd: None,
                pack: None,
            },
        )
        .unwrap();
        assert!(added.created_checks.is_none());
        assert_eq!(
            fs::read_to_string(game.path().join(".studio.toml")).unwrap(),
            "verify = [[\"make\"]]\n"
        );
    }

    fn make_pack(root: &Path, name: &str, project: Option<&str>) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pack.toml"), format!("name = \"{name}\"\n")).unwrap();
        if let Some(text) = project {
            fs::write(dir.join("project.toml"), text).unwrap();
        }
        dir
    }

    #[test]
    fn adding_a_pack_keeps_comments_and_refuses_duplicates() {
        let (_tmp, paths) = home();
        init(&paths).unwrap();
        let src = TempDir::new().unwrap();
        let dir = make_pack(src.path(), "mine", None);

        let added = add_pack(&paths, &dir).unwrap();
        assert_eq!(added.name, "mine");
        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.packs.len(), 1);
        let text = fs::read_to_string(paths.config_dir.join("studio.toml")).unwrap();
        assert!(text.contains("# Add projects with"));

        assert!(matches!(
            add_pack(&paths, &dir),
            Err(StarterError::DuplicatePack(_))
        ));
        let copy = src.path().join("copy");
        fs::create_dir_all(&copy).unwrap();
        fs::copy(dir.join("pack.toml"), copy.join("pack.toml")).unwrap();
        assert!(matches!(
            add_pack(&paths, &copy),
            Err(StarterError::DuplicatePack(_))
        ));
        let bare = src.path().join("bare");
        fs::create_dir_all(&bare).unwrap();
        assert!(matches!(
            add_pack(&paths, &bare),
            Err(StarterError::BadPack(_))
        ));
        assert_eq!(StudioConfig::load(&paths).unwrap().packs.len(), 1);
    }

    #[test]
    fn a_pack_template_becomes_the_projects_checks() {
        let (_tmp, paths) = home();
        init(&paths).unwrap();
        let src = TempDir::new().unwrap();
        let template = "verify = [[\"make\"]]\ncapture = [\"run\", \"{out}\"]\n";
        add_pack(&paths, &make_pack(src.path(), "with", Some(template))).unwrap();
        add_pack(&paths, &make_pack(src.path(), "without", None)).unwrap();
        let new = |dir: &Path, name: &str, pack: Option<&str>| {
            add_project(
                &paths,
                NewProject {
                    path: dir,
                    name: Some(name),
                    integration_branch: None,
                    daily_usd: None,
                    pack,
                },
            )
        };
        let (a, b, c, d) = (
            src.path().join("a"),
            src.path().join("b"),
            src.path().join("c"),
            src.path().join("d"),
        );
        for dir in [&a, &b, &c, &d] {
            fs::create_dir_all(dir).unwrap();
        }
        new(&a, "a", Some("with")).unwrap();
        assert_eq!(
            fs::read_to_string(a.join(".studio.toml")).unwrap(),
            template
        );
        // A repo that has its own checks keeps them.
        fs::write(b.join(".studio.toml"), "verify = []\n").unwrap();
        new(&b, "b", Some("with")).unwrap();
        assert_eq!(
            fs::read_to_string(b.join(".studio.toml")).unwrap(),
            "verify = []\n"
        );
        // No template in the pack: the guessed checks are used.
        new(&c, "c", Some("without")).unwrap();
        assert!(c.join(".studio.toml").is_file());
        assert!(matches!(
            new(&d, "d", Some("nope")),
            Err(StarterError::UnknownPack(_))
        ));
        assert!(!d.join(".studio.toml").exists());
    }
}
