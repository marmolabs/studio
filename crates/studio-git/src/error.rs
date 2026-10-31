use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("could not run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("unexpected git output: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, GitError>;
