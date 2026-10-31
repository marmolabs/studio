//! Git operations the studio needs: per-ticket worktrees, owner-identity commits
//! and goal branches. Everything shells out to the `git` CLI.

mod attribution;
mod cmd;
mod error;
mod repo;
mod worktree;

pub use attribution::strip_attribution;
pub use error::{GitError, Result};
pub use repo::{EVIDENCE_DIR, LandOutcome, MergeOutcome, Repo};
pub use worktree::{RebaseOutcome, Worktree};

/// Lowercase ASCII letters and digits, everything else collapsed to `-`.
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "ticket".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs() {
        assert_eq!(slug("HUE-12"), "hue-12");
        assert_eq!(slug("Add a daily mode!"), "add-a-daily-mode");
        assert_eq!(slug("  "), "ticket");
    }
}
