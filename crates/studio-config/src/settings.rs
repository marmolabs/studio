//! Changing single values in `studio.toml` the way a person would: only the
//! value's text changes, so every other line and comment stays as written.

use crate::config::StudioConfig;
use crate::paths::StudioPaths;
use std::fs;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("studio.toml was not found; run `studio init` first")]
    Missing,
    #[error("the change would leave studio.toml unreadable: {0}")]
    Unreadable(String),
    #[error("studio.toml has no [[projects]] entry named {0}")]
    NoProject(String),
}

/// A value to write: a whole number or a decimal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    Whole(u64),
    Decimal(f64),
}

impl Setting {
    fn text(self) -> String {
        match self {
            Setting::Whole(n) => n.to_string(),
            Setting::Decimal(x) => toml::Value::Float(x).to_string(),
        }
    }
}

/// Sets `[table] key = value` for each of `changes` in studio.toml, adding the
/// key or the table when it is missing. The result is checked to load as a
/// studio config before it replaces the file, which is swapped in whole so a
/// reader never sees half a write. A linked studio.toml has the file it links
/// to changed, and the file keeps its permissions and line endings.
pub fn set_values(
    paths: &StudioPaths,
    changes: &[(&str, &str, Setting)],
) -> Result<(), SettingsError> {
    rewrite(paths, |text| {
        let mut text = text.to_string();
        for (table, key, value) in changes {
            text = with_value(&text, table, key, &value.text());
        }
        Ok(text)
    })
}

/// Sets `lead = "<hire>"` in the `[[projects]]` entry named `project`, or
/// removes it when `lead` is none. Every other line and comment stays, and
/// the result is checked and swapped in as `set_values` does.
pub fn set_project_lead(
    paths: &StudioPaths,
    project: &str,
    lead: Option<&str>,
) -> Result<(), SettingsError> {
    rewrite(paths, |text| {
        let text = with_entry_value(text, project, "lead", lead)
            .ok_or_else(|| SettingsError::NoProject(project.to_string()))?;
        let config =
            StudioConfig::parse(&text).map_err(|e| SettingsError::Unreadable(e.to_string()))?;
        let now = config
            .projects
            .iter()
            .find(|p| p.name == project)
            .and_then(|p| p.lead.as_deref());
        if now != lead {
            return Err(SettingsError::Unreadable(format!(
                "the lead of {project} could not be written there"
            )));
        }
        Ok(text)
    })
}

/// Sets `daily_usd = <usd>` in the `[[projects]]` entry named `project`, or
/// removes it when `usd` is none. Every other line and comment stays, and the
/// result is checked and swapped in as `set_values` does.
pub fn set_project_daily_usd(
    paths: &StudioPaths,
    project: &str,
    usd: Option<f64>,
) -> Result<(), SettingsError> {
    rewrite(paths, |text| {
        let value = usd.map(|usd| toml::Value::Float(usd).to_string());
        let text = with_entry_raw(text, project, "daily_usd", value.as_deref())
            .ok_or_else(|| SettingsError::NoProject(project.to_string()))?;
        let config =
            StudioConfig::parse(&text).map_err(|e| SettingsError::Unreadable(e.to_string()))?;
        let now = config
            .projects
            .iter()
            .find(|p| p.name == project)
            .and_then(|p| p.daily_usd);
        if now != usd {
            return Err(SettingsError::Unreadable(format!(
                "the daily limit of {project} could not be written there"
            )));
        }
        Ok(text)
    })
}

/// Reads studio.toml, lets `change` make the new text, checks that it loads
/// and swaps it in whole.
fn rewrite(
    paths: &StudioPaths,
    change: impl FnOnce(&str) -> Result<String, SettingsError>,
) -> Result<(), SettingsError> {
    let path = paths.config_dir.join("studio.toml");
    let path = match fs::canonicalize(&path) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SettingsError::Missing),
        Err(e) => return Err(e.into()),
    };
    let before = fs::read_to_string(&path)?;
    let mut text = change(&before)?;
    if before.contains("\r\n") {
        text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if let Err(e) = StudioConfig::parse(&text) {
        return Err(SettingsError::Unreadable(e.to_string()));
    }
    let staging = path.with_extension("toml.tmp");
    fs::write(&staging, &text)?;
    fs::set_permissions(&staging, fs::metadata(&path)?.permissions())?;
    fs::rename(&staging, &path)?;
    Ok(())
}

/// `text` with `key` in `[table]` set to `value`.
fn with_value(text: &str, table: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = format!("[{table}]");
    let Some(start) = lines.iter().position(|l| is_header(l, &header)) else {
        let mut out = text.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("{header}\n{key} = {value}\n"));
        return out;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |n| start + 1 + n);
    match (start + 1..end).find(|&i| names_key(&lines[i], key)) {
        Some(i) => lines[i] = replaced(&lines[i], value),
        None => lines.insert(start + 1, format!("{key} = {value}")),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `text` with `key` set to the string `value` (or removed, for none) in the
/// `[[projects]]` entry named `project`; none when no entry has that name.
fn with_entry_value(text: &str, project: &str, key: &str, value: Option<&str>) -> Option<String> {
    with_entry_raw(text, project, key, value.map(quoted).as_deref())
}

/// As `with_entry_value`, with `value` already written as TOML.
fn with_entry_raw(text: &str, project: &str, key: &str, value: Option<&str>) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut found = None;
    for at in 0..lines.len() {
        if !is_header(&lines[at], "[[projects]]") {
            continue;
        }
        let end = lines[at + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |n| at + 1 + n);
        let named = (at + 1..end).find(|&i| {
            names_key(&lines[i], "name")
                && string_after_equals(&lines[i]).as_deref() == Some(project)
        });
        if let Some(named) = named {
            found = Some((named, (at + 1..end).find(|&i| names_key(&lines[i], key))));
            break;
        }
    }
    let (named, existing) = found?;
    match (existing, value) {
        (Some(i), Some(value)) => lines[i] = replaced(&lines[i], value),
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(value)) => lines.insert(named + 1, format!("{key} = {value}")),
        (None, None) => {}
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Some(out)
}

/// `value` as a TOML string.
fn quoted(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// The string that a `key = "value" # comment` line assigns.
fn string_after_equals(line: &str) -> Option<String> {
    let (_, value) = line.split_once('=')?;
    let table: toml::Table = toml::from_str(&format!("v = {}", value.trim())).ok()?;
    table.get("v")?.as_str().map(str::to_string)
}

/// Whether `line` is the table header `header`, with or without a comment.
fn is_header(line: &str, header: &str) -> bool {
    line.trim().strip_prefix(header).is_some_and(|rest| {
        let rest = rest.trim_start();
        rest.is_empty() || rest.starts_with('#')
    })
}

/// Whether `line` assigns `key`.
fn names_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `line` with the text after `=` swapped for `value`, keeping the indent and
/// any trailing comment.
fn replaced(line: &str, value: &str) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    let (name, rest) = line.split_once('=').unwrap_or((line, ""));
    let comment = rest
        .find('#')
        .map(|at| format!(" {}", rest[at..].trim_end()))
        .unwrap_or_default();
    format!("{indent}{} = {value}{comment}", name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn home(text: &str) -> (TempDir, StudioPaths) {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("studio.toml"), text).unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        (tmp, paths)
    }

    fn read(paths: &StudioPaths) -> String {
        fs::read_to_string(paths.config_dir.join("studio.toml")).unwrap()
    }

    const BASE: &str = "# my studio\n[budget]\ndaily_usd = 25.0 # per day\n\n\
                        [concurrency]\nmax_runs = 2\n\n[[projects]]\nname = \"g\"\npath = \"/g\"\n";

    #[test]
    fn a_value_changes_and_everything_else_stays() {
        let (_tmp, paths) = home(BASE);
        set_values(&paths, &[("concurrency", "max_runs", Setting::Whole(5))]).unwrap();
        assert_eq!(read(&paths), BASE.replace("max_runs = 2", "max_runs = 5"));
        set_values(&paths, &[("budget", "daily_usd", Setting::Decimal(40.0))]).unwrap();
        assert!(read(&paths).contains("daily_usd = 40.0 # per day\n"));
        assert!(read(&paths).starts_with("# my studio\n"));
    }

    #[test]
    fn a_missing_key_is_added_to_its_table() {
        let (_tmp, paths) = home(BASE);
        set_values(&paths, &[("budget", "ticket_usd", Setting::Decimal(1.5))]).unwrap();
        let text = read(&paths);
        assert!(text.contains("[budget]\nticket_usd = 1.5\ndaily_usd = 25.0 # per day\n"));
        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.budget.ticket_usd, 1.5);
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn a_missing_table_is_added_at_the_end() {
        let (_tmp, paths) = home("[budget]\ndaily_usd = 1.0\n");
        set_values(&paths, &[("concurrency", "max_runs", Setting::Whole(3))]).unwrap();
        assert_eq!(
            read(&paths),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 3\n"
        );
        assert_eq!(StudioConfig::load(&paths).unwrap().concurrency.max_runs, 3);
    }

    #[test]
    fn indented_keys_and_commented_headers_are_found() {
        let (_tmp, paths) =
            home("[budget] # money\n  daily_usd = 1.0\n[concurrency]\nmax_runs = 1\n");
        set_values(&paths, &[("budget", "daily_usd", Setting::Decimal(9.5))]).unwrap();
        assert_eq!(
            read(&paths),
            "[budget] # money\n  daily_usd = 9.5\n[concurrency]\nmax_runs = 1\n"
        );
    }

    #[test]
    fn a_change_that_breaks_the_file_is_refused_and_leaves_it_alone() {
        // No [concurrency] table, so the file never loaded; adding a budget
        // key doesn't fix that and nothing is written.
        let (_tmp, paths) = home("[budget]\ndaily_usd = 1.0\n");
        let err = set_values(&paths, &[("budget", "ticket_usd", Setting::Decimal(2.0))]);
        assert!(matches!(err, Err(SettingsError::Unreadable(_))));
        assert_eq!(read(&paths), "[budget]\ndaily_usd = 1.0\n");
    }

    #[test]
    fn line_endings_and_permissions_stay_as_they_were() {
        let (_tmp, paths) = home(&BASE.replace('\n', "\r\n"));
        let file = paths.config_dir.join("studio.toml");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        set_values(&paths, &[("concurrency", "max_runs", Setting::Whole(4))]).unwrap();
        assert_eq!(
            read(&paths),
            BASE.replace("max_runs = 2", "max_runs = 4")
                .replace('\n', "\r\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_studio_toml_changes_the_file_it_links_to() {
        let (tmp, paths) = home(BASE);
        let kept = tmp.path().join("dotfiles.toml");
        let file = paths.config_dir.join("studio.toml");
        fs::rename(&file, &kept).unwrap();
        std::os::unix::fs::symlink(&kept, &file).unwrap();
        set_values(&paths, &[("concurrency", "max_runs", Setting::Whole(3))]).unwrap();
        assert!(
            fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(fs::read_to_string(&kept).unwrap().contains("max_runs = 3"));
    }

    const TWO: &str = "# my studio\n[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 1\n\n\
                       # the game\n[[projects]]\nname = \"game\" # main\npath = \"/g\"\n\n\
                       [[projects]]\nname = \"site\"\npath = \"/s\"\nlead = \"old\" # was\n";

    fn lead_of(paths: &StudioPaths, project: &str) -> Option<String> {
        let config = StudioConfig::load(paths).unwrap();
        config
            .projects
            .iter()
            .find(|p| p.name == project)
            .and_then(|p| p.lead.clone())
    }

    #[test]
    fn a_lead_is_added_to_the_named_project_and_nothing_else_changes() {
        let (_tmp, paths) = home(TWO);
        set_project_lead(&paths, "game", Some("boss")).unwrap();
        assert_eq!(
            read(&paths),
            TWO.replace(
                "name = \"game\" # main\n",
                "name = \"game\" # main\nlead = \"boss\"\n"
            )
        );
        assert_eq!(lead_of(&paths, "game").as_deref(), Some("boss"));
        assert_eq!(lead_of(&paths, "site").as_deref(), Some("old"));
    }

    #[test]
    fn a_lead_is_replaced_keeping_its_comment_and_removed_with_its_line() {
        let (_tmp, paths) = home(TWO);
        set_project_lead(&paths, "site", Some("new")).unwrap();
        assert_eq!(
            read(&paths),
            TWO.replace("lead = \"old\" # was", "lead = \"new\" # was")
        );
        set_project_lead(&paths, "site", None).unwrap();
        assert_eq!(read(&paths), TWO.replace("lead = \"old\" # was\n", ""));
        assert_eq!(lead_of(&paths, "site"), None);
        // Clearing a lead that is not there changes nothing.
        set_project_lead(&paths, "game", None).unwrap();
        assert_eq!(read(&paths), TWO.replace("lead = \"old\" # was\n", ""));
    }

    #[test]
    fn a_projects_daily_limit_is_written_to_its_entry_only() {
        let (_tmp, paths) = home(TWO);
        set_project_daily_usd(&paths, "game", Some(3.5)).unwrap();
        assert_eq!(
            read(&paths),
            TWO.replace(
                "name = \"game\" # main\n",
                "name = \"game\" # main\ndaily_usd = 3.5\n"
            )
        );
        let config = StudioConfig::load(&paths).unwrap();
        assert_eq!(config.projects[0].daily_usd, Some(3.5));
        assert_eq!(config.projects[1].daily_usd, None);
        // A limit already there is replaced in place, comment and all.
        set_project_daily_usd(&paths, "game", Some(6.0)).unwrap();
        assert!(read(&paths).contains("daily_usd = 6.0\n"));
        assert!(!read(&paths).contains("3.5"));
        set_project_daily_usd(&paths, "game", None).unwrap();
        assert_eq!(read(&paths), TWO);
        let err = set_project_daily_usd(&paths, "nope", Some(1.0));
        assert!(matches!(err, Err(SettingsError::NoProject(_))));
    }

    #[test]
    fn an_unknown_project_is_refused_and_the_file_left_alone() {
        let (_tmp, paths) = home(TWO);
        let err = set_project_lead(&paths, "nope", Some("boss"));
        assert!(matches!(err, Err(SettingsError::NoProject(name)) if name == "nope"));
        assert_eq!(read(&paths), TWO);
    }

    #[test]
    fn a_lead_keeps_crlf_line_endings() {
        let (_tmp, paths) = home(&TWO.replace('\n', "\r\n"));
        set_project_lead(&paths, "game", Some("boss")).unwrap();
        assert!(read(&paths).contains("name = \"game\" # main\r\nlead = \"boss\"\r\n"));
        assert!(!read(&paths).replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn no_studio_toml_is_reported() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        let err = set_values(&paths, &[("budget", "daily_usd", Setting::Decimal(1.0))]);
        assert!(matches!(err, Err(SettingsError::Missing)));
    }
}
