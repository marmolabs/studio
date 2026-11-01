//! What a ticket's capture run left behind: images and clips under
//! `<worktree>/.studio-evidence/<attempt>/`, and a copy kept in the data
//! folder once the ticket lands and its worktree goes.

use crate::api::EvidenceFile;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) use studio_git::EVIDENCE_DIR as DIR;

const KINDS: [&str; 4] = ["png", "jpg", "gif", "mp4"];

/// Where attempt `attempt` writes, relative to the worktree.
pub(crate) fn folder(attempt: u32) -> String {
    format!("{DIR}/{attempt}")
}

/// A repo path as a file-name stem: anything but `[A-Za-z0-9_-]` becomes `-`,
/// cut to 80 characters.
pub(crate) fn image_key(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(80)
        .collect()
}

/// Whether any part of `rel` under `root` is a link. A part that doesn't exist
/// yet isn't one.
pub(crate) fn through_link(root: &Path, rel: &str) -> bool {
    let mut path = root.to_path_buf();
    Path::new(rel).components().any(|part| {
        path.push(part);
        fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// The images a gate writes for its critic, kept out of `list`.
fn is_pair(name: &str) -> bool {
    name.starts_with("before-") || name.starts_with("after-")
}

/// A plain file name with an evidence extension: no separators, no dot-files.
fn valid_name(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, ext)| ext);
    !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && ext.is_some_and(|ext| KINDS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Every capture in the worktree, oldest attempt first. Paths are absolute.
pub(crate) fn list(worktree: &Path) -> Vec<EvidenceFile> {
    files_in(&worktree.join(DIR), false)
}

/// Every evidence file in the worktree, the gates' image pairs included.
pub(crate) fn all(worktree: &Path) -> Vec<EvidenceFile> {
    files_in(&worktree.join(DIR), true)
}

/// Where a landed ticket's evidence is kept.
pub(crate) fn kept(data_dir: &Path, ticket: &str) -> PathBuf {
    data_dir.join("evidence").join(ticket)
}

/// The evidence kept for a ticket whose worktree is gone.
pub(crate) fn list_kept(data_dir: &Path, ticket: &str) -> Vec<EvidenceFile> {
    files_in(&kept(data_dir, ticket), true)
}

/// Copies the worktree's evidence, image pairs included, into the data
/// folder before the worktree is removed. Returns how many files it kept.
pub(crate) fn keep(worktree: &Path, data_dir: &Path, ticket: &str) -> std::io::Result<usize> {
    let files = all(worktree);
    let to = kept(data_dir, ticket);
    for file in &files {
        let folder = to.join(file.attempt.to_string());
        fs::create_dir_all(&folder)?;
        fs::copy(&file.path, folder.join(&file.name))?;
    }
    Ok(files.len())
}

/// The evidence files in `<dir>/<attempt>/`, oldest attempt first, with or
/// without the gates' image pairs. Paths are absolute.
fn files_in(dir: &Path, pairs: bool) -> Vec<EvidenceFile> {
    let mut found = Vec::new();
    let Ok(attempts) = fs::read_dir(dir) else {
        return found;
    };
    for dir in attempts.filter_map(|e| e.ok()) {
        let Some(attempt) = dir.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.filter_map(|e| e.ok()) {
            let name = file.file_name().to_string_lossy().into_owned();
            // `file_type` doesn't follow links, so a link out of the folder is skipped.
            if valid_name(&name)
                && (pairs || !is_pair(&name))
                && file.file_type().is_ok_and(|t| t.is_file())
            {
                found.push(EvidenceFile {
                    attempt,
                    path: file.path().to_string_lossy().into_owned(),
                    name,
                });
            }
        }
    }
    found.sort_by(|a, b| (a.attempt, &a.name).cmp(&(b.attempt, &b.name)));
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(valid_name("title.png"));
        assert!(valid_name("game-over_2.JPG"));
        assert!(!valid_name("notes.txt"));
        assert!(!valid_name(".hidden.png"));
        assert!(!valid_name("a b.png"));
        assert!(!valid_name("a/b.png"));
        assert!(!valid_name("png"));
    }

    #[test]
    fn keys_are_plain_and_short() {
        assert_eq!(
            image_key("assets/my sprite.v2/ü.png"),
            "assets-my-sprite-v2---png"
        );
        let long = format!("{}.png", "a/".repeat(100));
        let key = image_key(&long);
        assert_eq!(key.chars().count(), 80);
        assert!(valid_name(&format!("after-{key}.PNG")));
        assert!(valid_name(&format!("before-{}.gif", image_key("x y.gif"))));
    }

    #[test]
    fn lists_only_evidence_files() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join(folder(1));
        let zero = tmp.path().join(folder(0));
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&zero).unwrap();
        fs::create_dir_all(tmp.path().join(DIR).join("junk")).unwrap();
        for (dir, name) in [(&one, "b.png"), (&zero, "z.gif"), (&zero, "a.mp4")] {
            fs::write(dir.join(name), "x").unwrap();
        }
        fs::write(zero.join("log.txt"), "x").unwrap();
        fs::write(one.join("before-a-png.png"), "x").unwrap();
        fs::write(one.join("after-a-png.png"), "x").unwrap();
        fs::write(tmp.path().join(DIR).join("top.png"), "x").unwrap();
        let names: Vec<_> = list(tmp.path())
            .into_iter()
            .map(|f| (f.attempt, f.name))
            .collect();
        assert_eq!(
            names,
            vec![
                (0, "a.mp4".to_string()),
                (0, "z.gif".to_string()),
                (1, "b.png".to_string())
            ]
        );
        assert!(list(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn landed_evidence_is_kept_with_its_pairs_but_not_its_links() {
        let worktree = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let one = worktree.path().join(folder(1));
        fs::create_dir_all(&one).unwrap();
        for name in ["shot.png", "before-a-png.png", "after-a-png.png"] {
            fs::write(one.join(name), name).unwrap();
        }
        let secret = worktree.path().join("secret.png");
        fs::write(&secret, "secret").unwrap();
        std::os::unix::fs::symlink(&secret, one.join("link.png")).unwrap();
        let names = |files: Vec<EvidenceFile>| -> Vec<String> {
            files.into_iter().map(|f| f.name).collect()
        };
        assert_eq!(names(list(worktree.path())), ["shot.png"]);
        assert_eq!(
            names(all(worktree.path())),
            ["after-a-png.png", "before-a-png.png", "shot.png"]
        );

        assert_eq!(keep(worktree.path(), data.path(), "T-1").unwrap(), 3);
        let kept = list_kept(data.path(), "T-1");
        assert_eq!(
            names(kept.clone()),
            ["after-a-png.png", "before-a-png.png", "shot.png"]
        );
        assert!(kept.iter().all(|f| {
            f.attempt == 1
                && f.path
                    .starts_with(&data.path().to_string_lossy().into_owned())
        }));
        assert_eq!(fs::read_to_string(&kept[2].path).unwrap(), "shot.png");
        assert!(list_kept(data.path(), "T-2").is_empty());
    }
}
