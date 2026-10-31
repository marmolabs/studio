//! A ticket's work so far: the diffstat, and the diff file by file.

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

#[component]
pub async fn changes(stat: Option<String>, diff: Option<String>) -> topcoat::Result<impl View> {
    let files = diff.as_deref().map(parse).unwrap_or_default();
    Ok(view! {
        <section id="changes" class="mt-6">
            <h2 class=(format!("mb-2 text-[12px] font-medium {}", ui::MUTED))>"Changes"</h2>
            if let Some(stat) = stat.filter(|s| !s.trim().is_empty()) {
                <pre class="mb-2 max-w-full overflow-x-auto rounded-lg border border-zinc-200 bg-white p-3 font-mono text-[12px] dark:border-zinc-800 dark:bg-zinc-900">(stat)</pre>
            }
            if files.is_empty() {
                <p class=(ui::MUTED)>"No changes yet."</p>
            }
            <div class="space-y-2">
                for file in files {
                    <details class="rounded-lg border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
                        <summary class="flex cursor-pointer select-none items-center gap-2 px-3 py-1.5">
                            <span class="min-w-0 flex-1 break-all font-mono text-[12px]">(file.path)</span>
                            <span class="shrink-0 font-mono text-[12px] text-emerald-700 dark:text-emerald-400">"+" (file.adds)</span>
                            <span class="shrink-0 font-mono text-[12px] text-rose-700 dark:text-rose-400">"-" (file.removes)</span>
                        </summary>
                        <div class="overflow-x-auto border-t border-zinc-200 py-1 font-mono text-[12px] dark:border-zinc-800">
                            <div class="w-max min-w-full">
                                for (tone, text) in file.lines {
                                    <div class=(match tone {
                                        Tone::Add => "whitespace-pre bg-emerald-50 px-3 text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-300",
                                        Tone::Remove => "whitespace-pre bg-rose-50 px-3 text-rose-800 dark:bg-rose-950/50 dark:text-rose-300",
                                        Tone::Hunk => "whitespace-pre px-3 text-zinc-600 dark:text-zinc-400",
                                        Tone::Plain => "whitespace-pre px-3",
                                    })>(text)</div>
                                }
                            </div>
                        </div>
                    </details>
                }
            </div>
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
    }

    #[tokio::test]
    async fn the_diff_is_escaped() {
        let cx = topcoat::context::Cx::default();
        let html = crate::ui::html(
            &cx,
            view! { cx => changes(stat: None, diff: Some(DIFF.to_string())) },
        )
        .await
        .unwrap();
        assert!(html.contains("new &lt;b&gt;"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }
}
