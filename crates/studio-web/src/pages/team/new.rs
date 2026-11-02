//! A new project: the form that hires its lead, and the card in which the lead
//! says how the project runs once it has read it, with the offer of a first
//! milestone under it.

use studiod::api::{
    CommandRun, ProposedCommand, Request, SetupChoice, SetupProposal, SetupState, SetupView,
};
use topcoat::{
    context::{Cx, app_context},
    datastar::PatchElements,
    router::route,
    view::{Child, View, component, view},
};

use super::shell::{self, Here};
use super::talk::fill;
use super::{Project, Team, lead_href};
use crate::pages::office::model::Role;
use crate::pages::office::{Office, gather};
use crate::{AppConfig, client, ui};

/// Names for a new lead, in the order they are offered; the first that nobody
/// on the team has is suggested.
const NAMES: [&str; 12] = [
    "Mira", "Nora", "Sam", "Ines", "Theo", "Ada", "Leo", "Iris", "Remy", "Jun", "Zoe", "Otto",
];

/// The model a new lead runs on: a strong one, to plan the work, hand it out
/// and check it.
pub(super) const LEAD_MODEL: &str = "opus";

/// The name to suggest for a new lead: the first of `NAMES` that no hire of the
/// studio has, by id or by name.
fn suggested_name(office: &Office) -> String {
    let taken = |name: &str| {
        office.status.team.iter().any(|member| {
            member.id.eq_ignore_ascii_case(name) || member.name.trim().eq_ignore_ascii_case(name)
        })
    };
    NAMES.iter().find(|name| !taken(name)).map_or_else(
        || format!("Lead {}", office.status.team.len() + 1),
        |n| (*n).to_string(),
    )
}

/// What the owner chooses a project's lead from: a new hire with a name the
/// studio suggests, or a hire the studio has that can lead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hiring {
    /// The name suggested for a new lead.
    pub suggestion: String,
    /// Every hire with a lead's role, by (id, name). Any of them can lead one
    /// more project.
    pub leads: Vec<(String, String)>,
}

impl Hiring {
    pub fn of(office: &Office) -> Hiring {
        Hiring {
            suggestion: suggested_name(office),
            leads: office
                .status
                .team
                .iter()
                .filter(|member| Role::of(&member.role) == Role::Lead)
                .map(|member| (member.id.clone(), office.name_of(&member.id)))
                .collect(),
        }
    }
}

/// The whole page: the frame around the form.
#[component]
pub async fn new_project(team: Team, office: Office) -> topcoat::Result<impl View> {
    let Hiring { suggestion, leads } = Hiring::of(&office);
    let choice = !leads.is_empty();
    Ok(view! {
        shell::page(team: team, here: Here::New,
            <div class="mx-auto flex w-full max-w-[640px] flex-col gap-5">
                <div class="flex flex-col gap-1.5">
                    <h1 class=(ui::PAGE_TITLE)>"New project"</h1>
                    <p class="m-0 text-[14px] text-fg-3">"Pick its folder and hire a lead for it. The lead reads the project and tells you how it would run it."</p>
                </div>
                // A reply that says where the new project's conversation is
                // sends the browser there. What the owner types and picks here
                // is theirs: a patch of the page leaves the whole form as it is.
                <form id="new-project" data-ignore-morph="" class="flex flex-col gap-5" data-signals="{_who: 'new', _go: ''}" data-effect="$_go && (window.location.href = $_go)" data-on:submit="@post('/new', {contentType: 'form'})">
                    <section class="setup-box" aria-label="The folder">
                        <h2 class=(ui::SECTION_TITLE)>"The folder"</h2>
                        <label class=(ui::FIELD)>
                            <span class=(ui::FIELD_LABEL)>"Where is it?"</span>
                            <input name="path" required="" autocomplete="off" spellcheck="false" placeholder="~/games/garden" class=(ui::INPUT)>
                            <span class=(ui::HINT)>"A git repository on this computer."</span>
                        </label>
                    </section>
                    <section class="setup-box" aria-label="The lead">
                        <h2 class=(ui::SECTION_TITLE)>"The lead"</h2>
                        if choice {
                            <div class="flex flex-wrap gap-x-5 gap-y-2 text-[13.5px]" role="radiogroup" aria-label="Who leads it">
                                <label class="flex items-center gap-2"><input type="radio" name="who" value="new" checked="" data-bind:_who=""> "A new hire"</label>
                                <label class="flex items-center gap-2"><input type="radio" name="who" value="existing" data-bind:_who=""> "A lead you have"</label>
                            </div>
                        }
                        <div class="flex flex-col gap-3" data-show="$_who == 'new'">
                            <label class=(ui::FIELD)>
                                <span class=(ui::FIELD_LABEL)>"Its name"</span>
                                <input name="new_lead_name" required="" maxlength="40" autocomplete="off" spellcheck="false" value=(suggestion) class=(ui::INPUT) data-attr:disabled="$_who != 'new'">
                                <span class=(ui::HINT)>"Suggested. Change it if you like."</span>
                            </label>
                            <input type="hidden" name="new_lead_model" value=(LEAD_MODEL) data-attr:disabled="$_who != 'new'">
                            <p class="m-0 text-[13px] text-fg-3">"A new hire on a strong model (Opus), to plan the work, hand it out and check it."</p>
                        </div>
                        if choice {
                            <div class="flex flex-col gap-3" style="display: none" data-show="$_who == 'existing'">
                                <label class=(ui::FIELD)>
                                    <span class=(ui::FIELD_LABEL)>"Which lead"</span>
                                    <select name="lead" class=(ui::INPUT) data-attr:disabled="$_who != 'existing'">
                                        for (id, name) in leads {
                                            <option value=(id)>(name)</option>
                                        }
                                    </select>
                                </label>
                            </div>
                        }
                    </section>
                    <div class=(ui::ASK_ACTIONS)>
                        <button type="submit" class=(ui::PRIMARY)>"Hire and set up"</button>
                        <a href="/" class=(ui::GHOST)>"Cancel"</a>
                    </div>
                </form>
            </div>
        )
    })
}

/// A command as the card names it: its program without the folders in front,
/// then its arguments (`./scripts/test.sh` is `test.sh`).
fn short(command: &[String]) -> String {
    let Some((program, args)) = command.split_first() else {
        return String::new();
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the studio's try of `command` lets it be kept: it passed, or it
/// has not been tried.
fn works(command: &ProposedCommand) -> bool {
    !command.command.is_empty() && command.tried.as_ref().is_none_or(|run| run.passed)
}

/// What the owner keeps by taking the proposal as it stands: every check that
/// worked (a failed one would stop all work), the release and the paths to ask
/// about, and the daily limit. `plan_first` has the lead plan its first goal
/// at once.
fn chosen(proposal: &SetupProposal, plan_first: bool) -> SetupChoice {
    SetupChoice {
        verify: proposal
            .verify
            .iter()
            .filter(|check| works(check))
            .map(|check| check.command.clone())
            .collect(),
        capture: proposal
            .capture
            .iter()
            .filter(|capture| works(capture))
            .flat_map(|capture| capture.command.clone())
            .collect(),
        release: proposal
            .release
            .iter()
            .filter(|step| !step.is_empty())
            .cloned()
            .collect(),
        protected: proposal
            .protected
            .iter()
            .map(|path| path.glob.clone())
            .collect(),
        daily_usd: proposal.daily_usd,
        plan_first,
    }
}

/// A string as the project file writes it.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// A command as the project file writes it: `["./scripts/test.sh", "--fast"]`.
fn command_line(command: &[String]) -> String {
    let words: Vec<String> = command.iter().map(|word| quoted(word)).collect();
    format!("[{}]", words.join(", "))
}

/// The `.studio.toml` that `choice` would write.
fn file_text(choice: &SetupChoice) -> String {
    let list = |commands: &[Vec<String>]| {
        let lines: Vec<String> = commands.iter().map(|c| command_line(c)).collect();
        format!("[{}]", lines.join(", "))
    };
    let mut lines = Vec::new();
    if !choice.verify.is_empty() {
        lines.push(format!("verify = {}", list(&choice.verify)));
    }
    if !choice.capture.is_empty() {
        lines.push(format!("capture = {}", command_line(&choice.capture)));
    }
    if !choice.release.is_empty() {
        lines.push(format!("release = {}", list(&choice.release)));
    }
    if !choice.protected.is_empty() {
        let globs: Vec<String> = choice.protected.iter().map(|glob| quoted(glob)).collect();
        lines.push(format!("protected = [{}]", globs.join(", ")));
    }
    if lines.is_empty() {
        return "# Nothing to write: no checks, no release and no paths to ask about.".to_string();
    }
    lines.join("\n")
}

/// The choice as the Use these button posts it.
fn choice_json(choice: &SetupChoice) -> String {
    serde_json::to_string(choice).unwrap_or_default()
}

/// A tick for a row of the card: a circle with a mark, by how it went.
#[component]
async fn row(
    tone: &'static str,
    label: &'static str,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    let icon = match tone {
        "ok" => "check",
        "bad" => "x",
        "attn" => "circle-alert",
        "wait" => "clock",
        "money" => "wallet",
        _ => "circle-dashed",
    };
    Ok(view! {
        <div class="setup-row">
            <span class="setup-tick" data-tone=(tone)>ui::icon(name: icon)</span>
            <span class="text-[13px] text-fg-3">(label)</span>
            <div class="setup-value">(child)</div>
        </div>
    })
}

/// What came of the studio's try of a command: passed with what it came to,
/// failed, or still going (`trying`).
#[component]
async fn outcome(tried: Option<CommandRun>, trying: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        if let Some(run) = tried {
            if run.passed {
                <span class="text-ok-fg" title=(run.output_tail.clone())>
                    "✓ " (if run.summary.trim().is_empty() { "passes".to_string() } else { run.summary.clone() })
                </span>
            } else {
                <span class="text-bad-fg" title=(run.output_tail.clone())>
                    "✗ " (if run.summary.trim().is_empty() { "failed".to_string() } else { run.summary.clone() })
                </span>
            }
        } else if trying {
            <span class="text-fg-3">"trying…"</span>
        } else {
            <span class="text-fg-3">"not tried"</span>
        }
    })
}

/// Commands the lead proposed, each with its own result.
#[component]
async fn check_list(list: Vec<ProposedCommand>, trying: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        for (at, item) in list.into_iter().enumerate() {
            if at > 0 { <span class="text-fg-3">" · "</span> }
            <span class="whitespace-nowrap max-[760px]:whitespace-normal">
                <code class=(ui::MONO)>(short(&item.command))</code>
                " "
                outcome(tried: item.tried, trying: trying)
            </span>
        }
    })
}

/// Where a row's tone stands for `list`: bad if one failed, waiting while one
/// is untried, else good.
fn tone_of(list: &[ProposedCommand]) -> &'static str {
    if list
        .iter()
        .any(|check| check.tried.as_ref().is_some_and(|run| !run.passed))
    {
        "bad"
    } else if list.iter().any(|check| check.tried.is_none()) {
        "wait"
    } else {
        "ok"
    }
}

/// The checks named in one line, for what was set up.
fn names(choice: &SetupChoice) -> String {
    if choice.verify.is_empty() {
        return "none".to_string();
    }
    choice
        .verify
        .iter()
        .map(|check| short(check))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A sentence's words read after a colon: the first word loses its capital,
/// unless it is a name or an acronym ("CI", "iOS", "README") rather than an
/// ordinary word that only began the sentence.
fn mid_sentence(text: &str) -> String {
    let first = text.split_whitespace().next().unwrap_or_default();
    let mut letters = first.chars();
    let ordinary = first != "I"
        && !first.starts_with("I'")
        && letters.next().is_some_and(char::is_uppercase)
        && letters.all(|c| c.is_lowercase() || c == '\'' || c == '-');
    if !ordinary {
        return text.to_string();
    }
    let mut chars = text.chars();
    chars
        .next()
        .into_iter()
        .flat_map(char::to_lowercase)
        .chain(chars)
        .collect()
}

/// "How {project} runs": what the lead proposed after reading the project,
/// with what the studio found when it tried it, and the buttons to use it. The
/// conversation draws it where the lead says it. When the lead has a first
/// milestone to offer, the offer is under it.
#[component]
pub async fn setup_card(view: SetupView, lead_name: String) -> topcoat::Result<impl View> {
    let SetupView {
        project,
        lead,
        state,
        proposal,
        error,
    } = view;
    // The lead is called by its name, as everywhere else, not by its hire id.
    let name = if lead_name.trim().is_empty() {
        "The lead".to_string()
    } else {
        lead_name
    };
    let reading = state == SetupState::Reading;
    let checking = state == SetupState::Checking;
    let ready = state == SetupState::Ready;
    let accepted = state == SetupState::Accepted;
    let failed = state == SetupState::Failed;
    let title = match state {
        SetupState::Reading => format!("{name} is reading {project}…"),
        SetupState::Checking => "Trying the checks…".to_string(),
        SetupState::Failed => format!("Could not set up {project}"),
        _ => format!("How {project} runs"),
    };
    let href = lead_href(&project);
    let accept = format!("@post('{href}/setup/accept', {{contentType: 'form'}}); $_noplan = true");
    let accept_here = format!("@post('{href}/setup/accept', {{contentType: 'form'}})");
    let retry = format!("@post('{href}/setup/retry')");
    let proposal = proposal.filter(|_| !reading && !failed);
    let rows = proposal.clone().filter(|_| checking || ready);
    let choice = proposal.as_ref().map(|p| chosen(p, false));
    let use_these = choice.as_ref().map(choice_json).unwrap_or_default();
    let plan_it = proposal
        .as_ref()
        .map(|p| choice_json(&chosen(p, true)))
        .unwrap_or_default();
    let file = choice.as_ref().map(file_text).unwrap_or_default();
    let done_line = format!(
        "Set up. Checks: {}",
        choice.as_ref().map_or_else(|| "none".to_string(), names)
    );
    let offer = proposal
        .as_ref()
        .and_then(|p| p.first_goal.clone())
        .filter(|_| ready || accepted);
    let first = offer.as_ref().map(|goal| {
        let version = goal
            .version
            .as_deref()
            .map(|v| format!(" as v{}", v.trim_start_matches(['v', 'V'])))
            .unwrap_or_default();
        let text = goal.text.trim().trim_end_matches('.').to_string();
        let said = mid_sentence(&text);
        (text, said, version)
    });
    let state_key = match state {
        SetupState::Reading => "reading",
        SetupState::Checking => "checking",
        SetupState::Ready => "ready",
        SetupState::Accepted => "accepted",
        SetupState::Failed => "failed",
    };
    Ok(view! {
        <div class="setup flex flex-col gap-3" data-state=(state_key) data-signals__ifmissing="{_file: false, _noplan: false}">
            <section class="setup-card" aria-label=(format!("How {project} runs"))>
                <header class="setup-head">
                    <span class="setup-gear">ui::icon(name: "settings-2")</span>
                    <h3 class=(ui::SECTION_TITLE)>(title)</h3>
                    if reading || checking { <span class="pulse"></span> }
                </header>
                if reading {
                    <p class="setup-note">"Reading the code, the tests and how it is built and released."</p>
                }
                if failed {
                    <p class="setup-note text-bad-fg">(error.clone().unwrap_or_else(|| "The setup could not go on.".to_string()))</p>
                    <div class="setup-actions">
                        <button type="button" class=(ui::BUTTON_SM) data-on:click=(retry)>"Try again"</button>
                    </div>
                }
                if let Some(p) = rows {
                    if !p.summary.trim().is_empty() {
                        <p class="setup-note">(p.summary.clone())</p>
                    }
                    <div class="setup-rows">
                        row(tone: tone_of(&p.verify), label: "Checks",
                            if p.verify.is_empty() {
                                <span class="text-fg-3">"None found"</span>
                            } else {
                                check_list(list: p.verify.clone(), trying: checking)
                                if p.verify.iter().any(|check| !works(check)) {
                                    <span class="block text-[12.5px] text-fg-3">"A check that failed is left out."</span>
                                }
                            }
                        )
                        row(tone: p.capture.as_ref().map_or("none", |c| tone_of(std::slice::from_ref(c))), label: "Screenshots",
                            if let Some(capture) = p.capture.clone() {
                                check_list(list: vec![capture], trying: checking)
                            } else {
                                <span class="text-fg-3">"None"</span>
                            }
                        )
                        row(tone: if p.release.is_empty() { "none" } else { "ok" }, label: "Release",
                            if p.release.is_empty() {
                                <span class="text-fg-3">"No release found"</span>
                            } else {
                                for (at, step) in p.release.iter().enumerate() {
                                    if at > 0 { ", then " }
                                    <code class=(ui::MONO)>(short(step))</code>
                                }
                                if !p.release_source.trim().is_empty() {
                                    <span class="text-fg-3">" from " (p.release_source.clone())</span>
                                }
                            }
                        )
                        row(tone: if p.protected.is_empty() { "none" } else { "attn" }, label: "Ask you first",
                            if p.protected.is_empty() {
                                <span class="text-fg-3">"Nothing"</span>
                            } else {
                                for (at, path) in p.protected.iter().enumerate() {
                                    if at > 0 { <span class="text-fg-3">" · "</span> }
                                    <code class=(ui::MONO) title=(path.why.clone())>(path.glob.clone())</code>
                                }
                            }
                        )
                        row(tone: "money", label: "Budget",
                            if let Some(usd) = p.daily_usd {
                                (ui::limit(usd)) " a day"
                            } else {
                                <span class="text-fg-3">"As the studio's limit"</span>
                            }
                        )
                    </div>
                    <pre class="setup-file" style="display: none" data-preserve-attr="style" data-show="$_file" aria-label="The file it would write"><span class="setup-file-head">"Goes into " (project.clone()) "/.studio.toml"</span> (file.clone())</pre>
                    if ready {
                        <div class="setup-actions">
                            <form class="contents" data-on:submit=(accept_here)>
                                <input type="hidden" name="choice" value=(use_these.clone())>
                                <button type="submit" class=(ui::PRIMARY_SM)>"Use these"</button>
                            </form>
                            <button type="button" class=(ui::BUTTON_SM) data-on:click=(fill("About the setup: "))>"Change something"</button>
                            <button type="button" class=(ui::GHOST_SM) data-on:click="$_file = !$_file">"See the file"</button>
                        </div>
                    }
                }
                if accepted {
                    <p class="setup-note">(done_line)</p>
                }
            </section>
            if let Some((text, said, version)) = first {
                <div class="setup-offer" data-preserve-attr="style" data-show="!$_noplan">
                    <p class="m-0 text-[14px] leading-6">"Then I'd start with this: " (said) ". Shall I plan it" (version) "?"</p>
                    if let Some(goal) = offer.clone() {
                        if !goal.why.trim().is_empty() {
                            <p class="m-0 text-[13px] text-fg-3">(goal.why.clone())</p>
                        }
                    }
                    <div class="setup-actions">
                        if accepted {
                            <form class="contents" data-on:submit="@post('/goals', {contentType: 'form'}); $_noplan = true">
                                <input type="hidden" name="project" value=(project.clone())>
                                <input type="hidden" name="hire" value=(lead.clone().unwrap_or_default())>
                                <input type="hidden" name="text" value=(text.clone())>
                                <input type="hidden" name="version" value=(offer.as_ref().and_then(|g| g.version.clone()).unwrap_or_default())>
                                <input type="hidden" name="detail" value=(offer.as_ref().map(|g| g.why.clone()).unwrap_or_default())>
                                <button type="submit" class=(ui::BUTTON_SM)>"Plan it"</button>
                            </form>
                        } else {
                            <form class="contents" data-on:submit=(accept)>
                                <input type="hidden" name="choice" value=(plan_it.clone())>
                                <button type="submit" class=(ui::BUTTON_SM)>"Plan it"</button>
                            </form>
                        }
                        <button type="button" class=(ui::GHOST_SM) data-on:click="$_noplan = true">"Not yet"</button>
                    </div>
                </div>
            }
        </div>
    })
}

/// Has the lead of a project whose setup failed try again, in the same folder.
#[route(POST "/leads/{project}/setup/retry")]
async fn retry_setup(cx: &Cx) -> topcoat::Result<PatchElements> {
    have_lead_set_up(cx, |_, project| format!("Trying {project} again.")).await
}

/// Has the lead of a project that nobody has set up read it and say how it
/// runs. Nothing in the project's files changes until the owner uses what the
/// lead proposes.
#[route(POST "/leads/{project}/setup/start")]
async fn start_setup(cx: &Cx) -> topcoat::Result<PatchElements> {
    have_lead_set_up(cx, |lead, project| {
        format!("{lead} is looking at how {project} runs.")
    })
    .await
}

/// Has the lead of the project in the address set it up, in the project's own
/// folder, and tells the owner what `said` makes of the lead's name and the
/// project's.
async fn have_lead_set_up(
    cx: &Cx,
    said: impl FnOnce(&str, &str) -> String,
) -> topcoat::Result<PatchElements> {
    let config = app_context::<AppConfig>(cx);
    let project = path_param_project(cx);
    let office = match gather(config).await {
        Ok(office) => office,
        Err(err) => return ui::notice(cx, err.to_string(), true).await,
    };
    let Some(found) = office.project(&project) else {
        return ui::notice(cx, format!("The studio has no project {project}."), true).await;
    };
    let Some(lead) = found.lead.clone() else {
        return ui::notice(cx, format!("{project} has no lead to set it up."), true).await;
    };
    let name = office.name_of(&lead);
    let request = Request::SetupProject {
        path: found.path.clone(),
        name: Some(project.clone()),
        lead: Some(lead),
        new_lead: None,
    };
    match client::call::<SetupView>(&config.socket, request).await {
        Ok(_) => ui::notice(cx, said(&name, &project), false).await,
        Err(err) => ui::notice(cx, err.to_string(), true).await,
    }
}

/// The project in the address.
fn path_param_project(cx: &Cx) -> String {
    topcoat::router::path_param::<Project>(cx).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Tag, named, text_of};

    fn button<'a>(html: &'a str, label: &str) -> Option<Tag<'a>> {
        named(html, "button")
            .into_iter()
            .find(|button| button.text().contains(label))
    }
    use serde_json::json;
    use studiod::api::{FirstGoal, ProtectedPath};

    fn command(program: &str, tried: Option<(bool, &str)>) -> ProposedCommand {
        ProposedCommand {
            command: vec![format!("./scripts/{program}")],
            why: String::new(),
            tried: tried.map(|(passed, summary)| CommandRun {
                passed,
                seconds: 3,
                summary: summary.into(),
                output_tail: String::new(),
            }),
        }
    }

    fn proposal() -> SetupProposal {
        SetupProposal {
            summary: "A small 2D game.".into(),
            verify: vec![
                command("test.sh", Some((true, "112 pass"))),
                command("lint.sh", Some((false, "failed"))),
            ],
            capture: Some(command("capture.sh", Some((true, "4 pictures")))),
            release: vec![
                vec!["./scripts/package.sh".into()],
                vec!["./scripts/upload.sh".into()],
            ],
            release_source: "your CI".into(),
            protected: vec![
                ProtectedPath {
                    glob: "saves/**".into(),
                    why: "Players' saves.".into(),
                },
                ProtectedPath {
                    glob: ".env*".into(),
                    why: "Secrets.".into(),
                },
            ],
            daily_usd: Some(10.0),
            first_goal: Some(FirstGoal {
                text: "Fix the three crashes in TODO.md.".into(),
                version: Some("0.9".into()),
                why: "They block every level.".into(),
            }),
        }
    }

    /// The setup of the garden, whose lead is the hire `m-17`, called Mira.
    fn view(state: SetupState, proposal: Option<SetupProposal>) -> SetupView {
        SetupView {
            project: "garden".into(),
            lead: Some("m-17".into()),
            state,
            proposal,
            error: None,
        }
    }

    async fn card(view: SetupView) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => setup_card(view: view, lead_name: "Mira".to_string()) },
        )
        .await
        .unwrap()
    }

    #[test]
    fn a_name_nobody_has_is_suggested() {
        let mut office = Office::default();
        assert_eq!(suggested_name(&office), "Mira");
        office.status = serde_json::from_value(json!({
            "team": [
                {"id": "mira", "role": "lead", "model": "opus"},
                {"id": "x1", "name": "Nora", "role": "lead", "model": "opus"},
            ],
        }))
        .unwrap();
        assert_eq!(suggested_name(&office), "Sam");
    }

    #[test]
    fn a_project_can_be_given_a_new_hire_or_any_hire_with_a_leads_role() {
        let none = Hiring::of(&Office::default());
        assert_eq!(none.suggestion, "Mira");
        assert!(none.leads.is_empty());

        let office = Office {
            status: serde_json::from_value(json!({
                "team": [
                    {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                    {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
                    // A hire file that gives no name is called by its id.
                    {"id": "theo", "role": "lead", "model": "opus"},
                ],
            }))
            .unwrap(),
            ..Office::default()
        };
        let hiring = Hiring::of(&office);
        // The leads in the order of the team, whichever project they lead.
        assert_eq!(
            hiring.leads,
            [
                ("nora".to_string(), "Nora".to_string()),
                ("theo".to_string(), "theo".to_string())
            ]
        );
        // And a name for a new one: the first nobody on the team has.
        assert_eq!(hiring.suggestion, "Mira");
    }

    #[test]
    fn what_is_kept_leaves_out_a_check_that_failed() {
        let kept = chosen(&proposal(), false);
        assert_eq!(kept.verify, [vec!["./scripts/test.sh".to_string()]]);
        assert_eq!(kept.capture, ["./scripts/capture.sh"]);
        assert_eq!(kept.release.len(), 2);
        assert_eq!(kept.protected, ["saves/**", ".env*"]);
        assert_eq!(kept.daily_usd, Some(10.0));
        assert!(!kept.plan_first);
        assert!(chosen(&proposal(), true).plan_first);
        // What was never tried is kept, as the studio could not say it fails.
        let mut untried = proposal();
        untried.verify = vec![command("test.sh", None)];
        assert_eq!(chosen(&untried, false).verify.len(), 1);
    }

    #[test]
    fn the_file_is_worked_out_from_what_is_kept() {
        let text = file_text(&chosen(&proposal(), false));
        assert_eq!(
            text,
            "verify = [[\"./scripts/test.sh\"]]\n\
             capture = [\"./scripts/capture.sh\"]\n\
             release = [[\"./scripts/package.sh\"], [\"./scripts/upload.sh\"]]\n\
             protected = [\"saves/**\", \".env*\"]"
        );
        assert!(file_text(&SetupChoice::default()).starts_with("# Nothing"));
    }

    #[tokio::test]
    async fn reading_says_who_reads_what() {
        let html = card(view(SetupState::Reading, None)).await;
        let text = text_of(&html);
        assert!(text.contains("Mira is reading garden…"), "{text}");
        // The lead is called by its name, never by the id of its hire.
        assert!(!text.contains("m-17") && !html.contains("M-17"), "{html}");
        assert!(named(&html, "button").is_empty());
    }

    #[tokio::test]
    async fn a_card_without_a_name_for_its_lead_says_the_lead() {
        let cx = Cx::default();
        let html = ui::html(
            &cx,
            view! { cx => setup_card(view: view(SetupState::Reading, None), lead_name: String::new()) },
        )
        .await
        .unwrap();
        assert!(text_of(&html).contains("The lead is reading garden…"));
    }

    #[tokio::test]
    async fn checking_shows_each_command_with_its_try() {
        let mut p = proposal();
        p.verify = vec![
            command("test.sh", Some((true, "112 pass"))),
            command("lint.sh", None),
        ];
        let html = card(view(SetupState::Checking, Some(p))).await;
        let text = text_of(&html);
        assert!(text.contains("Trying the checks…"), "{text}");
        assert!(text.contains("test.sh ✓ 112 pass"), "{text}");
        assert!(text.contains("lint.sh trying…"), "{text}");
        // Nothing to choose until everything is tried.
        assert!(button(&html, "Use these").is_none());
    }

    #[tokio::test]
    async fn ready_gives_the_rows_and_the_buttons() {
        let html = card(view(SetupState::Ready, Some(proposal()))).await;
        let text = text_of(&html);
        assert!(text.contains("How garden runs"), "{text}");
        assert!(text.contains("test.sh ✓ 112 pass"), "{text}");
        assert!(text.contains("lint.sh ✗ failed"), "{text}");
        assert!(text.contains("capture.sh ✓ 4 pictures"), "{text}");
        assert!(
            text.contains("package.sh , then upload.sh from your CI"),
            "{text}"
        );
        assert!(text.contains("saves/**"), "{text}");
        assert!(text.contains("$10 a day"), "{text}");
        assert!(text.contains("Goes into garden/.studio.toml"), "{text}");
        for label in ["Use these", "Change something", "See the file"] {
            assert!(button(&html, label).is_some(), "{label}");
        }
    }

    #[tokio::test]
    async fn the_milestone_is_offered_and_plan_it_depends_on_the_setup() {
        let html = card(view(SetupState::Ready, Some(proposal()))).await;
        let text = text_of(&html);
        assert!(
            text.contains(
                "Then I'd start with this: fix the three crashes in TODO.md. Shall I plan it as v0.9?"
            ),
            "{text}"
        );
        assert!(button(&html, "Not yet").is_some());
        // Before the setup is accepted, Plan it accepts it with the plan.
        assert!(button(&html, "Plan it").is_some());
        assert!(html.contains("/leads/garden/setup/accept"));
        assert!(html.contains("plan_first&quot;:true") || html.contains(r#"plan_first":true"#));
        // After it, Plan it starts the goal.
        let html = card(view(SetupState::Accepted, Some(proposal()))).await;
        assert!(html.contains("'/goals'"), "{html}");
        assert!(!html.contains("/setup/accept"), "{html}");
    }

    #[tokio::test]
    async fn accepted_says_what_was_set_up_and_failed_says_why() {
        let html = card(view(SetupState::Accepted, Some(proposal()))).await;
        let text = text_of(&html);
        assert!(text.contains("Set up. Checks: test.sh"), "{text}");
        assert!(!text.contains("lint.sh"), "{text}");
        assert!(button(&html, "Use these").is_none());

        let mut failed = view(SetupState::Failed, None);
        failed.error = Some("Could not read the repository.".into());
        let html = card(failed).await;
        assert!(text_of(&html).contains("Could not read the repository."));
        assert!(button(&html, "Try again").is_some());
    }

    #[test]
    fn the_offer_reads_after_its_colon_whatever_the_goals_first_word() {
        assert_eq!(
            mid_sentence("Take the unexplained 'sleep 8' out of test.sh"),
            "take the unexplained 'sleep 8' out of test.sh"
        );
        assert_eq!(mid_sentence("take it out"), "take it out");
        assert_eq!(mid_sentence("A new menu"), "a new menu");
        // Names and acronyms keep their capitals.
        assert_eq!(mid_sentence("CI fails on main"), "CI fails on main");
        assert_eq!(mid_sentence("README is stale"), "README is stale");
        assert_eq!(mid_sentence("iOS builds break"), "iOS builds break");
        assert_eq!(mid_sentence("I'd drop it"), "I'd drop it");
        assert_eq!(mid_sentence(""), "");
    }
}
