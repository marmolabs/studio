//! The card for one ask, on its ticket page: what is asked, why it was asked, and the buttons to answer it.

use serde_json::Value;
use studio_config::draft::slug;
use studio_core::ticket::Ticket;
use studiod::api::AskView;
use topcoat::view::{View, component, view};

use crate::summary::{file_path, project_path, truncate};
use crate::{ui, words};

/// Form fields that aren't answers to a question.
pub const DECISION: &str = "decision";
pub const MESSAGE: &str = "message";
/// The `decision` value that approves and also saves an allow rule.
pub const ALWAYS: &str = "always";

const COMMAND_SHOWN: usize = 1200;
const PREVIEW_CHARS: usize = 400;
const PREVIEW_LINES: usize = 8;
const QUOTE_SHOWN: usize = 280;

/// What an ask is about, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
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
    match ask.tool.as_str() {
        // A critic failed the work as often as it may; the reasons are its
        // notes. A question shows its own text.
        _ if matches!(ask.kind.as_str(), "question" | "gate") => {}
        _ if ask.kind == "change" => {
            let files: Vec<&str> = input["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            summary.command = Some(truncate(&files.join("\n"), COMMAND_SHOWN));
            summary.description = input["message"]
                .as_str()
                .filter(|m| !m.trim().is_empty())
                .map(str::to_string);
        }
        "Bash" if input["command"].is_string() => {
            summary.command = input["command"]
                .as_str()
                .map(|c| truncate(c, COMMAND_SHOWN));
            summary.description = input["description"]
                .as_str()
                .filter(|d| !d.trim().is_empty())
                .map(str::to_string);
        }
        "Read" | "Edit" | "Write" | "MultiEdit" if file_path(input).is_some() => {
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
        _ => {
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

/// When an ask opened; studiods before `at_ms` existed send 0.
pub fn opened(ask: &AskView) -> Option<u64> {
    (ask.at_ms > 0).then_some(ask.at_ms)
}

/// The rule a reason names: the daemon writes a rule's finding as
/// "why (rule)", and the approver's as "Approver: why".
fn rule_named(reason: &str) -> Option<&str> {
    if reason.starts_with("Approver:") {
        return None;
    }
    let name = reason.rsplit_once(" (")?.1.strip_suffix(')')?;
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
    plain.then_some(name)
}

/// Where "Add a rule" goes: the rule editor, filled in with an allow rule
/// for the command's prefix when there is one.
fn rule_link(ask: &AskView, project: &str) -> String {
    let Some(prefix) = always_prefix(ask).filter(|_| !project.is_empty()) else {
        return "/rules?new=1".to_string();
    };
    let (name, text) = always_rule(project, &prefix);
    let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!("/rules?new=1&name={}&text={}", encode(&name), encode(&text))
}

/// A command or path on one line, for a compact card.
const CMD_LINE: &str = "truncate rounded-lg border border-line bg-bg-2 px-2.5 py-[7px] font-mono text-[12px] leading-5 text-fg";

/// The command, file, change or input an ask is about. A compact card keeps
/// a command or path to one line, and names the first question rather than
/// asking it.
#[component]
async fn ask_target(
    summary: Summary,
    question: Option<String>,
    compact: bool,
) -> topcoat::Result<impl View> {
    let Summary {
        command,
        path,
        outside,
        change,
        more_edits,
        json,
        ..
    } = summary;
    Ok(view! {
        if compact {
            if let Some(command) = command {
                <div class=(CMD_LINE) title=(command.clone())>(command)</div>
            } else if let Some((old, new)) = change {
                <div class="overflow-hidden rounded-lg border border-line font-mono text-[12px] leading-[18px]">
                    <div class="border-b border-line bg-bg-2 px-3 py-1.5 text-fg-2">(path.unwrap_or_default())</div>
                    <div class="whitespace-pre-wrap break-all bg-bad-bg px-3 py-0.5 text-bad-fg">"- " (old)</div>
                    <div class="whitespace-pre-wrap break-all bg-ok-bg px-3 py-0.5 text-ok-fg">"+ " (new)</div>
                </div>
            } else if let Some(path) = path {
                <div class=(CMD_LINE) title=(path.clone())>(path)</div>
            } else if let Some(text) = question {
                <p class="text-[13px]">(text)</p>
            } else if let Some(json) = json {
                <div class=(format!("{CMD_LINE} max-h-40 overflow-auto"))>(json)</div>
            }
        } else {
            if let Some(command) = command {
                <pre class=(format!("max-h-48 {}", ui::CMD))>(command)</pre>
            }
            if let Some((old, new)) = change {
                ask_change(path: path.unwrap_or_default(), old: old, new: new, more: more_edits)
            } else if let Some(path) = path {
                <pre class=(format!("max-h-48 {}", ui::CMD))>(path)</pre>
            }
            if outside {
                <span>ui::pill(tone: "attn", label: "Outside the project")</span>
            }
            if let Some(json) = json {
                <pre class=(format!("max-h-48 {}", ui::CMD))>(json)</pre>
            }
        }
    })
}

/// An edit as a small diff: the file, then the old lines and the new.
#[component]
async fn ask_change(
    path: String,
    old: String,
    new: String,
    more: usize,
) -> topcoat::Result<impl View> {
    let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();
    let (old, new) = (lines(&old), lines(&new));
    Ok(view! {
        <div class="overflow-hidden rounded-lg border border-line font-mono text-[12.5px] leading-5">
            <div class="truncate border-b border-line bg-bg-2 px-3.5 py-2 text-fg-2">(path)</div>
            <div class="max-h-64 overflow-auto">
                for line in old {
                    <div class="whitespace-pre bg-bad-bg px-3.5 text-bad-fg">"- " (line)</div>
                }
                for line in new {
                    <div class="whitespace-pre bg-ok-bg px-3.5 text-ok-fg">"+ " (line)</div>
                }
            </div>
            if more > 0 {
                <div class="border-t border-line bg-bg-2 px-3.5 py-1.5 font-sans text-[12.5px] text-fg-3">"and " (more) " more edits"</div>
            }
        </div>
    })
}

/// What the agent said just before it asked, under its avatar.
#[component]
async fn said(hire: String, quote: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="flex gap-2.5 text-fg-2">
            <span class="mt-px">ui::avatar(hire: hire)</span>
            <span class="min-w-0 [overflow-wrap:anywhere]">"“" (quote) "”"</span>
        </div>
    })
}

const NOTE_TOGGLE: &str = "inline-flex cursor-pointer select-none items-center gap-1.5 px-0.5 py-1 text-[13px] text-fg-3 hover:text-fg [&::-webkit-details-marker]:hidden";

/// Reasons as sentences: "No rule allows it."
fn sentences(reasons: &[String]) -> String {
    reasons
        .iter()
        .map(|r| {
            let r = r.trim();
            let mut chars = r.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            let end = if r.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            format!("{}{}{end}", first.unwrap_or_default(), chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One ask, with its answers: Allow, Deny, Always allow with the rule it
/// would save, and a note for the agent. On its own ticket's page and in the
/// ticket's panel it is whole, with `quote` what the agent said just before
/// asking; `compact`, in a list of what needs the owner, it is small and has
/// Details, which opens the ticket's panel. `now` is the clock the age is
/// told by; 0 means the real one.
#[component]
pub async fn ask_card(
    ask: AskView,
    project: String,
    #[default] quote: Option<String>,
    #[default] compact: bool,
    #[default] now: u64,
) -> topcoat::Result<impl View> {
    let summary = summarise(&ask);
    let reasons = reasons(&ask, &summary);
    let why = reasons
        .iter()
        .map(|r| match rule_named(r) {
            Some(name) => r.trim_end_matches(&format!(" ({name})")).to_string(),
            None => r.clone(),
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let brief = sentences(&reasons);
    let quote = quote.or_else(|| summary.description.clone());
    let question = ask.kind == "question";
    let all_questions = if question {
        questions(&ask.input)
    } else {
        Vec::new()
    };
    let first_question = all_questions.first().map(|(text, _)| text.clone());
    let always = always_prefix(&ask);
    let (rule_label, rule_href) = match ask.reasons.iter().find_map(|r| rule_named(r)) {
        Some(name) => (name.to_string(), format!("/rules?file={name}")),
        None => ("Add a rule".to_string(), rule_link(&ask, &project)),
    };
    let submit = format!("@post('/asks/{}', {{contentType: 'form'}})", ask.id);
    let id = format!("ask-{}", ask.id);
    let clock = if now > 0 { now } else { ui::now_ms() };
    let age = opened(&ask).map(|at| super::timeline::relative(clock, at));
    let ago = age.clone().unwrap_or_default();
    let short = age.map(|age| age.trim_end_matches(" ago").to_string());
    let details = super::office::select(&super::office::key("tickets", &ask.ticket));
    let save_title = always
        .as_ref()
        .map(|prefix| format!("Allow and save a rule for {prefix} in {project}"));
    let head = words::ask_head(&ask);
    let title = words::ask_title(&ask);
    Ok(view! {
        if compact {
            <div id=(id) class="overflow-hidden rounded-[10px] border border-attn-line">
                <div class="flex items-center gap-2 bg-attn-bg px-3 py-2 text-[12.5px] font-medium text-attn-fg">
                    ui::icon(name: "shield-alert", small: true)
                    <span class="font-mono">(ask.ticket.clone())</span>
                    <span class="truncate">(title)</span>
                    if let Some(short) = short {
                        <span class="ml-auto shrink-0 font-normal">(short)</span>
                    }
                </div>
                <form class="flex flex-col gap-2.5 px-3 pb-3 pt-2.5" data-on:submit=(submit)>
                    ask_target(summary: summary, question: first_question, compact: true)
                    if !question {
                        <p class="text-[12.5px] leading-[18px] text-fg-3">(brief)</p>
                        <input name=(MESSAGE) placeholder="Note for the agent (optional)" title="A deny with a note lets the agent carry on another way." class=(format!("{} text-[13px]", ui::INPUT))>
                    }
                    <div class="flex flex-wrap items-center gap-2">
                        if !question {
                            <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY_SM)>"Allow"</button>
                            <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON_SM)>"Deny"</button>
                            if let Some(prefix) = always.clone() {
                                <button type="submit" name=(DECISION) value=(ALWAYS) class=(ui::GHOST_SM) title=(save_title.clone().unwrap_or_default())>
                                    "Always allow " <span class=(ui::CODE)>(prefix)</span>
                                </button>
                            }
                        }
                        <button type="button" class=(if question { ui::BUTTON_SM } else { ui::GHOST_SM }) data-on:click=(details)>
                            (if question { "Answer" } else { "Details" })
                        </button>
                    </div>
                </form>
            </div>
        } else {
            <div id=(id) class="min-w-0">
                ui::ask_frame(
                    head: view! { (head.clone()) },
                    ago: ago,
                    ask_target(summary: summary, question: first_question, compact: false)
                    if let Some(quote) = quote {
                        said(hire: ask.hire.clone(), quote: truncate(&quote, QUOTE_SHOWN))
                    }
                    if !question {
                        <p class=(ui::WHY)>
                            <span class="mr-1.5 inline-block align-[-2px]">ui::icon(name: "shield-check", small: true)</span>
                            (why)
                            <a href=(rule_href) class="ml-1.5 text-fg-2 underline decoration-line-3 underline-offset-3 hover:text-fg">(rule_label)</a>
                        </p>
                    }
                    <form data-on:submit=(submit) class="flex flex-col gap-3.5">
                        for (text, options) in all_questions {
                            <fieldset class="flex flex-col gap-2">
                                <legend class="mb-2 font-medium">(text.clone())</legend>
                                <div class="flex flex-wrap gap-x-4 gap-y-2">
                                    for option in options {
                                        <label class="flex min-h-8 items-center gap-2">
                                            <input type="radio" name=(text.clone()) value=(option.clone()) class="accent-[var(--primary)]">
                                            (option)
                                        </label>
                                    }
                                </div>
                            </fieldset>
                        }
                        <div class=(ui::ASK_ACTIONS)>
                            <button type="submit" name=(DECISION) value="approve" class=(ui::PRIMARY)>
                                ui::icon(name: "check")
                                if question { "Answer" } else { "Allow" }
                            </button>
                            <button type="submit" name=(DECISION) value="deny" class=(ui::BUTTON)>
                                ui::icon(name: "x")
                                "Deny"
                            </button>
                            if let Some(prefix) = always {
                                <button type="submit" name=(DECISION) value=(ALWAYS) class=(format!("{} max-[760px]:whitespace-normal", ui::GHOST)) title=(save_title.clone().unwrap_or_default())>
                                    <span>"Always allow " <span class=(ui::CODE)>(prefix)</span> " in " (project)</span>
                                </button>
                            }
                            // On the right, or a row of their own on a phone or
                            // while the note is open.
                            <div class="ml-auto flex flex-wrap items-center gap-x-3 gap-y-1 has-[details[open]]:basis-full max-[760px]:ml-0 max-[760px]:basis-full">
                                <details id=(format!("note-{}", ask.id)) data-preserve-attr="open" class="group [&[open]]:basis-full">
                                    <summary class=(format!("{NOTE_TOGGLE} group-open:flex group-open:w-fit"))>
                                        ui::icon(name: "message-square-text", small: true)
                                        "Add a note"
                                    </summary>
                                    <input name=(MESSAGE) aria-label="Note for the agent" placeholder="Sent with Deny: the agent reads it and carries on." class=(format!("mt-2 {}", ui::INPUT))>
                                </details>
                            </div>
                        </div>
                    </form>
                )
            </div>
        }
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
            at_ms: 0,
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
            view! { cx => ask_card(ask: a, project: "game".to_string()) },
        )
        .await
        .unwrap();
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("Always allow"), "{html}");
    }

    #[tokio::test]
    async fn a_path_shows_whole_in_its_own_block() {
        let a = ask("permission", "Read", json!({"file_path": "/etc/hosts"}));
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, project: String::new()) },
        )
        .await
        .unwrap();
        let block = html.split_once("<pre").unwrap().1;
        // Wrapped rather than cut, so all of it is seen before it is allowed.
        assert!(block.contains("whitespace-pre-wrap"), "{html}");
        assert!(block.contains("/etc/hosts</pre>"), "{html}");
        assert!(!html.contains("break-all"), "{html}");
    }

    #[test]
    fn a_change_lists_its_files_and_message() {
        let change = ask(
            "change",
            "commit",
            json!({"files": ["a.rs", "b.rs"], "message": "Add waves"}),
        );
        let s = summarise(&change);
        assert_eq!(s.command.as_deref(), Some("a.rs\nb.rs"));
        assert_eq!(s.description.as_deref(), Some("Add waves"));
    }

    #[tokio::test]
    async fn the_card_quotes_the_agent_and_links_a_rule() {
        let mut a = ask("permission", "Bash", json!({"command": "cargo run"}));
        a.at_ms = ui::now_ms() - 120_000;
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, project: "game".to_string(),
            quote: Some("I want to <watch> it".to_string())) },
        )
        .await
        .unwrap();
        assert!(html.contains("Wants to run a command"), "{html}");
        assert!(html.contains("2m ago"), "{html}");
        assert!(html.contains("“I want to &lt;watch&gt; it”"), "{html}");
        assert!(
            html.contains("/rules?new=1&amp;name=allow-game-cargo-run"),
            "{html}"
        );
        assert!(html.contains("Add a note"), "{html}");
    }

    #[tokio::test]
    async fn a_reason_links_the_rule_it_names() {
        assert_eq!(
            rule_named("Ask first (protect-player)"),
            Some("protect-player")
        );
        assert_eq!(rule_named("Approver: it opens a window (maybe)"), None);
        assert_eq!(rule_named("Looks fine (to me)"), None);
        let mut a = ask("permission", "Edit", json!({"file_path": "src/player.rs"}));
        a.reasons = vec!["Ask first (protect-player)".into()];
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, project: "game".to_string()) },
        )
        .await
        .unwrap();
        assert!(html.contains("Ask first"), "{html}");
        // The rule's name is the link to its editor.
        assert_eq!(
            crate::probe::find(&html, "href", "/rules?file=protect-player").text(),
            "protect-player",
            "{html}"
        );
    }

    #[tokio::test]
    async fn an_edit_shows_as_a_diff() {
        let a = ask(
            "permission",
            "Edit",
            json!({"file_path": "src/player.rs", "old_string": "a = 1;", "new_string": "a = 2;"}),
        );
        let cx = topcoat::context::Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => ask_card(ask: a, project: "game".to_string()) },
        )
        .await
        .unwrap();
        assert!(html.contains(">src/player.rs</div>"), "{html}");
        assert!(
            html.contains("- a = 1;") && html.contains("+ a = 2;"),
            "{html}"
        );
    }

    async fn render(ask: AskView, project: &str, compact: bool, now: u64) -> String {
        let cx = topcoat::context::Cx::default();
        ui::html(
            &cx,
            view! { cx => ask_card(ask: ask, project: project.to_string(), compact: compact, now: now) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_compact_card_has_the_same_answers_in_a_small_form() {
        let mut a = ask("permission", "Bash", json!({"command": "cargo test"}));
        a.reasons = vec!["no rule <allows> it".into()];
        a.at_ms = 1_000_000;
        let html = render(a.clone(), "game", true, 1_120_000).await;
        // The same post, buttons and note as the whole card.
        assert!(
            html.contains("@post('/asks/ASK-1', {contentType: 'form'})"),
            "{html}"
        );
        assert!(html.contains(r#"value="approve""#), "{html}");
        assert!(html.contains(r#"value="deny""#), "{html}");
        assert!(html.contains(r#"value="always""#), "{html}");
        assert!(html.contains("Always allow"), "{html}");
        assert!(
            html.contains("Allow and save a rule for cargo test in game"),
            "{html}"
        );
        assert!(html.contains(r#"name="message""#), "{html}");
        // Details opens the ticket's panel; the age is short.
        assert!(html.contains("Details"), "{html}");
        assert!(html.contains("$sel = 'tickets/GAME-1'"), "{html}");
        assert!(html.contains(">2m<"), "{html}");
        assert!(html.contains("GAME-1"), "{html}");
        // What the agent asked is escaped; the reason reads as a sentence.
        assert!(
            html.contains(r#"title="cargo test">cargo test</div>"#),
            "{html}"
        );
        assert!(html.contains("No rule &lt;allows&gt; it."), "{html}");
        assert!(!html.contains("<allows>"), "{html}");
        // It is the small card: no frame, no note toggle.
        assert!(!html.contains("Add a note"), "{html}");
        assert!(!html.contains("Waiting for you"), "{html}");

        // The whole card has the same actions and no Details.
        let full = render(a, "game", false, 1_120_000).await;
        for same in [
            "@post('/asks/ASK-1', {contentType: 'form'})",
            r#"value="approve""#,
            r#"value="deny""#,
            r#"value="always""#,
            "Allow and save a rule for cargo test in game",
            r#"name="message""#,
            "2m ago",
            "Add a note",
        ] {
            assert!(full.contains(same), "{same}: {full}");
        }
        assert!(!full.contains("Details"), "{full}");
    }

    #[tokio::test]
    async fn a_question_is_answered_in_the_whole_card_and_opened_from_the_compact_one() {
        let q = ask(
            "question",
            "AskUserQuestion",
            json!([{"question": "Which <level>?", "options": ["One", "Two"]}]),
        );
        let compact = render(q.clone(), "game", true, 0).await;
        assert!(compact.contains("Which &lt;level&gt;?"), "{compact}");
        assert!(compact.contains("Answer"), "{compact}");
        assert!(compact.contains("$sel = 'tickets/GAME-1'"), "{compact}");
        assert!(!compact.contains(r#"value="approve""#), "{compact}");
        assert!(!compact.contains(r#"type="radio""#), "{compact}");
        let full = render(q, "game", false, 0).await;
        assert!(full.contains(r#"type="radio""#), "{full}");
        assert!(full.contains(r#"value="Two""#), "{full}");
        assert!(full.contains(r#"value="approve""#), "{full}");
    }
}
