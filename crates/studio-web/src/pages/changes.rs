//! A ticket's work so far: the diff file by file, and the size of each.

use topcoat::view::{View, component, view};

use crate::ui;

/// How the lines of a diff are colored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Add,
    Remove,
    Hunk,
    Plain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    path: String,
    adds: usize,
    removes: usize,
    lines: Vec<(Tone, String)>,
}

const LINES_SHOWN: usize = 2000;

/// Splits a unified diff into its files.
pub fn parse(diff: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // "a/x b/x": the new path is what the owner cares about.
            let path = rest.rsplit_once(" b/").map_or(rest, |(_, new)| new);
            files.push(FileDiff {
                path: path.to_string(),
                adds: 0,
                removes: 0,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        // Past the first hunk, "+++" and "---" are changes like any other.
        let in_hunk = file.lines.iter().any(|(tone, _)| *tone == Tone::Hunk);
        let tone = if line.starts_with("@@") {
            Tone::Hunk
        } else if !in_hunk {
            if line.starts_with("+++") || line.starts_with("---") || line.starts_with("index ") {
                continue;
            }
            Tone::Plain
        } else if line.starts_with('+') {
            file.adds += 1;
            Tone::Add
        } else if line.starts_with('-') {
            file.removes += 1;
            Tone::Remove
        } else {
            Tone::Plain
        };
        if file.lines.len() < LINES_SHOWN {
            file.lines.push((tone, line.to_string()));
        }
    }
    files
}

/// Each changed file with its lines added and removed.
pub fn stats(diff: &str) -> Vec<(String, u64, u64)> {
    parse(diff)
        .into_iter()
        .map(|f| (f.path, f.adds as u64, f.removes as u64))
        .collect()
}

/// The diff, one folded row per file.
#[component]
pub async fn changes(diff: Option<String>) -> topcoat::Result<impl View> {
    let files = diff.as_deref().map(parse).unwrap_or_default();
    Ok(view! {
        <section id="changes">
            ui::section_head(title: "Changes", count: Some(files.len() as u64), href: String::new(), label: String::new())
            if files.is_empty() {
                <p class=(ui::MUTED)>"No changes yet."</p>
            } else {
                <div class=(ui::LIST)>
                    for file in files {
                        <details class="group border-t border-line first:border-t-0">
                            <summary class="flex cursor-pointer list-none items-center gap-2.5 px-4 py-2.5 text-[12.5px] hover:bg-hover [&::-webkit-details-marker]:hidden">
                                <span class="text-fg-3 transition-transform group-open:rotate-90">ui::icon(name: "chevron-right", small: true)</span>
                                <span class="min-w-0 flex-1 truncate font-mono text-[12px] text-fg-2">(file.path)</span>
                                ui::diff_size(added: file.adds as u64, removed: file.removes as u64)
                            </summary>
                            <div class="overflow-x-auto border-t border-line bg-bg-2 py-1 font-mono text-[12px] leading-5">
                                <div class="w-max min-w-full">
                                    for (tone, text) in file.lines {
                                        <div class=(match tone {
                                            Tone::Add => "whitespace-pre bg-ok-bg px-4 text-ok-fg",
                                            Tone::Remove => "whitespace-pre bg-bad-bg px-4 text-bad-fg",
                                            Tone::Hunk => "whitespace-pre px-4 text-fg-3",
                                            Tone::Plain => "whitespace-pre px-4 text-fg-2",
                                        })>(text)</div>
                                    }
                                </div>
                            </div>
                        </details>
                    }
                </div>
            }
        </section>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n keep\n-old\n+new <b>\ndiff --git a/b.txt b/b.txt\nnew file mode 100644\n--- /dev/null\n+++ b/b.txt\n@@ -0,0 +1 @@\n+hi\n";

    #[test]
    fn splits_a_diff_per_file_and_counts_lines() {
        let files = parse(DIFF);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!((files[0].adds, files[0].removes), (1, 1));
        assert_eq!((files[1].path.as_str(), files[1].adds), ("b.txt", 1));
        assert_eq!(files[0].lines[0].0, Tone::Hunk);
        assert_eq!(
            stats(DIFF),
            [("src/a.rs".into(), 1, 1), ("b.txt".into(), 1, 0)]
        );
    }

    #[tokio::test]
    async fn the_diff_is_escaped() {
        let cx = topcoat::context::Cx::default();
        let html = crate::ui::html(&cx, view! { cx => changes(diff: Some(DIFF.to_string())) })
            .await
            .unwrap();
        assert!(html.contains("new &lt;b&gt;"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }
}
