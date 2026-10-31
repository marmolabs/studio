use crate::cmd::{conflicted_files, failed, output, run};
use crate::error::{GitError, Result};
use crate::slug;
use crate::worktree::Worktree;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use studio_core::ids::TicketId;

/// Git takes a lock on `.git/config` and the worktree list, so two workers
/// adding worktrees at once can fail each other. This keeps them in turn.
static WORKTREE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn worktree_lock() -> std::sync::MutexGuard<'static, ()> {
    WORKTREE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const EXCLUDE_LINE: &str = ".studio/";
/// Longest goal part of a goal branch name.
const GOAL_NAME_MAX: usize = 60;
/// Where a ticket's capture output goes; never committed.
pub const EVIDENCE_DIR: &str = ".studio-evidence";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LandOutcome {
    Merged { sha: String },
    Conflict { files: Vec<String> },
}

/// What merging a goal branch into its target did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged {
        sha: String,
    },
    Conflict {
        files: Vec<String>,
    },
    /// The target is checked out with uncommitted changes; nothing was done.
    Dirty {
        path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    pub fn open(path: impl AsRef<Path>) -> Result<Repo> {
        let path = path.as_ref();
        let out = output(path, &["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            return Err(GitError::NotARepo(path.to_path_buf()));
        }
        let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Repo {
            root: PathBuf::from(root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The checked-out branch, or None on a detached HEAD.
    pub fn current_branch(&self) -> Result<Option<String>> {
        let out = output(&self.root, &["symbolic-ref", "--short", "-q", "HEAD"])?;
        let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((out.status.success() && !branch.is_empty()).then_some(branch))
    }

    /// Full commit sha of `rev`.
    pub fn resolve(&self, rev: &str) -> Result<String> {
        resolve_in(&self.root, rev)
    }

    pub fn create_worktree(&self, ticket: &TicketId, base_rev: &str) -> Result<Worktree> {
        let _turn = worktree_lock();
        let name = slug(&ticket.to_string());
        let branch = Repo::ticket_branch_name(ticket);
        let path = self.worktrees_dir().join(&name);
        if self.branch_exists(&branch)? {
            return Err(GitError::AlreadyExists(branch));
        }
        if path.exists() {
            return Err(GitError::AlreadyExists(path.display().to_string()));
        }
        let base_commit = self.resolve(base_rev)?;
        self.ensure_excluded()?;
        let path_arg = path.to_string_lossy();
        run(
            &self.root,
            &["worktree", "add", "-b", &branch, &path_arg, &base_commit],
        )?;
        run(&self.root, &["config", &base_key(&branch), &base_commit])?;
        Ok(Worktree {
            path,
            branch,
            base_commit,
        })
    }

    pub fn remove_worktree(&self, worktree: &Worktree, force: bool) -> Result<()> {
        let path_arg = worktree.path.to_string_lossy();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&path_arg);
        run(&self.root, &args)?;
        Ok(())
    }

    pub fn prune(&self) -> Result<()> {
        run(&self.root, &["worktree", "prune"])?;
        Ok(())
    }

    /// Ticket worktrees the studio created in this repo (goal worktrees excluded).
    pub fn studio_worktrees(&self) -> Result<Vec<Worktree>> {
        let listing = run(&self.root, &["worktree", "list", "--porcelain"])?;
        let dir = self.worktrees_dir();
        let mut found = Vec::new();
        for block in listing.split("\n\n") {
            let mut path = None;
            let mut branch = None;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                }
            }
            let (Some(path), Some(branch)) = (path, branch) else {
                continue;
            };
            if !path.starts_with(&dir) || branch.starts_with("studio/goal/") {
                continue;
            }
            let base = run(&self.root, &["config", "--get", &base_key(&branch)])
                .map_err(|_| GitError::Parse(format!("no recorded base for {branch}")))?;
            found.push(Worktree {
                path,
                branch,
                base_commit: base.trim().to_string(),
            });
        }
        Ok(found)
    }

    /// The branch a ticket's worktree works on, whether or not it exists.
    pub fn ticket_branch_name(ticket: &TicketId) -> String {
        format!("studio/{}", slug(&ticket.to_string()))
    }

    /// The name of a goal's branch, whether or not it exists yet. Long goals
    /// are cut to their first words, so the name stays a valid ref.
    pub fn goal_branch_name(goal: &str) -> String {
        let slug = slug(goal);
        let cut: String = slug.chars().take(GOAL_NAME_MAX).collect();
        format!("studio/goal/{}", cut.trim_end_matches('-'))
    }

    /// Deletes a local branch that exists. With `merged_into`, only if the
    /// branch is already part of that revision; returns whether it deleted.
    pub fn delete_branch(&self, branch: &str, merged_into: Option<&str>) -> Result<bool> {
        if let Some(rev) = merged_into {
            let out = output(&self.root, &["merge-base", "--is-ancestor", branch, rev])?;
            if !out.status.success() {
                return Ok(false);
            }
        }
        run(&self.root, &["branch", "-D", branch])?;
        Ok(true)
    }

    /// Whether `branch` is already part of `rev`. A missing branch or a git
    /// error counts as not merged.
    pub fn is_merged(&self, branch: &str, rev: &str) -> bool {
        output(&self.root, &["merge-base", "--is-ancestor", branch, rev])
            .is_ok_and(|out| out.status.success())
    }

    /// The commit each local branch and `HEAD` point at, from one git call.
    pub fn branch_tips(&self) -> Result<HashMap<String, String>> {
        let refs = run(&self.root, &["show-ref", "--head"])?;
        let tips = refs.lines().filter_map(|line| {
            let (sha, name) = line.split_once(' ')?;
            let name = if name == "HEAD" {
                name
            } else {
                name.strip_prefix("refs/heads/")?
            };
            Some((name.to_string(), sha.to_string()))
        });
        Ok(tips.collect())
    }

    /// Creates `studio/goal/<slug>` from `base_rev` unless it already exists.
    pub fn ensure_goal_branch(&self, goal: &str, base_rev: &str) -> Result<String> {
        let branch = Self::goal_branch_name(goal);
        if !self.branch_exists(&branch)? {
            let base = self.resolve(base_rev)?;
            run(&self.root, &["branch", &branch, &base])?;
        }
        Ok(branch)
    }

    /// Merges a ticket branch into a goal branch inside a dedicated goal worktree,
    /// so the owner's own checkout is never touched.
    pub fn land(&self, worktree: &Worktree, goal_branch: &str) -> Result<LandOutcome> {
        let _turn = worktree_lock();
        let goal_name = goal_branch.trim_start_matches("studio/goal/");
        let goal_path = self
            .worktrees_dir()
            .join(format!("goal-{}", slug(goal_name)));
        if !goal_path.exists() {
            self.ensure_excluded()?;
            let path_arg = goal_path.to_string_lossy();
            run(&self.root, &["worktree", "add", &path_arg, goal_branch])?;
        }
        let args = ["merge", "--no-ff", "--no-edit", worktree.branch.as_str()];
        let out = output(&goal_path, &args)?;
        if out.status.success() {
            return Ok(LandOutcome::Merged {
                sha: resolve_in(&goal_path, "HEAD")?,
            });
        }
        let files = conflicted_files(&goal_path)?;
        run(&goal_path, &["merge", "--abort"])?;
        if files.is_empty() {
            return Err(failed(&args, &out));
        }
        Ok(LandOutcome::Conflict { files })
    }

    /// Merges `goal_branch` into `target`. Where `target` is checked out, the
    /// merge happens there, and only if that checkout has no uncommitted
    /// changes; otherwise in a short-lived worktree. A conflict is backed out.
    pub fn merge_goal(&self, goal_branch: &str, target: &str) -> Result<MergeOutcome> {
        let (dir, temporary) = match self.checkout_of(target)? {
            Some(path) => {
                let status = run(&path, &["status", "--porcelain", "--untracked-files=no"])?;
                if !status.trim().is_empty() {
                    return Ok(MergeOutcome::Dirty { path });
                }
                (path, false)
            }
            None => {
                self.ensure_excluded()?;
                let path = self.worktrees_dir().join(format!("merge-{}", slug(target)));
                let path_arg = path.to_string_lossy();
                run(&self.root, &["worktree", "add", &path_arg, target])?;
                (path, true)
            }
        };
        let args = ["merge", "--no-ff", "--no-edit", goal_branch];
        let out = output(&dir, &args)?;
        let outcome = if out.status.success() {
            Ok(MergeOutcome::Merged {
                sha: resolve_in(&dir, "HEAD")?,
            })
        } else {
            let files = conflicted_files(&dir)?;
            let _ = run(&dir, &["merge", "--abort"]);
            if files.is_empty() {
                Err(failed(&args, &out))
            } else {
                Ok(MergeOutcome::Conflict { files })
            }
        };
        if temporary {
            let path_arg = dir.to_string_lossy();
            let _ = run(&self.root, &["worktree", "remove", "--force", &path_arg]);
        }
        outcome
    }

    /// The worktree (the main checkout included) that has `branch` checked out.
    fn checkout_of(&self, branch: &str) -> Result<Option<PathBuf>> {
        let list = run(&self.root, &["worktree", "list", "--porcelain"])?;
        let wanted = format!("branch refs/heads/{branch}");
        let mut path: Option<PathBuf> = None;
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if line == wanted {
                return Ok(path);
            }
        }
        Ok(None)
    }

    fn worktrees_dir(&self) -> PathBuf {
        self.root.join(".studio").join("worktrees")
    }

    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        let reference = format!("refs/heads/{branch}");
        let out = output(&self.root, &["show-ref", "--verify", "--quiet", &reference])?;
        Ok(out.status.success())
    }

    /// Adds the studio's own folders to `.git/info/exclude`; the repo's
    /// `.gitignore` is left alone.
    fn ensure_excluded(&self) -> Result<()> {
        self.exclude(EXCLUDE_LINE)?;
        self.exclude(&format!("{EVIDENCE_DIR}/"))
    }

    /// Adds `line` to the repo's shared `info/exclude` once, so it covers every
    /// worktree.
    pub fn exclude(&self, line: &str) -> Result<()> {
        let rel = run(&self.root, &["rev-parse", "--git-path", "info/exclude"])?;
        let path = self.root.join(rel.trim());
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.lines().any(|l| l.trim() == line) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            writeln!(file)?;
        }
        writeln!(file, "{line}")?;
        Ok(())
    }
}

pub(crate) fn resolve_in(dir: &Path, rev: &str) -> Result<String> {
    let spec = format!("{rev}^{{commit}}");
    Ok(run(dir, &["rev-parse", "--verify", &spec])?
        .trim()
        .to_string())
}

pub(crate) fn base_key(branch: &str) -> String {
    format!("branch.{branch}.studioBase")
}
