use crate::attribution::strip_attribution;
use crate::cmd::{conflicted_files, failed, output, run};
use crate::error::{GitError, Result};
use crate::repo::{base_key, resolve_in};
use std::path::PathBuf;

/// A ticket's checkout: its own branch, started from a pinned base commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebaseOutcome {
    Clean { new_base: String },
    Conflict { files: Vec<String> },
}

impl Worktree {
    /// Files that differ from the base commit, including untracked ones.
    pub fn changed_files(&self) -> Result<Vec<PathBuf>> {
        let tracked = run(&self.path, &["diff", "--name-only", &self.base_commit])?;
        let mut files: Vec<PathBuf> = tracked
            .lines()
            .chain(self.untracked()?.iter().map(String::as_str))
            .map(PathBuf::from)
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// The file's bytes at the base commit, or `None` if it did not exist there.
    pub fn file_at_base(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{}:{path}", self.base_commit);
        if !output(&self.path, &["cat-file", "-e", &spec])?
            .status
            .success()
        {
            return Ok(None);
        }
        let args = ["show", spec.as_str()];
        let out = output(&self.path, &args)?;
        if !out.status.success() {
            return Err(failed(&args, &out));
        }
        Ok(Some(out.stdout))
    }

    /// `git diff --stat` against the base commit.
    pub fn diff_stat(&self) -> Result<String> {
        run(&self.path, &["diff", "--stat", &self.base_commit])
    }

    /// Unified diff against the base commit, with untracked files shown as added.
    pub fn diff(&self) -> Result<String> {
        let mut diff = run(&self.path, &["diff", &self.base_commit])?;
        for file in self.untracked()? {
            let args = ["diff", "--no-index", "--", "/dev/null", file.as_str()];
            let out = output(&self.path, &args)?;
            // `--no-index` exits 1 when the files differ, which is the expected case.
            if out.status.code() != Some(1) && !out.status.success() {
                return Err(failed(&args, &out));
            }
            diff.push_str(&String::from_utf8_lossy(&out.stdout));
        }
        Ok(diff)
    }

    /// Stages everything and commits with the repo's identity. Hooks run as normal.
    /// Returns `None` when there is nothing to commit.
    pub fn commit(&self, message: &str) -> Result<Option<String>> {
        run(&self.path, &["add", "-A"])?;
        let staged = output(&self.path, &["diff", "--cached", "--quiet"])?;
        if staged.status.success() {
            return Ok(None);
        }
        let message = strip_attribution(message);
        if message.trim().is_empty() {
            return Err(GitError::Parse("commit message is empty".to_string()));
        }
        run(&self.path, &["commit", "-m", &message])?;
        Ok(Some(resolve_in(&self.path, "HEAD")?))
    }

    /// Replays the ticket's commits (and uncommitted work) onto `rev`. On conflict
    /// the rebase is aborted and the worktree is left as it was.
    pub fn rebase_onto(&mut self, rev: &str) -> Result<RebaseOutcome> {
        let new_base = resolve_in(&self.path, rev)?;
        let args = [
            "rebase",
            "--autostash",
            "--onto",
            new_base.as_str(),
            self.base_commit.as_str(),
        ];
        let out = output(&self.path, &args)?;
        if !out.status.success() {
            let files = conflicted_files(&self.path)?;
            run(&self.path, &["rebase", "--abort"])?;
            if files.is_empty() {
                return Err(failed(&args, &out));
            }
            return Ok(RebaseOutcome::Conflict { files });
        }
        run(&self.path, &["config", &base_key(&self.branch), &new_base])?;
        self.base_commit = new_base.clone();
        Ok(RebaseOutcome::Clean { new_base })
    }

    fn untracked(&self) -> Result<Vec<String>> {
        let listing = run(&self.path, &["ls-files", "--others", "--exclude-standard"])?;
        Ok(listing.lines().map(str::to_string).collect())
    }
}
