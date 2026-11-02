//! Writing a project's checks to its `.studio.toml` the way a person would:
//! only the lines of `verify`, `capture`, `release` and `protected` change, so
//! every comment and every other key stays as written.

use crate::project::{ProjectConfig, ProjectConfigError};
use std::fs;
use std::path::{Path, PathBuf};

/// How a project runs, as far as `.studio.toml` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    /// The checks that show a change works, each a program with its arguments.
    pub verify: Vec<Vec<String>>,
    /// The capture command; empty for none.
    pub capture: Vec<String>,
    /// The commands that release the project, in order.
    pub release: Vec<Vec<String>>,
    /// The globs an agent asks about before touching them.
    pub protected: Vec<String>,
}

/// The text `.studio.toml` would have with `checks` in it: `existing` (none
/// when the project has no file) with `verify`, `capture`, `release` and
/// `protected` set. A key the file has is replaced where it stands, keeping its
/// trailing comment; one it lacks is added after the file's other top-level
/// keys, unless it would be empty. A file that doesn't load is an error and is
/// never written over.
pub fn with_checks(existing: Option<&str>, checks: &Checks) -> Result<String, ProjectConfigError> {
    check_choice(checks)?;
    let before = existing.unwrap_or_default();
    if existing.is_some() {
        ProjectConfig::parse(before)?;
    }
    let crlf = before.contains("\r\n");
    let plain = before.replace("\r\n", "\n");
    let mut lines: Vec<String> = match existing {
        Some(_) => plain.lines().map(str::to_string).collect(),
        None => HEADER.lines().map(str::to_string).collect(),
    };

    let wanted = [
        (
            "verify",
            list_of_lists("verify", &checks.verify),
            checks.verify.is_empty(),
        ),
        (
            "capture",
            list("capture", &checks.capture),
            checks.capture.is_empty(),
        ),
        (
            "release",
            list_of_lists("release", &checks.release),
            checks.release.is_empty(),
        ),
        (
            "protected",
            list("protected", &checks.protected),
            checks.protected.is_empty(),
        ),
    ];
    let mut added: Vec<String> = Vec::new();
    for (key, rendered, empty) in wanted {
        let found = statements(&lines);
        match found.statements.iter().find(|s| s.key == key) {
            Some(at) => {
                let mut rendered = rendered;
                if let (Some(comment), Some(last)) = (&at.comment, rendered.last_mut()) {
                    last.push_str(&format!(" {comment}"));
                }
                lines.splice(at.first..=at.last, rendered);
            }
            None if !empty => added.extend(rendered),
            None => {}
        }
    }
    if !added.is_empty() {
        let found = statements(&lines);
        let (at, before_table) = match (found.statements.last(), found.first_table) {
            (Some(last), _) => (last.last + 1, false),
            (None, Some(table)) => (comment_block_start(&lines, table), true),
            (None, None) => (lines.len(), false),
        };
        let gap = lines.get(at).is_some_and(|next| !next.trim().is_empty());
        if gap || before_table {
            added.push(String::new());
        }
        lines.splice(at..at, added);
    }

    let mut text = lines.join("\n");
    text.push('\n');
    if crlf {
        text = text.replace('\n', "\r\n");
    }
    let parsed = ProjectConfig::parse(&text)?;
    let same = parsed.verify == checks.verify
        && parsed.capture == checks.capture
        && parsed.release == checks.release
        && parsed.protected == checks.protected;
    if !same {
        return Err(ProjectConfigError::Invalid(
            "the checks could not be written into .studio.toml".to_string(),
        ));
    }
    Ok(text)
}

/// Writes `checks` into `<project>/.studio.toml` as `with_checks` says, making
/// the file when there is none. The file is swapped in whole, so a reader never
/// sees half a write; a linked file has the file it links to changed, and keeps
/// its permissions. Returns the file's path.
pub fn write_checks(project: &Path, checks: &Checks) -> Result<PathBuf, ProjectConfigError> {
    let path = project.join(".studio.toml");
    let (target, existing) = match fs::canonicalize(&path) {
        Ok(target) => {
            let text = fs::read_to_string(&target)?;
            (target, Some(text))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (path.clone(), None),
        Err(e) => return Err(e.into()),
    };
    let text = with_checks(existing.as_deref(), checks)?;
    let staging = target.with_extension("toml.tmp");
    fs::write(&staging, &text)?;
    if existing.is_some() {
        fs::set_permissions(&staging, fs::metadata(&target)?.permissions())?;
    }
    fs::rename(&staging, &target)?;
    Ok(path)
}

/// What a new file starts with.
const HEADER: &str = "# How this project runs. Each command is a program with its arguments and\n\
                      # runs in a fresh checkout; a change lands only when every check passes.";

/// Refuses what could never run: a command with no program, a glob that
/// doesn't compile.
fn check_choice(checks: &Checks) -> Result<(), ProjectConfigError> {
    let blank = |command: &[String]| {
        command
            .first()
            .is_none_or(|program| program.trim().is_empty())
    };
    for (what, commands) in [
        ("check", &checks.verify),
        ("release command", &checks.release),
    ] {
        if commands.iter().any(|command| blank(command)) {
            return Err(ProjectConfigError::Invalid(format!(
                "a {what} has no program to run"
            )));
        }
    }
    if !checks.capture.is_empty() && blank(&checks.capture) {
        return Err(ProjectConfigError::Invalid(
            "the capture command has no program to run".to_string(),
        ));
    }
    for glob in &checks.protected {
        globset::Glob::new(glob)
            .map_err(|e| ProjectConfigError::Glob(glob.clone(), e.to_string()))?;
    }
    Ok(())
}

fn quoted(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn inline(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|item| quoted(item)).collect();
    format!("[{}]", items.join(", "))
}

/// `key = ["a", "b"]`, on one line unless that would be long.
fn list(key: &str, items: &[String]) -> Vec<String> {
    let line = format!("{key} = {}", inline(items));
    if line.chars().count() <= 100 {
        return vec![line];
    }
    let mut lines = vec![format!("{key} = [")];
    lines.extend(items.iter().map(|item| format!("  {},", quoted(item))));
    lines.push("]".to_string());
    lines
}

/// `key = [["a", "b"], ["c"]]`, a command to a line.
fn list_of_lists(key: &str, commands: &[Vec<String>]) -> Vec<String> {
    if commands.is_empty() {
        return vec![format!("{key} = []")];
    }
    let mut lines = vec![format!("{key} = [")];
    lines.extend(
        commands
            .iter()
            .map(|command| format!("  {},", inline(command))),
    );
    lines.push("]".to_string());
    lines
}

/// A `key = value` at the top of the file, over lines `first` to `last`.
struct Statement {
    key: String,
    first: usize,
    last: usize,
    /// The comment after the value, with its `#`.
    comment: Option<String>,
}

struct Found {
    statements: Vec<Statement>,
    /// The line of the first table header, if the file has one.
    first_table: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    Basic,
    Literal,
    MultiBasic,
    MultiLiteral,
}

/// The top-level `key = value` statements before the first table header. The
/// file is known to be valid TOML, so a value ends where its brackets close.
fn statements(lines: &[String]) -> Found {
    let mut found = Found {
        statements: Vec::new(),
        first_table: None,
    };
    let mut open: Option<Statement> = None;
    let mut depth = 0i32;
    let mut quote: Option<Quote> = None;
    for (at, line) in lines.iter().enumerate() {
        if open.is_none() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') {
                found.first_table = Some(at);
                break;
            }
            let key = line
                .split_once('=')
                .map(|(key, _)| key.trim().trim_matches(['"', '\'']).to_string())
                .unwrap_or_default();
            open = Some(Statement {
                key,
                first: at,
                last: at,
                comment: None,
            });
        }
        let mut comment = None;
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let rest = |n: usize| chars.get(i + n).copied();
            match quote {
                Some(Quote::Basic) => match c {
                    '\\' => i += 1,
                    '"' => quote = None,
                    _ => {}
                },
                Some(Quote::Literal) => {
                    if c == '\'' {
                        quote = None;
                    }
                }
                Some(Quote::MultiBasic) => match c {
                    '\\' => i += 1,
                    '"' if rest(1) == Some('"') && rest(2) == Some('"') => {
                        quote = None;
                        i += 2;
                    }
                    _ => {}
                },
                Some(Quote::MultiLiteral) => {
                    if c == '\'' && rest(1) == Some('\'') && rest(2) == Some('\'') {
                        quote = None;
                        i += 2;
                    }
                }
                None => match c {
                    '#' => {
                        comment =
                            Some(chars[i..].iter().collect::<String>().trim_end().to_string());
                        break;
                    }
                    '"' if rest(1) == Some('"') && rest(2) == Some('"') => {
                        quote = Some(Quote::MultiBasic);
                        i += 2;
                    }
                    '\'' if rest(1) == Some('\'') && rest(2) == Some('\'') => {
                        quote = Some(Quote::MultiLiteral);
                        i += 2;
                    }
                    '"' => quote = Some(Quote::Basic),
                    '\'' => quote = Some(Quote::Literal),
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    _ => {}
                },
            }
            i += 1;
        }
        // A single-line string never runs on to the next line.
        if matches!(quote, Some(Quote::Basic | Quote::Literal)) {
            quote = None;
        }
        if depth <= 0 && quote.is_none() {
            depth = 0;
            if let Some(mut statement) = open.take() {
                statement.last = at;
                statement.comment = comment;
                found.statements.push(statement);
            }
        }
    }
    found
}

/// The first line of the run of comment lines that ends just above `line`;
/// `line` itself when there are none.
fn comment_block_start(lines: &[String], line: usize) -> usize {
    let mut start = line;
    while start > 0 && lines[start - 1].trim_start().starts_with('#') {
        start -= 1;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn command(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn checks() -> Checks {
        Checks {
            verify: vec![command(&["make", "lint"]), command(&["make", "test"])],
            capture: command(&["make", "shots", "{out}"]),
            release: vec![command(&["./publish.sh"])],
            protected: vec!["migrations/**".into(), "release.toml".into()],
        }
    }

    #[test]
    fn a_new_file_holds_the_checks_and_loads() {
        let text = with_checks(None, &checks()).unwrap();
        assert!(text.starts_with("# How this project runs."));
        assert!(text.contains("verify = [\n  [\"make\", \"lint\"],\n  [\"make\", \"test\"],\n]\n"));
        assert!(text.contains("capture = [\"make\", \"shots\", \"{out}\"]\n"));
        assert!(text.contains("protected = [\"migrations/**\", \"release.toml\"]\n"));
        let config = ProjectConfig::parse(&text).unwrap();
        assert_eq!(config.verify, checks().verify);
        assert_eq!(config.release, checks().release);
    }

    const OLD: &str = "# Checks for the shop.\n\
        # Edit with care.\n\
        verify = [[\"cargo\", \"test\"]] # the old check\n\
        auto_land = true\n\
        capture = [\n  \"old\", # the old capture\n  \"{out}\",\n]\n\
        \n\
        # Where the cache lives.\n\
        [env]\n\
        CACHE = \"{repo}/.cache\"\n\
        \n\
        [[gates]]\n\
        name = \"art\"\n\
        paths = [\"art/**\"]\n\
        hire = \"critic\"\n";

    #[test]
    fn keys_in_the_file_are_replaced_in_place_and_everything_else_stays() {
        let wanted = Checks {
            verify: vec![command(&["make", "test"])],
            capture: Vec::new(),
            release: Vec::new(),
            protected: vec!["secrets/**".into()],
        };
        let text = with_checks(Some(OLD), &wanted).unwrap();
        let expected = "# Checks for the shop.\n\
            # Edit with care.\n\
            verify = [\n  [\"make\", \"test\"],\n] # the old check\n\
            auto_land = true\n\
            capture = []\n\
            protected = [\"secrets/**\"]\n\
            \n\
            # Where the cache lives.\n\
            [env]\n\
            CACHE = \"{repo}/.cache\"\n\
            \n\
            [[gates]]\n\
            name = \"art\"\n\
            paths = [\"art/**\"]\n\
            hire = \"critic\"\n";
        assert_eq!(text, expected);
        let config = ProjectConfig::parse(&text).unwrap();
        assert!(config.auto_land);
        assert_eq!(config.gates.len(), 1);
        assert_eq!(config.env["CACHE"], "{repo}/.cache");
        assert!(config.capture.is_empty());
    }

    #[test]
    fn a_multi_line_value_is_replaced_whole() {
        let text = with_checks(Some(OLD), &checks()).unwrap();
        assert!(!text.contains("\"old\""));
        assert!(!text.contains("the old capture"));
        // release was missing, so it is added with the other new keys.
        let config = ProjectConfig::parse(&text).unwrap();
        assert_eq!(config.capture, checks().capture);
        assert_eq!(config.release, checks().release);
        assert_eq!(config.protected, checks().protected);
        // New keys follow the last top-level key and leave a blank line before
        // the comment of the table.
        let added = text.find("release = [").unwrap();
        let table = text.find("# Where the cache lives.").unwrap();
        assert!(added < table);
        assert!(text.contains(
            "protected = [\"migrations/**\", \"release.toml\"]\n\n# Where the cache lives.\n[env]"
        ));
    }

    #[test]
    fn keys_a_file_without_top_level_keys_lacks_go_above_its_first_table() {
        let old = "# the cache\n[env]\nA = \"1\"\n";
        let wanted = Checks {
            verify: vec![command(&["true"])],
            ..Checks::default()
        };
        let text = with_checks(Some(old), &wanted).unwrap();
        assert_eq!(
            text,
            "verify = [\n  [\"true\"],\n]\n\n# the cache\n[env]\nA = \"1\"\n"
        );
    }

    #[test]
    fn empty_keys_the_file_lacks_are_not_added() {
        let text = with_checks(Some("auto_land = true\n"), &Checks::default()).unwrap();
        assert_eq!(text, "auto_land = true\n");
        let wanted = Checks {
            verify: vec![command(&["true"])],
            ..Checks::default()
        };
        let text = with_checks(Some(""), &wanted).unwrap();
        assert_eq!(text, "verify = [\n  [\"true\"],\n]\n");
    }

    #[test]
    fn strings_and_brackets_inside_values_do_not_end_them() {
        let old = "verify = [\n  [\"sh\", \"-c\", \"test ] && echo '#'\"],\n  ['a', 'b]'],\n]\nprotected = [\"x\"]\n";
        let wanted = Checks {
            verify: vec![command(&["true"])],
            protected: vec!["y".into()],
            ..Checks::default()
        };
        let text = with_checks(Some(old), &wanted).unwrap();
        assert_eq!(text, "verify = [\n  [\"true\"],\n]\nprotected = [\"y\"]\n");
    }

    #[test]
    fn crlf_line_endings_stay() {
        let old = "verify = []\r\nauto_land = true\r\n";
        let wanted = Checks {
            verify: vec![command(&["true"])],
            ..Checks::default()
        };
        let text = with_checks(Some(old), &wanted).unwrap();
        assert_eq!(
            text,
            "verify = [\r\n  [\"true\"],\r\n]\r\nauto_land = true\r\n"
        );
    }

    #[test]
    fn a_choice_that_could_not_run_is_refused() {
        let blank = Checks {
            verify: vec![command(&[" "])],
            ..Checks::default()
        };
        assert!(matches!(
            with_checks(None, &blank),
            Err(ProjectConfigError::Invalid(_))
        ));
        let no_program = Checks {
            release: vec![Vec::new()],
            ..Checks::default()
        };
        assert!(with_checks(None, &no_program).is_err());
        let bad_glob = Checks {
            protected: vec!["src/[".into()],
            ..Checks::default()
        };
        assert!(matches!(
            with_checks(None, &bad_glob),
            Err(ProjectConfigError::Glob(..))
        ));
    }

    #[test]
    fn a_file_that_does_not_load_is_not_written_over() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".studio.toml");
        fs::write(&path, "verify = nonsense\n").unwrap();
        assert!(write_checks(tmp.path(), &checks()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "verify = nonsense\n");
    }

    #[test]
    fn writing_makes_the_file_or_changes_the_one_there() {
        let tmp = TempDir::new().unwrap();
        let path = write_checks(tmp.path(), &checks()).unwrap();
        assert_eq!(path, tmp.path().join(".studio.toml"));
        let config = ProjectConfig::load(tmp.path()).unwrap().unwrap();
        assert_eq!(config.verify, checks().verify);

        fs::write(&path, OLD).unwrap();
        write_checks(tmp.path(), &checks()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Checks for the shop.\n# Edit with care.\n"));
        assert!(text.contains("# Where the cache lives.\n[env]"));
        assert_eq!(
            ProjectConfig::load(tmp.path()).unwrap().unwrap().verify,
            checks().verify
        );
        assert!(!tmp.path().join(".studio.toml.tmp").exists());
    }
}
