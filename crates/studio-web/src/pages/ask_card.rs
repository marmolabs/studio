//! The card for one ask, on the asks page, the board and the ticket page:
//! what is asked, why it was asked, and the buttons to answer it.

use serde_json::Value;
use studio_config::draft::slug;
use studio_core::ticket::Ticket;
use studiod::api::AskView;
use topcoat::view::{View, component, view};

use crate::summary::{file_path, project_path, truncate};
use crate::ui;

/// Form fields that aren't answers to a question.
pub const DECISION: &str = "decision";
pub const MESSAGE: &str = "message";
/// The `decision` value that approves and also saves an allow rule.
pub const ALWAYS: &str = "always";

const COMMAND_SHOWN: usize = 1200;
const PREVIEW_CHARS: usize = 400;
const PREVIEW_LINES: usize = 8;

/// What an ask is about, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// "Run a command", "Edit a file"...
    pub verb: String,
    pub command: Option<String>,
    /// The command's own description of itself, when the agent gave one.
    pub description: Option<String>,
    /// A file path, relative to the worktree when it is inside it.
    pub path: Option<String>,
    /// The path is not in the project.
    pub outside: bool,
    /// The old and new text of an edit, cut short.
    pub change: Option<(String, String)>,
    /// More edits than the one shown, for `MultiEdit`.
    pub more_edits: usize,
    /// The input as JSON, when nothing better is known.
    pub json: Option<String>,
}

/// The first lines of `text`, cut to a preview's size.
fn preview(text: &str) -> String {
    let lines: Vec<&str> = text.lines().take(PREVIEW_LINES + 1).collect();
    let mut shown = lines[..lines.len().min(PREVIEW_LINES)].join("\n");
    let cut = lines.len() > PREVIEW_LINES;
    shown = truncate(&shown, PREVIEW_CHARS);
    if cut && !shown.ends_with('…') {
        shown.push('…');
    }
    shown
}

pub fn summarise(ask: &AskView) -> Summary {
    let input = &ask.input;
    let mut summary = Summary::default();
    let verb = |text: &str| text.to_string();
    match ask.tool.as_str() {
        _ if ask.kind == "question" => summary.verb = verb("Has a question"),
        "Bash" if input["command"].is_string() => {
            summary.verb = verb("Run a command");
            summary.command = input["command"]
                .as_str()
                .map(|c| truncate(c, COMMAND_SHOWN));
            summary.description = input["description"]
                .as_str()
                .filter(|d| !d.trim().is_empty())
                .map(str::to_string);
        }
        "Read" | "Edit" | "Write" | "MultiEdit" if file_path(input).is_some() => {
            summary.verb = verb(match ask.tool.as_str() {
                "Read" => "Read a file",
                "Write" => "Write a file",
                _ => "Edit a file",
            });
            let (path, outside) = project_path(file_path(input).unwrap_or_default());
            summary.path = Some(path);
            summary.outside = outside;
            let edit = if ask.tool == "MultiEdit" {
                summary.more_edits = input["edits"].as_array().map_or(0, |e| e.len());
                summary.more_edits = summary.more_edits.saturating_sub(1);
                input["edits"][0].clone()
            } else {
                input.clone()
            };
            if let (Some(old), Some(new)) =
                (edit["old_string"].as_str(), edit["new_string"].as_str())
            {
                summary.change = Some((preview(old), preview(new)));
            }
        }
        tool => {
            summary.verb = format!("Use {tool}");
            summary.json = Some(serde_json::to_string_pretty(input).unwrap_or_default());
        }
    }
    summary
}

/// Why the ask reached the owner, in the rules' own words if there are any.
pub fn reasons(ask: &AskView, summary: &Summary) -> Vec<String> {
    if !ask.reasons.is_empty() {
        return ask.reasons.clone();
    }
    if summary.outside {
        return vec!["It's outside the project's folder".to_string()];
    }
    vec!["No rule allows this yet".to_string()]
}

/// The command prefix "always allow" would save, for a Bash permission ask.
pub fn always_prefix(ask: &AskView) -> Option<String> {
    if ask.kind != "permission" || ask.tool != "Bash" {
        return None;
    }
    studio_core::policy::allow_prefix(ask.input["command"].as_str()?)
}

/// The name and text of a rule that allows `prefix` commands in `project`.
pub fn always_rule(project: &str, prefix: &str) -> (String, String) {
    let mut name = format!("allow-{}-{}", slug(project), slug(prefix));
    name.truncate(64);
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let text = format!(
        "---\nkind: gate\nlevel: allow\naction: {}\nwhen:\n  projects: [{}]\n---\nAllowed from the web UI.\n",
        quote(&format!("bash:{prefix}")),
        quote(project),
    );
    (name, text)
}

/// A question's text and its option labels, from the ask's input.
pub fn questions(input: &Value) -> Vec<(String, Vec<String>)> {
    input
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let text = q["question"].as_str()?.to_string();
            let options = q["options"]
                .as_array()?
                .iter()
                .filter_map(|o| o.as_str().map(str::to_string))
                .collect();
            Some((text, options))
        })
        .collect()
}

/// A ticket's title and project, for the card of an ask on it.
pub fn ticket_of(tickets: &[Ticket], id: &str) -> (String, String) {
    tickets
        .iter()
        .find(|t| t.id.to_string() == id)
        .map(|t| (t.title.clone(), t.project.to_string()))
        .unwrap_or_default()
}

/// The command, file or input an ask is about.
#[component]
async fn ask_target(summary: Summary) -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            if let Some(command) = summary.command {
                <pre class="mt-1.5 max-h-48 max-w-full overflow-auto whitespace-pre rounded-md bg-zinc-100 px-2 py-1.5 font-mono text-[12px] dark:bg-zinc-950">(command)</pre>
            }
            if let Some(description) = summary.description {
                <p class=(format!("mt-1 text-[12px] {}", ui::MUTED))>(description)</p>
            }
            if let Some(path) = summary.path {
                ask_file(path: path, outside: summary.outside)
            }
            if let Some((old, new)) = summary.change {
                ask_change(old: old, new: new, more: summary.more_edits)
            }
            if let Some(json) = summary.json {
                <pre class="mt-1.5 max-h-48 overflow-auto rounded-md bg-zinc-100 px-2 py-1.5 font-mono text-[12px] dark:bg-zinc-950">(json)</pre>
            }
        </div>
    })
}

#[component]
async fn ask_file(path: String, outside: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <pre class="mt-1.5 max-w-full overflow-auto whitespace-pre rounded-md bg-zinc-100 px-2 py-1.5 font-mono text-[12px] dark:bg-zinc-950">(path)</pre>
            if outside {
                <span class="mt-1 inline-block rounded bg-amber-100 px-1.5 py-px text-[11px] font-medium text-amber-800 dark:bg-amber-900 dark:text-amber-200">"outside the project"</span>
            }
        </div>
    })
}

#[component]
async fn ask_change(old: String, new: String, more: usize) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="mt-1.5 space-y-1 font-mono text-[12px]">
            <pre class="max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-md bg-rose-50 px-2 py-1 text-rose-800 dark:bg-rose-950/50 dark:text-rose-300">"- " (old)</pre>
            <pre class="max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-md bg-emerald-50 px-2 py-1 text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-300">"+ " (new)</pre>
            if more > 0 {
                <p class=(format!("font-sans {}", ui::MUTED))>"and " (more) " more edits"</p>
            }
        </div>
    })
}

/// What is asked, and why it reached the owner.
#[component]
async fn ask_body(
    summary: Summary,
    tool: String,
    question: bool,
    why: Vec<String>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <p class="mt-2 flex flex-wrap items-center gap-1.5 font-medium">
                (summary.verb.clone())
                if !tool.is_empty() && !question {
                    <span class="rounded bg-zinc-100 px-1.5 py-px font-mono text-[11px] font-normal text-zinc-600 dark:bg-zinc-800 dark:text-zinc-300">(tool.clone())</span>
                }
            </p>
            ask_target(summary: summary)
            if !question {
                <ul class="mt-2 list-disc pl-5 text-[12px] text-zinc-600 dark:text-zinc-400">
                    for reason in why { <li>(reason)</li> }
                </ul>
            }
        </div>
    })
}

/// A card with no `title` is on its own ticket's page, so it leaves out the
/// header that names the ticket.
#[component]
pub async fn ask_card(
    ask: AskView,
    title: String,
    project: String,
    compact: bool,
) -> topcoat::Result<impl View> {
    let summary = summarise(&ask);
    let why = reasons(&ask, &summary);
    let questions = if ask.kind == "question" {
        questions(&ask.input)
    } else {
        Vec::new()
    };
    let always = always_prefix(&ask);
    let raw = serde_json::to_string_pretty(&ask.input).unwrap_or_default();
    let submit = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    let big = "min-h-10 w-full sm:min-h-0 sm:w-auto";
    let primary = format!("{} {big}", ui::PRIMARY);
    let danger = format!("{} {big}", ui::DANGER);
    let pad = if compact { "p-2.5" } else { "p-3" };
    Ok(view! {
        <article
            id=(format!("ask-{}", ask.id))
            class=(format!("min-w-0 rounded-lg border border-amber-300 bg-white {pad} dark:border-amber-800 dark:bg-zinc-900"))
        >
            if !compact && !title.is_empty() {
                <div class="flex items-center gap-2 text-[12px]">
                    <a href=(format!("/tickets/{}", ask.ticket)) class=(format!("shrink-0 font-mono hover:text-zinc-900 dark:hover:text-zinc-100 {}", ui::MUTED))>
                        (ask.ticket.clone())
                    </a>
                    <span title=(title.clone()) class="min-w-0 flex-1 truncate text-[13px] font-medium">(title.clone())</span>
                    <span class="ml-auto min-w-0 max-w-[40%] shrink-0">ui::badge(text: ask.hire.clone())</span>
                </div>
            }
            ask_body(summary: summary, tool: ask.tool.clone(), question: ask.kind == "question", why: why)
            <form data-on:submit=(submit) class="mt-3 space-y-3">
                for (text, options) in questions {
                    <fieldset>
                        <legend class="mb-1">(text.clone())</legend>
                        <div class="flex flex-wrap gap-x-4 gap-y-2">
                            for option in options {
                                <label class="flex min-h-8 items-center gap-1.5">
                                    <input type="radio" name=(text.clone()) value=(option.clone())>
                                    (option)
                                </label>
                            }
                        </div>
                    </fieldset>
                }
                if ask.kind != "question" {
                    <details class="text-[12px]">
                        <summary class=(format!("cursor-pointer select-none {}", ui::MUTED))>"Raw input"</summary>
                        <pre class="mt-1 max-h-64 overflow-auto rounded-md bg-zinc-100 p-2 font-mono text-[12px] dark:bg-zinc-950">(raw)</pre>
                    </details>
                }
                if compact {
                    <details class="text-[12px]">
                        <summary class=(format!("cursor-pointer select-none {}", ui::MUTED))>"Add a note"</summary>
                        <input name=(MESSAGE) placeholder="Note for the agent (a deny with a note lets it carry on)" class=(format!("{} mt-1 !text-[12px]", ui::INPUT))>
                    </details>
                } else {
                    <input name=(MESSAGE) placeholder="Note for the agent (a deny with a note lets it carry on)" class=(format!("{} !text-[12px]", ui::INPUT))>
                }
                <div class="flex flex-col gap-2 sm:flex-row sm:flex-wrap">
                    <button type="submit" name=(DECISION) value="approve" class=(primary)>"Allow"</button>
                    <button type="submit" name=(DECISION) value="deny" class=(danger)>"Deny"</button>
                    if let Some(prefix) = always {
                        <button
                            type="submit"
                            name=(DECISION)
                            value=(ALWAYS)
                            class=(format!("{} min-h-10 w-full text-[12px] sm:min-h-0 sm:w-auto", ui::BUTTON))
                        >
                            <span>"Always allow " <code class="font-mono break-all">(prefix)</code> " in " (project)</span>
                        </button>
                    }
                </div>
            </form>
        </article>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ask(kind: &str, tool: &str, input: Value) -> AskView {
        AskView {
            id: "ASK-1".into(),
            ticket: "GAME-1".into(),
            hire: "dev".into(),
            kind: kind.into(),
            tool: tool.into(),
            input,
            reasons: Vec::new(),
        }
    }

    #[test]
    fn bash_shows_its_command_and_description() {
        let a = ask(
            "permission",
            "Bash",
            json!({"command": "cargo test --locked", "description": "Run the tests"}),
        );
        let s = summarise(&a);
        assert_eq!(s.command.as_deref(), Some("cargo test --locked"));
        assert_eq!(s.description.as_deref(), Some("Run the tests"));
        assert_eq!(reasons(&a, &s), ["No rule allows this yet"]);
        assert_eq!(always_prefix(&a).as_deref(), Some("cargo test"));
    }

    #[test]
    fn chained_commands_cannot_be_always_allowed() {
        let a = ask(
            "permission",
            "Bash",
            json!({"command": "cargo test && rm x"}),
        );
        assert_eq!(always_prefix(&a), None);
    }

    #[test]
    fn files_outside_the_worktree_are_flagged() {
        let a = ask("permission", "Read", json!({"file_path": "/etc/hosts"}));
        let s = summarise(&a);
        assert_eq!(s.path.as_deref(), Some("/etc/hosts"));
        assert!(s.outside);
        assert_eq!(reasons(&a, &s), ["It's outside the project's folder"]);

        let inside = ask(
            "permission",
            "Read",
            json!({"file_path": "/p/game/.studio/worktrees/GAME-1/src/main.rs"}),
        );
        let s = summarise(&inside);
        assert_eq!(s.path.as_deref(), Some("src/main.rs"));
        assert!(!s.outside);
    }

    #[test]
    fn edits_preview_old_and_new() {
        let long = "x".repeat(900);
        let a = ask(
            "permission",
            "Edit",
            json!({"file_path": "src/a.rs", "old_string": "a\nb", "new_string": long}),
        );
        let s = summarise(&a);
        let (old, new) = s.change.unwrap();
        assert_eq!(old, "a\nb");
        assert!(new.chars().count() <= PREVIEW_CHARS + 1 && new.ends_with('…'));
    }

    #[test]
    fn other_tools_get_pretty_json() {
        let a = ask("permission", "WebFetch", json!({"url": "https://x.test"}));
        assert!(summarise(&a).json.unwrap().contains("\"url\""));
    }

    #[test]
    fn the_saved_rule_loads_as_an_allow_gate() {
        let (name, text) = always_rule("My Game", "cargo test");
        assert_eq!(name, "allow-my-game-cargo-test");
        studio_config::check_rule(&name, &text).unwrap();
        assert!(text.contains(r#"action: "bash:cargo test""#));
        assert!(text.contains(r#"projects: ["My Game"]"#));
    }

    #[tokio::test]
    async fn the_card_escapes_the_command() {
        let a = ask(
            "permission",
            "Bash",
            json!({"command": "echo '<script>x</script>'"}),
        );
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, title: "T <b>".to_string(), project: "game".to_string(), compact: false) },
        )
        .await
        .unwrap();
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("Always allow"), "{html}");
        assert!(html.contains("T &lt;b&gt;"), "{html}");
    }

    #[tokio::test]
    async fn a_path_scrolls_in_its_own_block() {
        let a = ask("permission", "Read", json!({"file_path": "/etc/hosts"}));
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, title: String::new(), project: String::new(), compact: false) },
        )
        .await
        .unwrap();
        let block = html.split_once("<pre").unwrap().1;
        assert!(block.contains("overflow-auto whitespace-pre "), "{html}");
        assert!(block.contains("/etc/hosts</pre>"), "{html}");
        assert!(!html.contains("break-all"), "{html}");
    }
}
