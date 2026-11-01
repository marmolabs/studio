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
    let path = paths.config_dir.join("studio.toml");
    let path = match fs::canonicalize(&path) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SettingsError::Missing),
        Err(e) => return Err(e.into()),
    };
    let before = fs::read_to_string(&path)?;
    let mut text = before.clone();
    for (table, key, value) in changes {
        text = with_value(&text, table, key, &value.text());
    }
    if before.contains("\r\n") {
        text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if let Err(e) = toml::from_str::<StudioConfig>(&text) {
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

    #[test]
    fn no_studio_toml_is_reported() {
        let tmp = TempDir::new().unwrap();
        let paths = StudioPaths::resolve(Some(tmp.path().to_path_buf()));
        let err = set_values(&paths, &[("budget", "daily_usd", Setting::Decimal(1.0))]);
        assert!(matches!(err, Err(SettingsError::Missing)));
    }
}
