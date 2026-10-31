//! Edit the studio's hires and rules. studiod validates and writes; this
//! page only shows the files and sends the text back.

use futures_util::Stream;
use futures_util::stream;
use serde::Deserialize;
use studiod::api::{FileKind, FileView, Request, RuleTest};
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
    title: &'static str,
    template: &'static str,
}

const HIRES: Kind = Kind {
    files: FileKind::Hires,
    base: "hires",
    title: "Hires",
    template: HIRE_TEMPLATE,
};

const RULES: Kind = Kind {
    files: FileKind::Rules,
    base: "rules",
    title: "Rules",
    template: RULE_TEMPLATE,
};

#[query_params(error = bad_request)]
struct Selected {
    file: Option<String>,
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

#[page("/hires")]
async fn hires_page(cx: &Cx) -> topcoat::Result<impl View> {
    show(cx, &HIRES).await
}

#[page("/rules")]
async fn rules_page(cx: &Cx) -> topcoat::Result<impl View> {
    show(cx, &RULES).await
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

async fn show(cx: &Cx, kind: &Kind) -> topcoat::Result<impl View> {
    let all = fetch(cx, kind).await?;
    let query = query_params::<Selected>(cx)?;
    let adding = query.new.is_some();
    let current = query.file.clone().unwrap_or_default();
    let open = all.iter().find(|f| f.name == current).cloned();
    let (name, text) = match open {
        Some(f) => (f.name, f.text),
        None if adding => (
            query.name.clone().unwrap_or_default(),
            query
                .text
                .clone()
                .unwrap_or_else(|| kind.template.to_string()),
        ),
        None => (String::new(), String::new()),
    };
    let shown = adding || !name.is_empty();
    // A new file's prefilled name isn't an open file.
    let current = if adding { String::new() } else { name.clone() };
    // With no file open, hires show as cards instead of a bare list.
    let overview = kind.base == "hires" && !shown;
    let cards = if overview {
        let config = app_context::<AppConfig>(cx);
        hires::cards_for(&config.socket, &config.data_dir, &all).await
    } else {
        Vec::new()
    };
    let (base, title) = (kind.base, kind.title);
    Ok(view! { cx =>
        <div class="mb-3 flex items-center justify-between">
            <h1 class=(ui::PAGE_TITLE)>(title)</h1>
            <a href=(format!("/{base}?new")) class=(ui::BUTTON)>"New"</a>
        </div>
        if overview {
            hires::cards(list: cards)
        } else {
            <div class="grid grid-cols-1 gap-3 md:grid-cols-[16rem_minmax(0,1fr)]">
                file_list(base: base, files: all, current: current)
                if shown {
                    editor(base: base, name: name, text: text, adding: adding)
                } else {
                    empty_editor()
                }
            </div>
        }
    })
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
            events.push(list_patch(cx, kind, &form.name).await?.into());
            if form.adding.is_some() {
                // The new file is now an ordinary one: name fixed, deletable.
                let (base, name) = (kind.base, form.name);
                let saved = ui::html(
                    cx,
                    view! { cx => editor(base: base, name: name, text: text, adding: false) },
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
            if open == target {
                events.push(list_patch(cx, kind, "").await?.into());
                let cleared = ui::html(cx, view! { cx => empty_editor() }).await?;
                events.push(PatchElements::new(cleared).into());
            } else {
                events.push(list_patch(cx, kind, &open).await?.into());
            }
        }
        Err(err) => events.push(ui::notice(cx, err.to_string(), true).await?.into()),
    }
    Ok(Sse::new(stream::iter(events.into_iter().map(Ok))))
}

async fn list_patch(cx: &Cx, kind: &Kind, current: &str) -> topcoat::Result<PatchElements> {
    let all = fetch(cx, kind).await?;
    let (base, current) = (kind.base, current.to_owned());
    let html = ui::html(
        cx,
        view! { cx => file_list(base: base, files: all, current: current) },
    )
    .await?;
    Ok(PatchElements::new(html))
}

/// Names the daemon accepts, so one is safe to put in a URL.
fn safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The first line of a rule's text after its frontmatter, cut short.
fn preview(text: &str) -> String {
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or(text);
    let line = body.lines().map(str::trim).find(|l| !l.is_empty());
    crate::summary::truncate(line.unwrap_or_default(), PREVIEW_CHARS)
}

#[component]
async fn file_list(
    base: &'static str,
    files: Vec<FileView>,
    current: String,
) -> topcoat::Result<impl View> {
    // `open` goes into a script, so only a name that is safe there.
    let open = if safe(&current) {
        current.clone()
    } else {
        String::new()
    };
    Ok(view! {
        <nav id="file-list" class="min-w-0 space-y-1 self-start">
            if files.is_empty() {
                <p class=(format!("px-2 py-1 {}", ui::MUTED))>"Nothing here yet."</p>
            }
            #[key(file.name.clone())]
            for file in files {
                let rule = base == "rules";
                let deletable = rule && safe(&file.name);
                let note = preview(&file.text);
                let href = format!("/{base}?file={}", file.name);
                let remove = format!("confirm('Delete rule {0}?') && @post('/rules/{0}/delete?open={open}')", file.name);
                <div class=(if file.name == current {
                    "rounded-md bg-zinc-100 px-2 py-1 dark:bg-zinc-800"
                } else {
                    "rounded-md px-2 py-1 hover:bg-zinc-100 dark:hover:bg-zinc-900"
                })>
                    <a href=(href.clone()) class="block min-w-0">
                        <span class="break-all font-mono">(file.name.clone())</span>
                        if let Some(problem) = file.problem {
                            <span class="ml-1.5 rounded bg-rose-100 px-1 text-[11px] font-semibold text-rose-800 dark:bg-rose-950 dark:text-rose-300">"broken"</span>
                            <span class="mt-0.5 block break-words text-[12px] text-rose-700 dark:text-rose-400">(problem)</span>
                        } else if rule && !note.is_empty() {
                            <span class=(format!("mt-0.5 block truncate text-[12px] {}", ui::SECONDARY))>(note)</span>
                        }
                    </a>
                    if rule {
                        <div class="mt-0.5 flex gap-1">
                            <a href=(href) class=(format!("{} !px-2 !py-0.5 !text-[12px]", ui::QUIET))>"Edit"</a>
                            if deletable {
                                <button type="button" data-on:click=(remove) class=(format!("{} !px-2 !py-0.5 !text-[12px]", ui::DANGER))>"Delete"</button>
                            }
                        </div>
                    }
                </div>
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
        <div id="rule-test" class="space-y-1">
            if error.is_empty() {
                <p class="text-zinc-600 dark:text-zinc-400">(summary)</p>
                <ul class="space-y-0.5">
                    for hit in test.hits.into_iter().take(HITS_SHOWN) {
                        <li class="flex min-w-0 items-baseline gap-2 text-[12px]">
                            <a href=(format!("/tickets/{}", hit.ticket)) class=(format!("shrink-0 font-mono hover:text-zinc-900 dark:hover:text-zinc-100 {}", ui::MUTED))>(hit.ticket.clone())</a>
                            <span class="shrink-0 rounded bg-zinc-100 px-1.5 py-px text-[11px] dark:bg-zinc-800">(hit.level)</span>
                            <span class=(format!("shrink-0 {}", ui::MUTED))>(hit.tool)</span>
                            <code title=(hit.what.clone()) class="min-w-0 flex-1 truncate font-mono">(hit.what)</code>
                        </li>
                    }
                </ul>
            } else {
                <p class="whitespace-pre-wrap text-rose-700 dark:text-rose-400">(error)</p>
            }
        </div>
    })
}

#[component]
async fn empty_editor() -> topcoat::Result<impl View> {
    Ok(view! {
        <div id="editor" class=(format!("px-1 py-2 {}", ui::MUTED))>"Pick a file, or add a new one."</div>
    })
}

#[component]
async fn editor(
    base: &'static str,
    name: String,
    text: String,
    adding: bool,
) -> topcoat::Result<impl View> {
    let submit = format!("@post('/{base}/save', {{contentType: 'form'}})");
    let delete =
        format!("confirm('Delete this file?') && @post('/{base}/{name}/delete?open={name}')");
    let deletable = !adding && safe(&name);
    let test = (base == "rules").then(|| "@post('/rules/test', {contentType: 'form'})".to_string());
    Ok(view! {
        <form id="editor" data-on:submit=(submit) class="space-y-2">
            if adding {
                <input type="hidden" name="adding" value="1">
                <input
                    name="name"
                    required=""
                    placeholder="file-name"
                    pattern="[a-z0-9][a-z0-9_\\-]{0,63}"
                    title="Lowercase letters, digits, - and _"
                    value=(name)
                    class=(format!("{} font-mono", ui::INPUT))
                >
            } else {
                <input name="name" value=(name) readonly="" class=(format!("{} font-mono", ui::INPUT))>
            }
            <textarea
                name="text"
                rows="22"
                spellcheck="false"
                class=(format!("{} font-mono text-[12px] leading-relaxed", ui::INPUT))
            >(text)</textarea>
            <div class="flex flex-wrap gap-2">
                <button type="submit" class=(ui::PRIMARY)>"Save"</button>
                if let Some(test) = test {
                    <button type="button" data-on:click=(test) class=(ui::BUTTON)>"Test"</button>
                }
                if deletable {
                    <button type="button" data-on:click=(delete) class=(ui::DANGER)>"Delete"</button>
                }
            </div>
            if base == "rules" {
                <div id="rule-test"></div>
            }
        </form>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_previews_its_first_line_of_text() {
        let text = "---\nkind: guide\n---\n\nKeep tickets small.\nSecond line.\n";
        assert_eq!(preview(text), "Keep tickets small.");
        assert_eq!(preview("plain"), "plain");
        assert_eq!(preview(""), "");
        assert!(preview(&"x".repeat(200)).chars().count() <= PREVIEW_CHARS + 1);
    }
}
