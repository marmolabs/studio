use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub verify: Vec<Vec<String>>,
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub integration_branch: Option<String>,
    /// Command that runs the project and writes screenshots into `{out}`.
    #[serde(default)]
    pub capture: Vec<String>,
    /// Changed files (globs) that call for a capture; empty means any change.
    #[serde(default)]
    pub capture_paths: Vec<String>,
    /// Critic hires that look at matching changes before the owner does.
    #[serde(default)]
    pub gates: Vec<Gate>,
    /// Land a ticket on its goal branch as soon as it is ready.
    #[serde(default)]
    pub auto_land: bool,
    /// Environment for this project's agents, checks and captures, such as a
    /// build cache its worktrees share. `{repo}` in a value is the project's
    /// own checkout.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub name: String,
    /// Changed files (globs) that send the work to this gate.
    pub paths: Vec<String>,
    /// The hire that judges the work.
    pub hire: String,
    /// Extra instructions for the critic.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Most times this gate may send the work back before the owner is asked.
    #[serde(default = "default_rounds")]
    pub rounds: u32,
    /// Command run on changed files that match `paths`, with `{file}` in its
    /// arguments replaced by each file's path. Its output goes to the critic.
    #[serde(default)]
    pub measure: Vec<String>,
}

fn default_rounds() -> u32 {
    3
}

/// True when any file matches any glob. Globs were checked on load, so one
/// that fails to compile here matches nothing.
pub fn matches_any(globs: &[String], files: &[String]) -> bool {
    globs
        .iter()
        .filter_map(|g| globset::Glob::new(g).ok())
        .map(|g| g.compile_matcher())
        .any(|m| files.iter().any(|f| m.is_match(f)))
}

#[derive(Error, Debug)]
pub enum ProjectConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("bad glob `{0}`: {1}")]
    Glob(String, String),
    #[error("gate `{0}` has an empty measure program")]
    EmptyMeasure(String),
    #[error("`{0}` is not an environment variable name")]
    EnvName(String),
}

impl ProjectConfig {
    pub fn load(project_dir: impl AsRef<Path>) -> Result<Option<Self>, ProjectConfigError> {
        let path = project_dir.as_ref().join(".studio.toml");
        match fs::read_to_string(&path) {
            Ok(text) => Ok(Some(Self::parse(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Parses `.studio.toml` text and checks every glob in it.
    pub fn parse(text: &str) -> Result<Self, ProjectConfigError> {
        let config: ProjectConfig = toml::from_str(text)?;
        let globs = config
            .capture_paths
            .iter()
            .chain(config.gates.iter().flat_map(|g| &g.paths));
        for glob in globs {
            globset::Glob::new(glob)
                .map_err(|e| ProjectConfigError::Glob(glob.clone(), e.to_string()))?;
        }
        for gate in &config.gates {
            if gate.measure.first().is_some_and(|p| p.trim().is_empty()) {
                return Err(ProjectConfigError::EmptyMeasure(gate.name.clone()));
            }
        }
        for name in config.env.keys() {
            let valid = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && name.chars().next().is_some_and(|c| !c.is_ascii_digit());
            if !valid {
                return Err(ProjectConfigError::EnvName(name.clone()));
            }
        }
        Ok(config)
    }

    /// The project's environment with `{repo}` replaced by its checkout.
    pub fn env_for(&self, repo: &Path) -> Vec<(String, String)> {
        let repo = repo.to_string_lossy();
        self.env
            .iter()
            .map(|(name, value)| (name.clone(), value.replace("{repo}", &repo)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn env_names_are_checked_and_repo_is_filled_in() {
        let config = ProjectConfig::parse(
            "[env]\nCARGO_TARGET_DIR = \"{repo}/.studio/target\"\nMODE = \"ci\"\n",
        )
        .unwrap();
        assert_eq!(
            config.env_for(Path::new("/code/shop")),
            [
                (
                    "CARGO_TARGET_DIR".to_string(),
                    "/code/shop/.studio/target".to_string()
                ),
                ("MODE".to_string(), "ci".to_string())
            ]
        );
        for bad in ["\"A B\" = \"x\"", "\"1X\" = \"x\"", "\"A=B\" = \"x\""] {
            let err = ProjectConfig::parse(&format!("[env]\n{bad}\n")).unwrap_err();
            assert!(matches!(err, ProjectConfigError::EnvName(_)), "{bad}");
        }
    }

    #[test]
    fn loads_project_config() {
        let tmp = TempDir::new().unwrap();
        let mut file = fs::File::create(tmp.path().join(".studio.toml")).unwrap();
        file.write_all(
            br#"
verify = [["cargo", "test"], ["cargo", "clippy"]]
protected = ["release.toml"]
integration_branch = "master"
"#,
        )
        .unwrap();

        let config = ProjectConfig::load(tmp.path()).unwrap().unwrap();
        assert_eq!(
            config.verify,
            vec![vec!["cargo", "test"], vec!["cargo", "clippy"]]
        );
        assert_eq!(config.protected, vec!["release.toml"]);
        assert_eq!(config.integration_branch, Some("master".to_string()));
        assert!(!config.auto_land);
    }

    #[test]
    fn loads_auto_land() {
        let config = ProjectConfig::parse("auto_land = true").unwrap();
        assert!(config.auto_land);
    }

    #[test]
    fn loads_capture_and_gates() {
        let config = ProjectConfig::parse(
            r#"
capture = ["cargo", "run", "--", "--capture", "{out}"]
capture_paths = ["src/**"]

[[gates]]
name = "art"
paths = ["assets/**/*.png"]
hire = "art-critic"

[[gates]]
name = "copy"
paths = ["store/**/*.md"]
hire = "reviewer"
prompt = "Check the tone."
rounds = 2
measure = ["tools/pixel-lint", "{file}"]
"#,
        )
        .unwrap();
        assert_eq!(config.capture.len(), 5);
        assert_eq!(config.gates[0].rounds, 3);
        assert_eq!(config.gates[0].prompt, None);
        assert_eq!(config.gates[1].rounds, 2);
        assert!(config.gates[0].measure.is_empty());
        assert_eq!(config.gates[1].measure, vec!["tools/pixel-lint", "{file}"]);
        assert!(ProjectConfig::parse("").unwrap().gates.is_empty());
    }

    #[test]
    fn empty_measure_program_is_an_error() {
        let gate = "[[gates]]\nname = \"a\"\npaths = []\nhire = \"h\"\nmeasure = [\"\"]\n";
        assert!(matches!(
            ProjectConfig::parse(gate),
            Err(ProjectConfigError::EmptyMeasure(name)) if name == "a"
        ));
    }

    #[test]
    fn bad_globs_and_unknown_keys_are_errors() {
        assert!(matches!(
            ProjectConfig::parse("capture_paths = [\"src/[\"]"),
            Err(ProjectConfigError::Glob(..))
        ));
        assert!(ProjectConfig::parse("captur = []").is_err());
        let gate = "[[gates]]\nname = \"a\"\npaths = []\nhire = \"h\"\nextra = 1\n";
        assert!(ProjectConfig::parse(gate).is_err());
    }

    #[test]
    fn globs_match_changed_files() {
        let files = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let globs = files(&["src/**", "assets/**/*.png"]);
        assert!(matches_any(&globs, &files(&["README.md", "src/a/b.rs"])));
        assert!(matches_any(&globs, &files(&["assets/ui/icon.png"])));
        assert!(!matches_any(&globs, &files(&["README.md", "assets/a.txt"])));
        let store = files(&["**/store/**/*.md", "fastlane/**/*.md"]);
        assert!(matches_any(&store, &files(&["store/en/desc.md"])));
        assert!(matches_any(&store, &files(&["app/store/desc.md"])));
        assert!(matches_any(&store, &files(&["fastlane/metadata/en.md"])));
        assert!(!matches_any(&store, &files(&["docs/desc.md"])));
        assert!(!matches_any(&[], &files(&["a"])));
    }

    #[test]
    fn missing_config_is_none() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(ProjectConfig::load(tmp.path()).unwrap(), None);
    }
}
