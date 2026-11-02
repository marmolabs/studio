//! The Log tab: what happened lately, newest first, tasks and releases.

use studio_core::event::Event as TicketEvent;
use studiod::api::{ReleaseState, ReleaseView};
use topcoat::view::{View, component, view};

use super::{quiet, tint};
use crate::pages::office::facts::{self, dot_class};
use crate::pages::office::inspector::{self};
use crate::pages::office::{Office, ago, key, select};

/// The Log tab lists this many events.
const LOG_SHOWN: usize = 50;
/// How far back the log reaches, as the office's events do.
const DAY: u64 = 24 * 3_600_000;

/// What a line of the log is about: a task, or a project's release.
#[derive(Clone, Debug, PartialEq)]
enum About {
    Task(String),
    Project(String),
}

/// One line of the log.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    at: u64,
    /// The state its dot takes the colour of.
    state: &'static str,
    about: About,
    said: String,
}

/// What a release did, as lines: it started, its checks passed, it waits for
/// the owner, and how it ended. Each has the time its step or the release
/// gives; a line without a time is left out.
fn release_lines(release: &ReleaseView) -> Vec<Line> {
    let v = release.version.trim_start_matches('v');
    let line = |at: u64, state: &'static str, said: String| Line {
        at,
        state,
        about: About::Project(release.project.clone()),
        said,
    };
    let mut out = vec![line(
        release.started_at,
        "working",
        format!("started releasing v{v}"),
    )];
    let checked = release.steps.first().filter(|s| s.done).and_then(|s| s.at);
    if let Some(at) = checked {
        out.push(line(at, "landed", format!("v{v}: the final checks passed")));
        out.push(line(at, "asking", format!("v{v} waits for your go-ahead")));
    }
    let last = release
        .steps
        .iter()
        .filter(|s| s.done)
        .filter_map(|s| s.at)
        .max()
        .unwrap_or(release.started_at);
    let ended = release.finished_at.unwrap_or(last);
    match release.state {
        ReleaseState::Released => out.push(line(ended, "landed", format!("v{v} is published"))),
        ReleaseState::Failed => {
            let why = release
                .error
                .as_deref()
                .filter(|e| !e.is_empty())
                .map(|e| format!(": {e}"))
                .unwrap_or_default();
            out.push(line(
                ended,
                "failed",
                format!("the release of v{v} failed{why}"),
            ));
        }
        ReleaseState::Cancelled => out.push(line(
            ended,
            "cancelled",
            format!("the release of v{v} was called off"),
        )),
        ReleaseState::Checking | ReleaseState::Waiting | ReleaseState::Publishing => {}
    }
    out
}

/// The log: the office's task events and the projects' releases, newest
/// first, from the last day.
fn lines(office: &Office) -> Vec<Line> {
    let since = office.now.saturating_sub(DAY);
    let mut out: Vec<Line> = office
        .recent
        .iter()
        .filter(|e| !matches!(e.event, TicketEvent::RunStarted { .. }))
        .map(|e| Line {
            at: e.at_ms,
            state: facts::event_state(&e.event),
            about: About::Task(e.ticket_id.clone()),
            said: inspector::event_words(&e.event, &office.asks),
        })
        .collect();
    let mut seen: Vec<&ReleaseView> = Vec::new();
    for project in &office.projects {
        for release in project.release.iter().chain(project.last_release.iter()) {
            // The release under way and the last one that published can be
            // the same release.
            let twice = seen.iter().any(|r| {
                (&r.project, &r.version, r.started_at)
                    == (&release.project, &release.version, release.started_at)
            });
            if !twice {
                seen.push(release);
            }
        }
    }
    out.extend(seen.into_iter().flat_map(release_lines));
    out.retain(|l| l.at >= since);
    // Newest first; for one moment, the later step first.
    out.reverse();
    out.sort_by_key(|l| std::cmp::Reverse(l.at));
    out.truncate(LOG_SHOWN);
    out
}

#[component]
pub(super) async fn log_pane(office: Office) -> topcoat::Result<impl View> {
    let now = office.now;
    let rows: Vec<(String, String, Option<usize>, About, String)> = lines(&office)
        .into_iter()
        .map(|l| {
            let project = match &l.about {
                About::Task(id) => office
                    .ticket(id)
                    .map(|t| tint(&office, &t.project.to_string())),
                About::Project(name) => Some(tint(&office, name)),
            };
            (
                ago(now.saturating_sub(l.at)),
                format!("dot {}", facts::dot(l.state)),
                project,
                l.about,
                l.said,
            )
        })
        .collect();
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "Nothing has happened lately.")
        }
        <ul class="m-0 flex list-none flex-col gap-2 p-0 text-[13px] text-fg-2">
            for (when, dot, project, about, said) in rows {
                <li class="grid grid-cols-[28px_8px_9px_minmax(0,1fr)] items-baseline gap-2">
                    <time class="text-[12px] tabular-nums text-fg-3">(when)</time>
                    <span class=(dot)></span>
                    if let Some(index) = project {
                        <i class=(dot_class(index))></i>
                    } else {
                        <i></i>
                    }
                    <span class="min-w-0">
                        if let About::Task(id) = about.clone() {
                            <a href=(format!("/tickets/{id}")) class=(inspector::TICKET_LINK)>(id.clone())</a>
                        } else if let About::Project(name) = about.clone() {
                            <button type="button" class=(format!("cursor-pointer border-0 bg-transparent p-0 {}", inspector::TICKET_LINK)) data-on:click=(select(&key("projects", &name)))>(name.clone())</button>
                        }
                        " " (said)
                    </span>
                </li>
            }
        </ul>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::text_of;

    use crate::pages::office::panel::fixtures::{
        HOUR, NOW, extra, html_of, office, project_view, release,
    };

    #[tokio::test]
    async fn the_log_is_newest_first_with_project_dots() {
        let html = html_of(office(), extra(), "").await;
        let log = &html[html.find(r#"data-show="$tab == 'log'""#).unwrap()..];
        let api = log.find(">API-1<").unwrap();
        let shop = log.find(">SHOP-2<").unwrap();
        assert!(api < shop, "{log}");
        assert!(log.contains("failed"), "{log}");
        assert!(log.contains(">1h<") && log.contains(">2h<"), "{log}");
    }

    fn said(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.said.as_str()).collect()
    }

    #[test]
    fn a_release_waiting_for_the_go_ahead_has_its_lines() {
        let mut office = office();
        office.recent.clear();
        // Started 20 minutes ago; the checks passed 9 minutes ago.
        office.projects[0].release = Some(release(ReleaseState::Waiting, 1));
        office.projects[0].last_release = None;
        let lines = lines(&office);
        assert_eq!(
            said(&lines),
            [
                "v1.2 waits for your go-ahead",
                "v1.2: the final checks passed",
                "started releasing v1.2",
            ]
        );
        assert!(
            lines
                .iter()
                .all(|l| l.about == About::Project("shop".into()))
        );
    }

    #[test]
    fn a_release_that_ended_says_how() {
        let mut office = office();
        office.recent.clear();
        office.projects[0].last_release = None;
        let ended = |state, error: Option<&str>| {
            let mut r = release(state, 2);
            r.finished_at = Some(NOW - 60_000);
            r.error = error.map(str::to_string);
            r
        };
        let words = |office: &Office| lines(office)[0].said.clone();
        office.projects[0].release = Some(ended(ReleaseState::Released, None));
        assert_eq!(words(&office), "v1.2 is published");
        office.projects[0].release = Some(ended(ReleaseState::Failed, Some("tests failed")));
        assert_eq!(words(&office), "the release of v1.2 failed: tests failed");
        office.projects[0].release = Some(ended(ReleaseState::Cancelled, None));
        assert_eq!(words(&office), "the release of v1.2 was called off");
    }

    #[test]
    fn old_releases_and_repeats_stay_out() {
        let mut office = office();
        office.recent.clear();
        // The last release is four days old: not lately.
        office.projects[0].release = None;
        assert!(lines(&office).is_empty());
        // One that is both the current and the last release counts once.
        let mut done = release(ReleaseState::Released, 3);
        done.finished_at = Some(NOW - HOUR);
        office.projects[0].release = Some(done.clone());
        office.projects[0].last_release = Some(done);
        office.projects[1] = project_view("api", None, None);
        let said_once = said(&lines(&office))
            .iter()
            .filter(|s| s.contains("is published"))
            .count();
        assert_eq!(said_once, 1);
    }

    #[tokio::test]
    async fn release_lines_show_in_the_log_with_their_project() {
        let html = html_of(office(), extra(), "").await;
        let log = &html[html.find(r#"data-show="$tab == 'log'""#).unwrap()..];
        let text = text_of(log);
        assert!(text.contains("shop v1.2 waits for your go-ahead"), "{text}");
        assert!(text.contains("shop started releasing v1.2"), "{text}");
        assert!(log.contains("$sel = 'projects/shop'"), "{log}");
    }
}
