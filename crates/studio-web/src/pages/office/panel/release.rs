//! Releases in the panel: the card that asks for the owner's go-ahead, the
//! checklist of a release under way, and the form that starts one.

use studiod::api::{ReleaseState, ReleaseStep, ReleaseView};
use topcoat::view::{View, ViewExt, component, view};

use super::standing::{self, Standing};
use super::{CARD, CARD_ATTENTION, project_tag, quiet, section};
use crate::pages::office::inspector;
use crate::pages::office::model::Phase;
use crate::pages::office::select;
use crate::pages::timeline::relative;
use crate::ui;

/// The version after `last`: its last number up by one, for "1.0" and
/// "1.2.3" (a leading "v" is left off); empty for anything else.
pub(super) fn next_version(last: &str) -> String {
    let parts: Vec<&str> = last
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .collect();
    let numbers: Option<Vec<u64>> = parts.iter().map(|p| p.parse().ok()).collect();
    match numbers {
        Some(mut numbers) if (2..=3).contains(&numbers.len()) => {
            if let Some(end) = numbers.last_mut() {
                *end = end.saturating_add(1);
            }
            numbers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(".")
        }
        _ => String::new(),
    }
}

/// Whether `step` is one of the fixed steps of a release, rather than one of
/// the project's release commands.
fn fixed(step: &ReleaseStep) -> bool {
    matches!(step.name.as_str(), "Final checks" | "Your go-ahead") || step.name.starts_with("Tag ")
}

/// What a step is called: the fixed ones by their name, a release command
/// "Release step 1", "Release step 2". A command comes with its text, to show
/// small under the label.
fn step_label(steps: &[ReleaseStep], at: usize) -> (String, Option<String>) {
    let Some(step) = steps.get(at) else {
        return (String::new(), None);
    };
    if fixed(step) {
        return (step.name.clone(), None);
    }
    let n = steps[..at].iter().filter(|s| !fixed(s)).count() + 1;
    (format!("Release step {n}"), Some(step.name.clone()))
}

/// The release's steps in plain order: where it is, what comes next and what
/// it ends in. "Final checks passed. Next: your go-ahead, then it publishes
/// v1.2."
pub(super) fn order(release: &ReleaseView) -> String {
    let v = release.version.trim_start_matches('v');
    match release.state {
        ReleaseState::Checking => {
            format!("The final checks are running. Next: your go-ahead, then it publishes v{v}.")
        }
        ReleaseState::Waiting => {
            format!("Final checks passed. Next: your go-ahead, then it publishes v{v}.")
        }
        ReleaseState::Publishing => format!("You gave the go-ahead. It is publishing v{v} now."),
        ReleaseState::Failed => format!("The release of v{v} stopped. Nothing was published."),
        ReleaseState::Released => format!("v{v} is published."),
        ReleaseState::Cancelled => format!("The release of v{v} was called off."),
    }
}

/// A button that sends one release action for `project`: `approve`, `cancel`
/// or `start` (with the version typed beside it). `kind` is `primary`,
/// `danger` or the plain one.
#[component]
pub(super) async fn release_button(
    action: &'static str,
    project: String,
    label: &'static str,
    #[default] kind: &'static str,
) -> topcoat::Result<impl View> {
    let class = match kind {
        "primary" => ui::PRIMARY_SM,
        "danger" => ui::GHOST_DANGER_SM,
        _ => ui::BUTTON_SM,
    };
    let post = format!("@post('/office/releases/{action}', {{contentType: 'form'}})");
    Ok(view! {
        <form class="contents" data-on:submit=(post)>
            <input type="hidden" name="project" value=(project)>
            <button type="submit" class=(class)>(label)</button>
        </form>
    })
}

/// A release's steps as a row of short bars: the ones done in the project's
/// colour, the one that waits for the owner in amber.
#[component]
async fn step_bars(release: ReleaseView) -> topcoat::Result<impl View> {
    let waiting = release.state == ReleaseState::Waiting;
    let first_open = release.steps.iter().position(|s| !s.done);
    let bars: Vec<&str> = release
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            if step.done {
                "bg-[var(--pj)]"
            } else if waiting && Some(i) == first_open {
                "bg-attn"
            } else {
                "bg-bg-4"
            }
        })
        .collect();
    let words = standing::steps_of(&release);
    Ok(view! {
        <div class="flex items-center gap-2.5">
            <span class="flex items-center gap-1" role="img" aria-label=(words.clone())>
                for bar in bars { <i class=(format!("h-1.5 w-6 rounded-[3px] {bar}"))></i> }
            </span>
            <span class="text-[12.5px] text-fg-2">(words)</span>
        </div>
    })
}

/// A release that waits for the owner's go-ahead, in For you: what it
/// publishes, how far it is, and Approve or Not yet.
#[component]
pub(super) async fn release_card(project: Standing, now: u64) -> topcoat::Result<impl View> {
    let Some(release) = project.waiting().cloned() else {
        return Ok(view! {}.boxed());
    };
    let version = release.version.trim_start_matches('v').to_string();
    let order = order(&release);
    let when = relative(now, standing::moved_at(&release));
    let pick = select(&project.key());
    let see = format!("See {}", project.name);
    let tint = format!("pj-{}", project.tint);
    Ok(view! {
        <div class=(format!("{tint} {CARD} {CARD_ATTENTION}"))>
            <div class="flex items-center gap-2">
                project_tag(name: project.name.clone(), tint: project.tint)
                <time class="ml-auto whitespace-nowrap text-[12px] text-fg-3">(when)</time>
            </div>
            <h3 class="m-0 text-[15px] font-semibold leading-[1.3] [text-wrap:balance]">"Publish " (project.name.clone()) " v" (version.clone()) "?"</h3>
            <p class="m-0 text-[13px] leading-5 text-fg-2">(order)</p>
            step_bars(release: release)
            if let Some(lead) = project.lead.clone() {
                <div class="flex items-center gap-2 text-[12.5px] text-fg-3">
                    ui::avatar(hire: lead.id.clone(), name: lead.name.clone())
                    <span>(lead.name.clone()) ", lead"</span>
                </div>
            }
            <div class="flex flex-wrap items-center gap-2">
                release_button(action: "approve", project: project.name.clone(), label: "Approve", kind: "primary")
                release_button(action: "cancel", project: project.name.clone(), label: "Not yet")
                <button type="button" class=(format!("ml-auto {}", ui::GHOST_SM)) data-on:click=(pick)>(see)</button>
            </div>
        </div>
    }
    .boxed())
}

/// The checklist of a release: each step done or not, with Approve and
/// Cancel on the one that waits for the owner, and Cancel while it checks.
#[component]
pub(super) async fn checklist(project: Standing, now: u64) -> topcoat::Result<impl View> {
    let Some(release) = project.release.clone() else {
        return Ok(view! {}.boxed());
    };
    let version = release.version.trim_start_matches('v').to_string();
    let waiting = release.state == ReleaseState::Waiting;
    let first_open = release.steps.iter().position(|s| !s.done);
    let rows: Vec<(String, Option<String>, bool, bool, String)> = release
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let asks = waiting && Some(i) == first_open;
            let when = step.at.filter(|_| step.done).map(|at| relative(now, at));
            let (label, command) = step_label(&release.steps, i);
            (label, command, step.done, asks, when.unwrap_or_default())
        })
        .collect();
    let order = order(&release);
    let done = rows.iter().filter(|(_, _, done, ..)| *done).count();
    let total = rows.len();
    let checking = release.state == ReleaseState::Checking;
    let failed = release.state == ReleaseState::Failed;
    let error = release.error.clone().unwrap_or_default();
    let name = project.name.clone();
    Ok(view! {
        section(label: format!("Release v{version}"), note: format!("{done} of {total} steps"),
            ui::meter(value: done as f64, max: total as f64, tone: if failed { "bad" } else { "ok" })
            <p class="m-0 text-[12.5px] leading-5 text-fg-2">(order)</p>
            <ul class="m-0 flex list-none flex-col overflow-hidden rounded-xl border border-line bg-bg-3 p-0">
                for (step, command, is_done, asks, when) in rows {
                    <li class=(format!("grid grid-cols-[22px_minmax(0,1fr)_auto] items-center gap-2.5 border-t border-line px-3 py-2.5 text-[13px] first:border-t-0 {}", if asks { "bg-attn-bg" } else { "" }))
                        data-release-step=(if is_done { "done" } else if asks { "waiting" } else { "open" })>
                        <span class=(format!("grid size-5 place-items-center rounded-full border-2 text-[11px] font-bold leading-none {}", if is_done { "border-ok bg-ok text-bg" } else if asks { "border-attn bg-attn text-bg" } else { "border-line-3" }))>
                            if is_done { "✓" } else if asks { "!" }
                        </span>
                        <span class="flex min-w-0 flex-col">
                            <b class=(if asks { "font-semibold text-attn-fg" } else { "font-medium" })>(step)</b>
                            if let Some(command) = command {
                                <small class="truncate font-mono text-[11px] text-fg-3" title=(command.clone())>(command.clone())</small>
                            }
                            if asks {
                                <small class="text-[12px] text-fg-3">"Needs your OK"</small>
                            }
                        </span>
                        if asks {
                            <span class="flex gap-1.5">
                                release_button(action: "approve", project: name.clone(), label: "Approve", kind: "primary")
                                release_button(action: "cancel", project: name.clone(), label: "Cancel", kind: "danger")
                            </span>
                        } else if !when.is_empty() {
                            <small class="whitespace-nowrap text-[12px] text-fg-3">(when)</small>
                        }
                    </li>
                }
            </ul>
            if checking {
                <div class=(inspector::ACTIONS)>
                    release_button(action: "cancel", project: name.clone(), label: "Cancel the release", kind: "danger")
                </div>
            }
            if failed {
                <p class="m-0 text-[12.5px] text-bad-fg">"The release failed" if !error.is_empty() { ": " (error.clone()) } "."</p>
            }
        )
    }
    .boxed())
}

/// "Start a release", with the version to release, when the project can be
/// released and nothing is under way.
#[component]
pub(super) async fn start_release(project: Standing) -> topcoat::Result<impl View> {
    let possible = project.releasable && project.under_way().is_none();
    if !possible {
        let file = match project.path.trim_end_matches('/') {
            "" => ".studio.toml, in the project's folder".to_string(),
            path => format!("{path}/.studio.toml"),
        };
        return Ok(view! {
            if !project.releasable {
                section(label: "Start a release",
                    quiet(text: "This project has no release commands yet, so it can't be released from here. To add them, put a line like this in its settings file, above any line in [brackets]:")
                    <pre class="m-0 overflow-x-auto rounded-lg border border-line bg-bg-3 px-3 py-2 font-mono text-[12px] leading-5 text-fg">"release = [[\"./scripts/publish.sh\"]]"</pre>
                    <p class="m-0 text-[13px] leading-5 text-fg-3">"The file: " <code class="font-mono text-[12px] text-fg-2 [overflow-wrap:anywhere]">(file)</code></p>
                    quiet(text: "Each command runs in turn once you give the go-ahead, with VERSION set to the version you release.")
                )
            }
        }
        .boxed());
    }
    // A project that is live suggests the version after the one released.
    let suggested = match (&project.last, project.phase) {
        (Some(last), Phase::Live) => next_version(&last.version),
        _ => String::new(),
    };
    let name = project.name.clone();
    Ok(view! {
        section(label: "Start a release",
            <form class="flex flex-col gap-2" data-on:submit="@post('/office/releases/start', {contentType: 'form'})">
                <input type="hidden" name="project" value=(name)>
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Version"</span>
                    <input name="version" required="" maxlength="40" autocomplete="off" spellcheck="false" placeholder="1.0" value=(suggested) class=(ui::INPUT)>
                    <span class=(ui::HINT)>"The final checks run first. Nothing is published until you say so."</span>
                </label>
                <div class=(inspector::ACTIONS)>
                    <button type="submit" class=(ui::PRIMARY)>
                        ui::icon(name: "send")
                        "Start a release"
                    </button>
                </div>
            </form>
        )
    }
    .boxed())
}

/// The last release that published, in a line.
#[component]
pub(super) async fn last_release(project: Standing, now: u64) -> topcoat::Result<impl View> {
    let Some(last) = project.last.clone() else {
        return Ok(view! {}.boxed());
    };
    let at = last.finished_at.unwrap_or(last.started_at);
    let words = format!(
        "v{} went out {}.",
        last.version.trim_start_matches('v'),
        relative(now, at)
    );
    Ok(view! {
        section(label: "Last release",
            <div class=(format!("{CARD} flex-row items-center"))>
                ui::state_icon(state: "landed")
                <span class="text-[13px]">(words)</span>
            </div>
        )
    }
    .boxed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::panel::fixtures::release;

    #[test]
    fn the_next_version_goes_up_by_one_at_the_end() {
        assert_eq!(next_version("1.0"), "1.1");
        assert_eq!(next_version("1.2.3"), "1.2.4");
        assert_eq!(next_version("v0.9"), "0.10");
        // Anything else is left for the owner to type.
        for odd in ["", "1", "2.0-beta", "1.2.3.4", "one.two", "1..2"] {
            assert_eq!(next_version(odd), "", "{odd}");
        }
    }

    #[test]
    fn release_commands_are_steps_and_the_rest_keep_their_names() {
        let release = release(ReleaseState::Waiting, 1);
        let labels: Vec<(String, Option<String>)> = (0..release.steps.len())
            .map(|i| step_label(&release.steps, i))
            .collect();
        assert_eq!(
            labels,
            [
                ("Final checks".to_string(), None),
                ("Your go-ahead".to_string(), None),
                (
                    "Release step 1".to_string(),
                    Some("./scripts/build.sh".into())
                ),
                (
                    "Release step 2".to_string(),
                    Some("./scripts/publish.sh".into())
                ),
                ("Tag v1.2".to_string(), None),
            ]
        );
    }

    #[test]
    fn the_order_reads_where_it_is_and_what_comes_next() {
        let say = |state| order(&release(state, 1));
        assert_eq!(
            say(ReleaseState::Waiting),
            "Final checks passed. Next: your go-ahead, then it publishes v1.2."
        );
        assert!(say(ReleaseState::Checking).starts_with("The final checks are running."));
        assert!(say(ReleaseState::Publishing).contains("publishing v1.2"));
    }
}
