//! Edit the studio's hires and rules. studiod validates and writes; this
//! page only shows the files and sends the text back. Rules from packs and
//! projects are listed too, read-only: they live in those folders.

use futures_util::Stream;
use futures_util::stream;
use serde::Deserialize;
use studiod::api::{FileKind, FileView, Request, RuleFile, RuleTest};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::{
        content::{
            Form,
            sse::{Event, Sse},
        },
        page, path_param, query_params, route,
    },
    view::{View, component, view},
};

use super::hires;
use crate::{AppConfig, client, ui};

path_param!(name);

const HIRE_TEMPLATE: &str = "---\nname: New hire\nrole: engineer\nruntime: claude-code\nmodel: sonnet\n---\nYou are a careful engineer. Describe how you work here.\n";
const RULE_TEMPLATE: &str =
    "---\nkind: guide\nlevel: warn\n---\nWhy this rule exists, in a sentence or two.\n";

/// How far back the Test button looks, and how many matches it lists.
const TEST_DAYS: u64 = 7;
const DAY_MS: u64 = 86_400_000;
const HITS_SHOWN: usize = 50;
/// How much of a rule's text the list shows.
const PREVIEW_CHARS: usize = 80;

/// What differs between the two editable folders.
struct Kind {
    files: FileKind,
    /// The URL segment, e.g. `hires`.
    base: &'static str,
    template: &'static str,
}

const HIRES: Kind = Kind {
    files: FileKind::Hires,
    base: "hires",
    template: HIRE_TEMPLATE,
};

const RULES: Kind = Kind {
    files: FileKind::Rules,
    base: "rules",
    template: RULE_TEMPLATE,
};

#[query_params(error = bad_request)]
struct Selected {
    file: Option<String>,
    /// Where a rule from elsewhere comes from: `pack/<name>` or
    /// `project/<name>`. Missing for the studio's own files.
    from: Option<String>,
    new: Option<String>,
    /// Fills the new file's name, e.g. from a drafted rule.
    name: Option<String>,
    /// Fills the new file's text instead of the template.
    text: Option<String>,
}

/// The file open in the editor when a delete was asked for.
#[query_params(error = bad_request)]
struct Deleting {
    open: Option<String>,
}

#[derive(Deserialize)]
struct Save {
    name: String,
    text: String,
    /// Present when the form is for a new file, which must not replace one.
    #[serde(default)]
    adding: Option<String>,
}

/// The file in the editor.
#[derive(Clone, Debug, Default)]
struct Open {
    name: String,
    text: String,
    /// Where it is on disk, when studiod says.
    path: String,
    /// Why it can't be edited here, for a pack's or project's rule.
    elsewhere: String,
}

#[page("/hires")]
async fn hires_page(cx: &Cx) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    let all = fetch(cx, &HIRES).await?;
    let query = query_params::<Selected>(cx)?;
    let adding = query.new.is_some();
    let open = query
        .file
        .as_ref()
        .and_then(|name| all.iter().find(|f| &f.name == name))
        .map(|f| Open {
            name: f.name.clone(),
            text: f.text.clone(),
            ..Open::default()
        });
    let open = match open {
        None if adding => Some(new_file(&HIRES, query)),
        open => open,
    };
    let list = if open.is_none() {
        hires::hires_for(&config.socket, &all).await
    } else {
        Vec::new()
    };
    // The editor's top bar names the file instead.
    let listing = open.is_none();
    Ok(view! { cx =>
        if listing {
            ui::top_actions(
                <a href="/hires?new" class=(ui::PRIMARY)>
                    ui::icon(name: "plus")
                    <span class=(ui::LABEL)>"New hire"</span>
                </a>
            )
        }
        if let Some(open) = open {
            <div class=(format!("flex min-h-[520px] overflow-hidden {}", ui::CARD))>
                editor(base: "hires", open: open, adding: adding)
            </div>
        } else {
            hires::tables(list: list)
        }
    })
}

#[page("/rules")]
async fn rules_page(cx: &Cx) -> topcoat::Result<impl View> {
    let all = rule_files(cx).await?;
    let query = query_params::<Selected>(cx)?;
    let adding = query.new.is_some();
    let from = query.from.clone().unwrap_or_default();
    // The first rule is open until another is picked.
    let found = match &query.file {
        Some(name) => all.iter().find(|r| &r.name == name && source(r) == from),
        None => all.first(),
    };
    let current = match (adding, found) {
        (false, Some(rule)) => (rule.name.clone(), source(rule)),
        _ => (String::new(), String::new()),
    };
    let open = match found {
        _ if adding => Some(new_file(&RULES, query)),
        found => found.map(open_rule),
    };
    Ok(view! { cx =>
        ui::top_actions(
            <a href="/rules?new" class=(ui::PRIMARY)>
                ui::icon(name: "plus")
                <span class=(ui::LABEL)>"New rule"</span>
            </a>
        )
        <div class="grid min-h-[520px] grid-cols-[300px_minmax(0,1fr)] overflow-hidden rounded-[10px] border border-line max-[760px]:grid-cols-1">
            rule_list(files: all, current: current)
            if let Some(open) = open {
                editor(base: "rules", open: open, adding: adding)
            } else {
                empty_editor(base: "rules", gone: String::new())
            }
        </div>
    })
}

#[route(POST "/hires/save")]
async fn save_hire(
    cx: &Cx,
    Form(form): Form<Save>,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    save(cx, &HIRES, form).await
}

#[route(POST "/rules/save")]
async fn save_rule(
    cx: &Cx,
    Form(form): Form<Save>,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    save(cx, &RULES, form).await
}

/// Tries the rule in the editor on the tool calls of the last week.
#[route(POST "/rules/test")]
async fn test_rule(cx: &Cx, Form(form): Form<Save>) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let request = Request::TestRule {
        text: form.text.replace("\r\n", "\n"),
        since: ui::now_ms().saturating_sub(TEST_DAYS * DAY_MS),
    };
    let (error, test) = match client::call::<RuleTest>(&config.socket, request).await {
        Ok(test) => (String::new(), Some(test)),
        Err(err) => (err.to_string(), None),
    };
    let html = ui::html(cx, view! { cx => rule_result(error: error, test: test) }).await?;
    Ok(PatchElements::new(html))
}

#[route(POST "/hires/{name}/delete")]
async fn delete_hire(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    delete(cx, &HIRES).await
}

#[route(POST "/rules/{name}/delete")]
async fn delete_rule(
    cx: &Cx,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    delete(cx, &RULES).await
}

async fn fetch(cx: &Cx, kind: &Kind) -> topcoat::Result<Vec<FileView>> {
    let config = app_context::<AppConfig>(cx);
    client::call(&config.socket, Request::Files { kind: kind.files }).await
}

async fn rule_files(cx: &Cx) -> topcoat::Result<Vec<RuleFile>> {
    let config = app_context::<AppConfig>(cx);
    client::call(&config.socket, Request::RuleFiles).await
}

/// Where a rule comes from, as the `from` query names it: empty for the
/// studio's own.
fn source(rule: &RuleFile) -> String {
    if rule.origin == "studio" {
        String::new()
    } else {
        format!("{}/{}", rule.origin, rule.owner)
    }
}

fn open_rule(rule: &RuleFile) -> Open {
    let elsewhere = match rule.origin.as_str() {
        "pack" => format!("Comes with the {} pack; change it there.", rule.owner),
        "project" => format!("Lives in the {} project; change it there.", rule.owner),
        _ => String::new(),
    };
    Open {
        name: rule.name.clone(),
        text: rule.text.clone(),
        path: rule.path.clone(),
        elsewhere,
    }
}

/// A new file's name and text: from the query, else the template.
fn new_file(kind: &Kind, query: &Selected) -> Open {
    Open {
        name: query.name.clone().unwrap_or_default(),
        text: query
            .text
            .clone()
            .unwrap_or_else(|| kind.template.to_string()),
        ..Open::default()
    }
}

async fn save(
    cx: &Cx,
    kind: &Kind,
    form: Save,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx);
    // Browsers submit textareas with CRLF line ends.
    let text = form.text.replace("\r\n", "\n");
    let mut events: Vec<Event> = Vec::new();
    if form.adding.is_some() && fetch(cx, kind).await?.iter().any(|f| f.name == form.name) {
        let taken = format!(
            "{} already exists; open it from the list to change it.",
            form.name
        );
        events.push(ui::notice(cx, taken, true).await?.into());
        return Ok(Sse::new(stream::iter(events.into_iter().map(Ok))));
    }
    let request = Request::SaveFile {
        kind: kind.files,
        name: form.name.clone(),
        text: text.clone(),
    };
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            events.push(
                ui::notice(cx, format!("Saved {}.", form.name), false)
                    .await?
                    .into(),
            );
            if kind.base == "rules" {
                events.push(list_patch(cx, &form.name).await?.into());
            }
            if form.adding.is_some() {
                // The new file is now an ordinary one: name fixed, deletable.
                let path = if kind.base == "rules" {
                    rule_files(cx)
                        .await?
                        .into_iter()
                        .find(|r| r.origin == "studio" && r.name == form.name)
                        .map(|r| r.path)
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let open = Open {
                    name: form.name,
                    text,
                    path,
                    elsewhere: String::new(),
                };
                let base = kind.base;
                let saved = ui::html(
                    cx,
                    view! { cx => editor(base: base, open: open, adding: false) },
                )
                .await?;
                events.push(PatchElements::new(saved).into());
            }
        }
        Err(err) => events.push(ui::notice(cx, err.to_string(), true).await?.into()),
    }
    Ok(Sse::new(stream::iter(events.into_iter().map(Ok))))
}

async fn delete(
    cx: &Cx,
    kind: &Kind,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx);
    let target = path_param::<Name>(cx).to_owned();
    let open = query_params::<Deleting>(cx)?
        .open
        .clone()
        .unwrap_or_default();
    let request = Request::DeleteFile {
        kind: kind.files,
        name: target.clone(),
    };
    let mut events: Vec<Event> = Vec::new();
    match client::call::<serde_json::Value>(&config.socket, request).await {
        Ok(_) => {
            events.push(
                ui::notice(cx, format!("Deleted {target}."), false)
                    .await?
                    .into(),
            );
            // Any other open file keeps its unsaved text.
            let current = if open == target { "" } else { open.as_str() };
            if kind.base == "rules" {
                events.push(list_patch(cx, current).await?.into());
            }
            if open == target {
                let base = kind.base;
                let cleared =
                    ui::html(cx, view! { cx => empty_editor(base: base, gone: target) }).await?;
                events.push(PatchElements::new(cleared).into());
            }
        }
        Err(err) => events.push(ui::notice(cx, err.to_string(), true).await?.into()),
    }
    Ok(Sse::new(stream::iter(events.into_iter().map(Ok))))
}

/// The rule list again, with the studio's rule `current` open.
async fn list_patch(cx: &Cx, current: &str) -> topcoat::Result<PatchElements> {
    let all = rule_files(cx).await?;
    let current = (current.to_owned(), String::new());
    let html = ui::html(cx, view! { cx => rule_list(files: all, current: current) }).await?;
    Ok(PatchElements::new(html))
}

/// The link to a rule in the editor. Pack and project rules can have any
/// file name, so the query is encoded.
fn rule_href(name: &str, from: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("file", name);
    if !from.is_empty() {
        query.append_pair("from", from);
    }
    format!("/rules?{}", query.finish())
}

/// Names the daemon accepts, so one is safe to put in a URL.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The first paragraph of a rule's text after its frontmatter, as one line
/// cut short.
fn preview(text: &str) -> String {
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or(text);
    let paragraph: Vec<&str> = body
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect();
    crate::summary::truncate(&paragraph.join(" "), PREVIEW_CHARS)
}

/// A rule's kind and level, from its frontmatter.
#[derive(Debug, Default, Deserialize, PartialEq)]
struct Head {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    level: String,
}

fn head(text: &str) -> Head {
    studio_config::frontmatter::split(text)
        .ok()
        .flatten()
        .and_then(|(yaml, _)| serde_yaml_ng::from_str(yaml).ok())
        .unwrap_or_default()
}

/// The pill tone for a rule's level.
fn level_tone(level: &str) -> &'static str {
    match level {
        "block" => "bad",
        "ask" => "attn",
        "allow" => "ok",
        _ => "neutral",
    }
}

/// `path` with the owner's home folder written as `~`.
fn shown_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// "gate · block", plus "off" for a pack rule turned off.
fn kind_line(rule: &RuleFile) -> String {
    let head = head(&rule.text);
    let mut words: Vec<&str> = [head.kind.as_str(), head.level.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if rule.disabled {
        words.push("off");
    }
    words.join(" · ")
}

/// The heading over each origin's rules.
fn origin_label(rule: &RuleFile) -> String {
    match rule.origin.as_str() {
        "pack" => format!("Pack {}", rule.owner),
        "project" => format!("Project {}", rule.owner),
        _ => "Studio".to_string(),
    }
}

/// Rules grouped by where they come from. `current` is the open rule's
/// (name, source).
#[component]
async fn rule_list(files: Vec<RuleFile>, current: (String, String)) -> topcoat::Result<impl View> {
    let mut groups: Vec<(String, Vec<RuleFile>)> = Vec::new();
    for rule in files {
        let label = origin_label(&rule);
        match groups.last_mut() {
            Some((last, rules)) if *last == label => rules.push(rule),
            _ => groups.push((label, vec![rule])),
        }
    }
    Ok(view! {
        <nav id="file-list" aria-label="Rules" class="flex min-w-0 flex-col border-r border-line bg-bg-2 pb-2 max-[760px]:border-b max-[760px]:border-r-0">
            if groups.is_empty() {
                <p class=(format!("px-3.5 py-3.5 {}", ui::MUTED))>"No rules yet."</p>
            }
            for (label, rules) in groups {
                <div class="px-3.5 pb-1.5 pt-3.5 text-[12px] font-medium leading-4 text-fg-3">(label)</div>
                #[key(format!("{}/{}", source(&rule), rule.name))]
                for rule in rules {
                    let from = source(&rule);
                    let href = rule_href(&rule.name, &from);
                    let here = current == (rule.name.clone(), from);
                    let kind = kind_line(&rule);
                    let note = preview(&rule.text);
                    <a
                        href=(href)
                        aria-current=(if here { "true" } else { "false" })
                        class="flex min-w-0 flex-col gap-1 border-l-2 border-transparent px-3.5 py-2.5 hover:bg-bg-3 aria-[current=true]:border-fg aria-[current=true]:bg-bg-3"
                    >
                        <span class="flex items-center gap-2 font-medium">
                            <span class="min-w-0 truncate">(rule.name.clone())</span>
                            <span class="ml-auto shrink-0 text-[11.5px] font-medium leading-none text-fg-3">(kind)</span>
                        </span>
                        if let Some(problem) = rule.problem {
                            <span class="truncate text-[12.5px] text-bad-fg" title=(problem.clone())>"Doesn't load: " (problem)</span>
                        } else if !note.is_empty() {
                            <span class="truncate text-[12.5px] text-fg-3">(note)</span>
                        }
                    </a>
                }
            }
        </nav>
    })
}

/// What a rule would have done, or why it can't be tried.
#[component]
async fn rule_result(error: String, test: Option<RuleTest>) -> topcoat::Result<impl View> {
    let test = test.unwrap_or_else(|| RuleTest {
        rule: String::new(),
        scanned: 0,
        hits: Vec::new(),
    });
    let summary = format!(
        "Would have matched {} of {} tool calls in the last {TEST_DAYS} days.",
        test.hits.len(),
        test.scanned
    );
    Ok(view! {
        <div id="rule-test" class="space-y-1 border-t border-line px-[18px] py-3 text-[13px]">
            if error.is_empty() {
                <p class=(ui::SECONDARY)>(summary)</p>
                <ul class="space-y-0.5">
                    for hit in test.hits.into_iter().take(HITS_SHOWN) {
                        <li class="flex min-w-0 items-baseline gap-2 text-[12px]">
                            <a href=(format!("/tickets/{}", hit.ticket)) class=(format!("shrink-0 font-mono hover:text-fg {}", ui::MUTED))>(hit.ticket.clone())</a>
                            ui::badge(text: hit.level)
                            <span class=(format!("shrink-0 {}", ui::MUTED))>(hit.tool)</span>
                            <code title=(hit.what.clone()) class="min-w-0 flex-1 truncate font-mono">(hit.what)</code>
                        </li>
                    }
                </ul>
            } else {
                <p class="whitespace-pre-wrap text-bad-fg">(error)</p>
            }
        </div>
    })
}

/// The editor's place when no file is open; `gone` names a file just deleted.
#[component]
async fn empty_editor(base: &'static str, gone: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="editor" class=(format!("flex flex-1 flex-col items-start gap-2 px-[18px] py-4 {}", ui::MUTED))>
            if !gone.is_empty() {
                <p>"Deleted " <span class=(ui::MONO)>(gone)</span> "."</p>
            }
            if base == "rules" {
                <p>"Pick a rule from the list, or add one with New rule."</p>
            } else {
                <a href="/hires" class="hover:text-fg">"Back to all hires"</a>
            }
        </div>
    })
}

/// The editor's text area: the file's text in mono, filling the editor.
/// The file's text: lines don't wrap, so each stays beside its number, and
/// the box grows with the text where the browser can.
const TEXT: &str = "min-w-0 flex-1 resize-none overflow-x-auto whitespace-pre border-0 bg-transparent py-3.5 pl-3 pr-[18px] font-mono text-[13px] leading-[22px] text-fg [field-sizing:content] focus:outline-none";
/// The line numbers beside `TEXT`.
const GUTTER: &str = "m-0 select-none border-r border-line py-3.5 pl-[18px] pr-3 text-right font-mono text-[13px] leading-[22px] text-fg-3 tabular-nums";
/// Rows the editor shows at least.
const ROWS: usize = 18;

/// The open file: a header, its text, and Delete and Save. A rule from a
/// pack or a project is shown read-only.
#[component]
async fn editor(base: &'static str, open: Open, adding: bool) -> topcoat::Result<impl View> {
    let name = open.name.clone();
    let submit = format!("@post('/{base}/save', {{contentType: 'form'}})");
    let delete = format!("@post('/{base}/{name}/delete?open={name}')");
    let editable = open.elsewhere.is_empty();
    let deletable = editable && !adding && safe(&name);
    let rule = base == "rules";
    let head = if rule {
        head(&open.text)
    } else {
        Head::default()
    };
    let (thing, confirm) = if rule {
        ("Delete rule", "Delete this rule?")
    } else {
        ("Delete hire", "Delete this hire?")
    };
    let path = shown_path(&open.path);
    let label = if rule { "Rule file" } else { "Hire file" };
    let lines = open.text.lines().count().max(1);
    let numbers = (1..=lines)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let rows = (lines + 1).max(ROWS).to_string();
    Ok(view! {
        <form id="editor" data-on:submit=(submit) class="flex min-w-0 flex-1 flex-col">
            <div class="flex flex-wrap items-center gap-2.5 border-b border-line px-[18px] py-3.5">
                if adding {
                    <input type="hidden" name="adding" value="1">
                    <input
                        name="name"
                        required=""
                        placeholder="file-name"
                        aria-label="File name"
                        pattern="[a-z0-9][a-z0-9_\\-]{0,63}"
                        title="Lowercase letters, digits, - and _"
                        value=(name.clone())
                        class=(format!("{} max-w-[260px] font-mono", ui::INPUT))
                    >
                } else {
                    <input type="hidden" name="name" value=(name.clone())>
                    if !rule {
                        ui::avatar(hire: name.clone(), large: true)
                    }
                    <h2 class="text-[15px] font-semibold leading-5">(name.clone())</h2>
                }
                if !head.kind.is_empty() {
                    ui::pill(tone: "neutral", label: head.kind.clone())
                }
                if !head.level.is_empty() {
                    ui::pill(tone: level_tone(&head.level), label: head.level.clone())
                }
                if !path.is_empty() {
                    <span class="min-w-0 truncate text-[12.5px] text-fg-3" title=(path.clone())>(path)</span>
                }
            </div>
            <div class="flex min-h-[320px] flex-1 overflow-y-auto">
                <pre aria-hidden="true" class=(GUTTER)>(numbers)</pre>
                if editable {
                    <textarea name="text" rows=(rows) wrap="off" spellcheck="false" aria-label=(label) class=(TEXT)>(open.text)</textarea>
                } else {
                    <textarea name="text" rows=(rows) wrap="off" readonly="" aria-label=(label) class=(TEXT)>(open.text)</textarea>
                }
            </div>
            if rule {
                <div id="rule-test"></div>
            }
            <div class="flex flex-wrap items-center gap-2 border-t border-line px-[18px] py-3">
                if deletable {
                    <button type="button" popovertarget="delete-file" class=(ui::GHOST_DANGER_SM)>
                        ui::icon(name: "circle-x", small: true)
                        (thing)
                    </button>
                    ui::dialog(id: "delete-file", title: confirm, <div>
                        <div class=(ui::DIALOG_BODY)>
                            <p class=(ui::SECONDARY)>
                                "This removes " <span class=(ui::MONO)>(name.clone()) ".md"</span>
                                ". It can't be undone."
                            </p>
                        </div>
                        <div class=(ui::DIALOG_FOOT)>
                            <span class="spacer"></span>
                            <button type="button" popovertarget="delete-file" popovertargetaction="hide" class=(ui::BUTTON)>"Keep it"</button>
                            <button type="button" data-on:click=(delete) class=(ui::DANGER)>(thing)</button>
                        </div>
                    </div>)
                }
                <span class="flex-1"></span>
                if editable {
                    <span class="text-[12.5px] text-fg-3 max-[760px]:hidden">"Applies from the next run"</span>
                    <span class="ml-auto flex gap-2">
                        if rule {
                            <button type="button" data-on:click="@post('/rules/test', {contentType: 'form'})" class=(ui::GHOST_SM)>"Test"</button>
                        }
                        <button type="submit" class=(ui::PRIMARY_SM)>"Save"</button>
                    </span>
                } else {
                    <span class="text-[12.5px] text-fg-3">(open.elsewhere)</span>
                }
            </div>
        </form>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_previews_its_first_paragraph() {
        let text = "---\nkind: guide\n---\n\nKeep tickets\nsmall.\n\nMore.\n";
        assert_eq!(preview(text), "Keep tickets small.");
        assert_eq!(preview("plain"), "plain");
        assert_eq!(preview(""), "");
        assert!(preview(&"x".repeat(200)).chars().count() <= PREVIEW_CHARS + 1);
    }

    #[test]
    fn rule_links_encode_their_names() {
        assert_eq!(rule_href("tests", ""), "/rules?file=tests");
        assert_eq!(
            rule_href("a&b #1", "pack/x"),
            "/rules?file=a%26b+%231&from=pack%2Fx"
        );
    }

    #[test]
    fn a_rule_shows_its_kind_and_level() {
        let text = "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"src/**\"]\n---\nAsk.\n";
        assert_eq!(
            head(text),
            Head {
                kind: "protect".into(),
                level: "ask".into()
            }
        );
        assert_eq!(head("oops"), Head::default());
        assert_eq!(level_tone("block"), "bad");
    }
}
