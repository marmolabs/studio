//! How a goal is planned: what the planner is told up front, what it may use
//! and what it may spend. A plan that has to explore the repository to find
//! its feet re-reads a growing conversation on every turn, so the planner gets
//! a brief of the repository instead and is told to read only what the plan
//! needs.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use studio_core::ticket::Ticket;

/// The most a plan run may cost, in dollars. Counted in the project's day.
pub(crate) const PLAN_USD: f64 = 1.25;

/// The most turns a plan run may take.
pub(crate) const PLAN_TURNS: u32 = 25;

/// What a planner may not use: it reads, and files tasks with the studio's tool.
pub(crate) const DISALLOWED: [&str; 6] = [
    "Edit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
];

/// The most lines of a README, and characters of them, the brief carries.
const README_LINES: usize = 60;
const README_CHARS: usize = 3_000;

/// The most lines of the file tree. A larger tree is folded by folder.
const TREE_LINES: usize = 200;

/// The most recent commits and open tasks the brief names.
const COMMITS: usize = 10;
const OPEN_TASKS: usize = 20;

/// What the planner is shown of the repository.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Brief {
    /// The start of the README, or empty.
    pub readme: String,
    /// Every tracked file with its size in bytes.
    pub files: Vec<(String, u64)>,
    /// The latest commits, one line each, newest first.
    pub commits: Vec<String>,
    /// The project's open tasks, one line each.
    pub open: Vec<String>,
}

/// Reads the brief of the checkout at `root`. A part git or the disk can't
/// give is left out: the planner can still read the code itself.
pub(crate) fn gather(root: &Path, open: Vec<String>) -> Brief {
    let git = |args: &[&str]| -> String {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default()
    };
    let files = git(&["ls-files"])
        .lines()
        .map(|path| {
            let size = std::fs::metadata(root.join(path)).map_or(0, |m| m.len());
            (path.to_string(), size)
        })
        .collect();
    let readme = std::fs::read_dir(root)
        .ok()
        .and_then(|dir| {
            let mut names: Vec<String> = dir
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.to_lowercase().starts_with("readme"))
                .collect();
            names.sort();
            names.into_iter().next()
        })
        .and_then(|name| std::fs::read_to_string(root.join(name)).ok())
        .unwrap_or_default();
    let log = format!("-{COMMITS}");
    Brief {
        readme,
        files,
        commits: git(&["log", "--oneline", &log])
            .lines()
            .map(String::from)
            .collect(),
        open,
    }
}

/// The brief as text for the planner to read.
pub(crate) fn render(brief: &Brief) -> String {
    let mut out = String::new();
    let readme = head(&brief.readme);
    if !readme.is_empty() {
        out.push_str(&format!("## README (the start)\n{readme}\n\n"));
    }
    if !brief.files.is_empty() {
        out.push_str(&format!(
            "## Files ({} tracked, with sizes)\n{}\n\n",
            brief.files.len(),
            tree(&brief.files)
        ));
    }
    if !brief.commits.is_empty() {
        out.push_str("## Recent commits\n");
        for commit in brief.commits.iter().take(COMMITS) {
            out.push_str(&format!("{commit}\n"));
        }
        out.push('\n');
    }
    if !brief.open.is_empty() {
        out.push_str("## Open tasks (don't plan these again)\n");
        for task in brief.open.iter().take(OPEN_TASKS) {
            out.push_str(&format!("{task}\n"));
        }
        if brief.open.len() > OPEN_TASKS {
            out.push_str(&format!("and {} more\n", brief.open.len() - OPEN_TASKS));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// The first lines of a README, cut to a length.
fn head(readme: &str) -> String {
    let lines: Vec<&str> = readme.lines().take(README_LINES).collect();
    let text = lines.join("\n");
    match text.char_indices().nth(README_CHARS) {
        Some((at, _)) => format!("{}...", text[..at].trim_end()),
        None => text,
    }
}

/// A size in words: "812 B", "14 K", "2 M".
fn size_words(bytes: u64) -> String {
    match bytes {
        0..1_024 => format!("{bytes} B"),
        1_024..1_048_576 => format!("{} K", bytes.div_ceil(1_024)),
        _ => format!("{} M", bytes.div_ceil(1_048_576)),
    }
}

/// The files one to a line with their sizes; a tree too long for that is
/// folded into folders, as deep as still fits, each with its file count and
/// total size.
fn tree(files: &[(String, u64)]) -> String {
    let single = |files: &[(String, u64)]| -> Vec<String> {
        files
            .iter()
            .map(|(path, size)| format!("{path} ({})", size_words(*size)))
            .collect()
    };
    if files.len() <= TREE_LINES {
        return single(files).join("\n");
    }
    let deepest = files
        .iter()
        .map(|(p, _)| p.matches('/').count())
        .max()
        .unwrap_or(0);
    let mut folded = Vec::new();
    for depth in (1..=deepest.max(1)).rev() {
        folded = fold(files, depth);
        if folded.len() <= TREE_LINES {
            break;
        }
    }
    folded.join("\n")
}

/// The files grouped by their folder, `depth` folders deep.
fn fold(files: &[(String, u64)], depth: usize) -> Vec<String> {
    let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (path, size) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let key = if parts.len() > depth {
            format!("{}/", parts[..depth].join("/"))
        } else {
            path.clone()
        };
        let entry = groups.entry(key).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += size;
    }
    groups
        .into_iter()
        .map(|(key, (count, size))| match count {
            1 if !key.ends_with('/') => format!("{key} ({})", size_words(size)),
            n => format!("{key} ({n} files, {})", size_words(size)),
        })
        .collect()
}

/// What the planner is asked, with the repository's brief.
pub(crate) fn prompt(plan: &Ticket, brief: &str) -> String {
    let mut out = format!(
        "You are planning the goal \"{}\" for project {}. The brief below is all you \
         need to start: use it instead of exploring the repository. Read only the files \
         the plan needs, and only the parts of large ones (the sizes are listed; search \
         for a name, then read a range). A handful of reads should be enough. Never build, \
         run tests or change files.\n\n\
         File one ticket per independent, reviewable change with file_ticket, each small \
         enough for one agent run, with acceptance criteria in its detail. File each as \
         soon as it is clear, so a plan cut short keeps what it filed. You have {} turns \
         and ${:.2}. Use ask_owner only if the goal is ambiguous. End with a short \
         summary of the plan.",
        plan.goal, plan.project, PLAN_TURNS, PLAN_USD
    );
    if let Some(version) = &plan.version {
        out.push_str(&format!(
            " The goal is a milestone towards version {version}."
        ));
    }
    if !plan.detail.trim().is_empty() {
        out.push_str(&format!("\n\nThe owner's brief:\n\n{}", plan.detail.trim()));
    }
    if !brief.is_empty() {
        out.push_str(&format!("\n\n# The repository\n\n{brief}"));
    }
    out
}

/// What the studio says when a plan is cut off after it filed `filed` tasks.
pub(crate) fn cut_note(filed: usize, why: &str) -> String {
    format!("The plan was stopped ({why}) after it filed {filed} task(s); the plan may be partial.")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use studio_core::ids::{HireId, ProjectId, TicketId};
    use studio_core::ticket::{TicketKind, TicketState};

    fn plan() -> Ticket {
        Ticket {
            id: TicketId::new("SHOP-1"),
            project: ProjectId::new("shop"),
            title: "Plan: Add a cart".into(),
            goal: "Add a cart".into(),
            hire: HireId::new("lead"),
            state: TicketState::Queued,
            attempts: 0,
            base_branch: "master".into(),
            created_at: 0,
            held_from: None,
            feedback: None,
            kind: TicketKind::Plan,
            parent: None,
            detail: String::new(),
            source: None,
            version: None,
            priority: studio_core::ticket::DEFAULT_PRIORITY,
            depends_on: Vec::new(),
        }
    }

    fn files(n: usize) -> Vec<(String, u64)> {
        (0..n)
            .map(|i| (format!("crates/c{}/src/f{i}.rs", i % 10), 2_000))
            .collect()
    }

    #[test]
    fn sizes_read_in_bytes_kilobytes_and_megabytes() {
        assert_eq!(size_words(812), "812 B");
        assert_eq!(size_words(1_025), "2 K");
        assert_eq!(size_words(3 * 1_048_576), "3 M");
    }

    #[test]
    fn a_small_tree_lists_every_file_with_its_size() {
        let text = tree(&[("a.rs".into(), 10), ("src/b.rs".into(), 5_000)]);
        assert_eq!(text, "a.rs (10 B)\nsrc/b.rs (5 K)");
    }

    #[test]
    fn a_large_tree_is_folded_into_folders_that_fit() {
        let text = tree(&files(500));
        assert!(text.lines().count() <= TREE_LINES, "{text}");
        assert!(text.contains("crates/c3/src/ (50 files, 98 K)"), "{text}");
    }

    #[test]
    fn the_readme_is_cut_to_its_first_lines_and_characters() {
        let long = "line\n".repeat(500);
        assert_eq!(head(&long).lines().count(), README_LINES);
        let wide = "x".repeat(10_000);
        assert!(head(&wide).chars().count() <= README_CHARS + 3);
        assert_eq!(head("short"), "short");
    }

    #[test]
    fn the_brief_names_each_part_and_leaves_out_what_is_missing() {
        let brief = Brief {
            readme: "# Shop\nA shop.".into(),
            files: vec![("src/lib.rs".into(), 100)],
            commits: vec!["abc123 Add shelves".into()],
            open: vec!["SHOP-2: Fix tills [working]".into()],
        };
        let text = render(&brief);
        for part in [
            "## README (the start)\n# Shop",
            "## Files (1 tracked, with sizes)\nsrc/lib.rs (100 B)",
            "## Recent commits\nabc123 Add shelves",
            "## Open tasks (don't plan these again)\nSHOP-2: Fix tills [working]",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert_eq!(render(&Brief::default()), "");
    }

    #[test]
    fn many_open_tasks_are_counted_not_all_listed() {
        let brief = Brief {
            open: (0..25).map(|i| format!("T-{i}")).collect(),
            ..Brief::default()
        };
        let text = render(&brief);
        assert!(text.contains("T-19") && !text.contains("T-20"), "{text}");
        assert!(text.contains("and 5 more"), "{text}");
    }

    #[test]
    fn the_prompt_gives_the_goal_the_limits_and_the_brief() {
        let mut ticket = plan();
        ticket.version = Some("0.5".into());
        ticket.detail = "Keep it small.".into();
        let text = prompt(&ticket, "## Files\nsrc/lib.rs (1 B)");
        assert!(text.contains("planning the goal \"Add a cart\" for project shop"));
        assert!(text.contains("Never build, run tests or change files"));
        assert!(text.contains("You have 25 turns and $1.25"));
        assert!(text.contains("milestone towards version 0.5"));
        assert!(text.contains("The owner's brief:\n\nKeep it small."));
        assert!(text.ends_with("# The repository\n\n## Files\nsrc/lib.rs (1 B)"));
        assert!(!prompt(&plan(), "").contains("# The repository"));
    }

    #[test]
    fn a_planner_may_not_change_files_or_spawn_helpers() {
        for tool in ["Edit", "Write", "Task", "WebFetch"] {
            assert!(DISALLOWED.contains(&tool));
        }
        assert!(!DISALLOWED.contains(&"Read"));
        assert!(PLAN_USD > 0.0 && PLAN_TURNS > 0);
    }

    #[test]
    fn gathering_reads_a_real_repository() {
        let dir = tempfile_dir();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "{args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# Hi\n").unwrap();
        std::fs::write(dir.join("a.txt"), "abc").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "First"]);
        let brief = gather(&dir, vec!["X-1: t".into()]);
        assert_eq!(brief.readme, "# Hi\n");
        assert!(brief.files.contains(&("a.txt".to_string(), 3)));
        assert!(brief.commits[0].ends_with("First"));
        assert_eq!(brief.open, vec!["X-1: t".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gathering_a_folder_without_git_gives_a_thin_brief() {
        let dir = tempfile_dir();
        let brief = gather(&dir, Vec::new());
        assert!(brief.files.is_empty() && brief.commits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "studio-planning-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
