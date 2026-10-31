//! The project's own checks, run in the ticket's worktree.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MEASURE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const OUTPUT_LIMIT: usize = 4000;
const MEASURE_LIMIT: usize = 2000;

/// Runs one check; a failure gives its command line and the tail of its
/// output.
pub(crate) async fn run(dir: &Path, command: &[String]) -> Result<(), String> {
    exec(dir, command, TIMEOUT).await
}

/// Runs the game's capture command, with `{out}` in its arguments replaced by
/// the folder to write into.
pub(crate) async fn capture(dir: &Path, command: &[String], out: &Path) -> Result<(), String> {
    let out = out.to_string_lossy();
    let command: Vec<String> = command.iter().map(|a| a.replace("{out}", &out)).collect();
    exec(dir, &command, CAPTURE_TIMEOUT).await
}

/// Runs a gate's measure command, with `{file}` in its arguments replaced by
/// `file` when given. Returns the command line and its output for the critic.
/// A failure is part of the result, never an error.
pub(crate) async fn measure(dir: &Path, command: &[String], file: Option<&str>) -> String {
    let command: Vec<String> = match file {
        Some(file) => command.iter().map(|a| a.replace("{file}", file)).collect(),
        None => command.to_vec(),
    };
    let line = command.join(" ");
    let out = match output(dir, &command, MEASURE_TIMEOUT).await {
        Ok(Some(out)) => out,
        Ok(None) => return format!("{line} could not start: no command"),
        Err(why) => return format!("{line} {why}"),
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let shown: String = text.chars().take(MEASURE_LIMIT).collect();
    let mut result = format!("$ {line}\n{shown}");
    if let Some(code) = out.status.code().filter(|c| *c != 0) {
        result.push_str(&format!("\n(exit {code})"));
    } else if !out.status.success() {
        result.push_str("\n(exit signal)");
    }
    result
}

async fn exec(dir: &Path, command: &[String], timeout: Duration) -> Result<(), String> {
    let line = command.join(" ");
    let Some(out) = output(dir, command, timeout)
        .await
        .map_err(|why| format!("`{line}` {why}"))?
    else {
        return Ok(());
    };
    if !out.status.success() {
        let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
        output.push_str(&String::from_utf8_lossy(&out.stderr));
        return Err(format!("`{line}` failed:\n{}", tail(&output)));
    }
    Ok(())
}

/// Runs the command to the end. `None` for an empty command; the error says
/// "timed out" or "could not start: ...".
async fn output(
    dir: &Path,
    command: &[String],
    timeout: Duration,
) -> Result<Option<std::process::Output>, String> {
    let Some((program, args)) = command.split_first() else {
        return Ok(None);
    };
    let child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(timeout, child).await {
        Err(_) => Err("timed out".to_string()),
        Ok(Err(err)) => Err(format!("could not start: {err}")),
        Ok(Ok(out)) => Ok(Some(out)),
    }
}

fn tail(text: &str) -> &str {
    let start = text.len().saturating_sub(OUTPUT_LIMIT);
    let start = (start..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    &text[start..]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        ["sh", "-c", script, "sh", "{file}"]
            .map(String::from)
            .to_vec()
    }

    #[tokio::test]
    async fn measure_returns_the_command_and_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = measure(dir.path(), &sh("echo \"hi $1\""), Some("a.png")).await;
        assert_eq!(out, "$ sh -c echo \"hi $1\" sh a.png\nhi a.png\n");
    }

    #[tokio::test]
    async fn measure_notes_a_failing_exit_and_keeps_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let out = measure(dir.path(), &sh("echo out; echo err >&2; exit 3"), None).await;
        assert!(out.contains("out\nerr\n"));
        assert!(out.ends_with("(exit 3)"));
    }

    #[tokio::test]
    async fn measure_reports_a_command_that_cannot_start() {
        let dir = tempfile::tempdir().unwrap();
        let command = vec!["no-such-program-here".to_string(), "x".to_string()];
        let out = measure(dir.path(), &command, None).await;
        assert!(out.starts_with("no-such-program-here x could not start: "));
    }

    #[tokio::test]
    async fn measure_cuts_output_to_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let out = measure(dir.path(), &sh("yes é | head -c 5000"), None).await;
        let body = out.split_once('\n').unwrap().1;
        assert_eq!(body.chars().count(), MEASURE_LIMIT);
    }
}
