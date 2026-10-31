use crate::error::{GitError, Result};
use std::path::Path;
use std::process::{Command, Output};

/// Runs `git -C dir args…` and returns the raw output, whatever the exit code.
pub(crate) fn output(dir: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new("git").arg("-C").arg(dir).args(args).output()?)
}

/// Runs git and returns stdout, or an error carrying stderr on a non-zero exit.
pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = output(dir, args)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failed(args, &out))
    }
}

pub(crate) fn failed(args: &[&str], out: &Output) -> GitError {
    GitError::Failed {
        args: args.join(" "),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    }
}

/// Paths of files with unresolved conflicts in `dir`.
pub(crate) fn conflicted_files(dir: &Path) -> Result<Vec<String>> {
    let names = run(dir, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(names.lines().map(str::to_string).collect())
}
