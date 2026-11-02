//! What a release needs from git: a throwaway checkout of one commit to run
//! the release in, and the annotated tag that names the version.

use crate::cmd::{output, run};
use crate::error::Result;
use crate::repo::{EXCLUDE_LINE, Repo, worktree_lock};
use std::path::{Path, PathBuf};

impl Repo {
    /// Whether `tag` names a tag in this repository.
    pub fn tag_exists(&self, tag: &str) -> Result<bool> {
        let reference = format!("refs/tags/{tag}");
        let out = output(
            self.root(),
            &["show-ref", "--verify", "--quiet", &reference],
        )?;
        Ok(out.status.success())
    }

    /// Whether git accepts `tag` as a tag name.
    pub fn is_tag_name(&self, tag: &str) -> Result<bool> {
        let reference = format!("refs/tags/{tag}");
        let out = output(self.root(), &["check-ref-format", &reference])?;
        Ok(out.status.success())
    }

    /// Where the release checkout called `name` lives.
    pub fn release_worktree_path(&self, name: &str) -> PathBuf {
        self.root().join(".studio").join("releases").join(name)
    }

    /// A fresh checkout of `rev` on no branch, under the studio's own
    /// folder, named `name`. Returns its path.
    pub fn create_release_worktree(&self, name: &str, rev: &str) -> Result<PathBuf> {
        let _turn = worktree_lock();
        let commit = self.resolve(rev)?;
        self.exclude(EXCLUDE_LINE)?;
        let path = self.release_worktree_path(name);
        let path_arg = path.to_string_lossy();
        run(
            self.root(),
            &["worktree", "add", "--detach", &path_arg, &commit],
        )?;
        Ok(path)
    }

    /// Removes a checkout made by `create_release_worktree`, whatever it
    /// holds. A checkout that is already gone is not an error.
    pub fn remove_release_worktree(&self, path: &Path) -> Result<()> {
        let _turn = worktree_lock();
        let path_arg = path.to_string_lossy();
        let out = output(self.root(), &["worktree", "remove", "--force", &path_arg])?;
        if !out.status.success() && path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        run(self.root(), &["worktree", "prune"])?;
        Ok(())
    }

    /// Tags `rev` with an annotated tag, made with the repository's own
    /// identity. Nothing is pushed.
    pub fn tag(&self, tag: &str, rev: &str, message: &str) -> Result<()> {
        let commit = self.resolve(rev)?;
        run(
            self.root(),
            &["tag", "--annotate", "--message", message, tag, &commit],
        )?;
        Ok(())
    }
}
