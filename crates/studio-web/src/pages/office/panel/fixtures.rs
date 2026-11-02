//! What the panel's tests share: a small office, what studiod adds to it, and
//! the panel as HTML.

use super::*;
use crate::pages::office::inspector::Detail;
use crate::ui;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use studio_core::ticket::Ticket;
use studiod::api::{
    AskView, CheckResult, DaySpend, HireDay, ProjectCheck, ProjectView, ReleaseState, ReleaseStep,
    ReleaseView, RuleFile, Trophy,
};
use topcoat::context::Cx;
use topcoat::view::view;

pub(super) fn ticket(id: &str, project: &str, state: Value, extra: Value) -> Ticket {
    let mut t = json!({
        "id": id, "project": project, "title": format!("Title {id}"),
        "goal": "", "hire": "dev", "state": state,
        "attempts": 0, "base_branch": "main", "created_at": 0,
    });
    if let (Some(t), Some(extra)) = (t.as_object_mut(), extra.as_object()) {
        t.extend(extra.clone());
    }
    serde_json::from_value(t).unwrap()
}

pub(super) const HOUR: u64 = 3_600_000;
pub(super) const NOW: u64 = 100 * HOUR;

/// A project as studiod reports it, without a lead and able to release.
pub(super) fn project_view(
    name: &str,
    release: Option<ReleaseView>,
    last_release: Option<ReleaseView>,
) -> ProjectView {
    ProjectView {
        name: name.into(),
        path: format!("/work/{name}"),
        lead: None,
        releasable: true,
        release,
        last_release,
    }
}

/// A release of shop as v1.2 in five steps, `done` of them done: it started
/// 20 minutes ago.
pub(super) fn release(state: ReleaseState, done: usize) -> ReleaseView {
    release_from(state, done, NOW - 20 * 60_000)
}

/// As `release`, started at `start`.
pub(super) fn release_from(state: ReleaseState, done: usize, start: u64) -> ReleaseView {
    ReleaseView {
        project: "shop".into(),
        version: "1.2".into(),
        state,
        steps: [
            "Final checks",
            "Your go-ahead",
            "./scripts/build.sh",
            "./scripts/publish.sh",
            "Tag v1.2",
        ]
        .iter()
        .enumerate()
        .map(|(i, name)| ReleaseStep {
            name: (*name).into(),
            done: i < done,
            at: (i < done).then_some(start + 60_000 * (10 + i as u64)),
        })
        .collect(),
        started_at: start,
        finished_at: None,
        error: None,
    }
}

/// Two projects, a goal with a landed and a working ticket, a queued
/// ticket, a failed one, a ready one, a held one and a loose one.
pub(super) fn office() -> Office {
    let status: client::Status = serde_json::from_value(json!({
        "asks": 1, "running": 2, "projects": ["shop", "api"],
        "tickets": {"working": 1, "ready": 1},
        "daily_usd": 10.0, "spent_24h": 6.4, "max_runs": 3,
        "team": [
            {"id": "dev", "role": "engineer", "model": "sonnet"},
            {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
            {"id": "reviewer", "role": "reviewer", "model": "opus"},
            {"id": "approver", "role": "approver", "model": "haiku"},
            {"id": "router", "role": "router", "model": "haiku"},
        ],
    }))
    .unwrap();
    let tickets = vec![
        ticket(
            "SHOP-1",
            "shop",
            json!("landed"),
            json!({"kind": "plan", "goal": "Checkout flow", "version": "0.4"}),
        ),
        ticket(
            "SHOP-2",
            "shop",
            json!("landed"),
            json!({"parent": "SHOP-1"}),
        ),
        ticket(
            "SHOP-3",
            "shop",
            json!("working"),
            json!({"parent": "SHOP-1"}),
        ),
        ticket(
            "SHOP-4",
            "shop",
            json!("queued"),
            json!({"parent": "SHOP-1"}),
        ),
        ticket("SHOP-5", "shop", json!("ready"), json!({})),
        ticket(
            "API-1",
            "api",
            json!({"failed": {"class": "checks", "detail": "clippy"}}),
            json!({}),
        ),
        ticket(
            "API-2",
            "api",
            json!({"held": {"reason": "paused"}}),
            json!({}),
        ),
        ticket("API-3", "api", json!("queued"), json!({})),
    ];
    let at = |ticket: &str, event: Value, ago: u64| -> client::Happened {
        serde_json::from_value(json!({
            "ticket_id": ticket, "at_ms": NOW - ago, "event": event,
        }))
        .unwrap()
    };
    let mut today = BTreeMap::new();
    today.insert(
        "approver".to_string(),
        HireDay {
            allowed: 7,
            denied: 2,
            to_owner: 1,
            ..HireDay::default()
        },
    );
    let mut spend = BTreeMap::new();
    spend.insert("SHOP-3".to_string(), 1.5);
    let ask = AskView {
        id: "a1".into(),
        ticket: "SHOP-3".into(),
        hire: "dev".into(),
        kind: "permission".into(),
        tool: "Bash".into(),
        input: json!({"command": "cargo run -- --port 8080"}),
        reasons: vec![],
        at_ms: NOW - 60_000,
    };
    // shop is led by nora and is releasing v1.2, which waits for the owner;
    // api has no lead, can be released and never was.
    let projects = vec![
        ProjectView {
            lead: Some("nora".into()),
            ..project_view(
                "shop",
                Some(release(ReleaseState::Waiting, 1)),
                Some(ReleaseView {
                    version: "0.3".into(),
                    finished_at: Some(NOW - 4 * 24 * HOUR),
                    ..release_from(ReleaseState::Released, 5, NOW - 4 * 24 * HOUR - 20 * 60_000)
                }),
            )
        },
        project_view("api", None, None),
    ];
    Office {
        status,
        tickets,
        projects,
        asks: vec![ask],
        recent: vec![
            at("SHOP-2", json!({"type": "landed"}), 2 * HOUR),
            at(
                "API-1",
                json!({"type": "failed", "class": "checks", "detail": "clippy"}),
                HOUR,
            ),
        ],
        today,
        spend,
        now: NOW,
        ..Office::default()
    }
}

fn budget() -> Budget {
    serde_json::from_value(json!({
        "limits": {"daily_usd": 10.0, "ticket_usd": 2.5, "run_usd": null},
        "spent_24h": {
            "total": 6.4,
            "by_hire": {"dev": 4.4, "reviewer": 0.5},
            "by_ticket": {"SHOP-3": 3.9, "API-1": 1.1},
        },
        "reserved": {"SHOP-3": 0.72},
    }))
    .unwrap()
}

pub(super) fn extra() -> Extra {
    let days = (0..14)
        .map(|i| DaySpend {
            date: format!("2026-10-{:02}", 19 + i),
            start_ms: 0,
            usd: if i == 13 { 6.4 } else { f64::from(i) * 0.5 },
        })
        .collect();
    Extra {
        budget: Some(budget()),
        rules: vec![RuleFile {
            origin: "studio".into(),
            owner: String::new(),
            name: "no-push".into(),
            path: "/rules/no-push.md".into(),
            text: "---\nkind: gate\nlevel: block\n---\nAgents never push.\n".into(),
            problem: None,
            disabled: false,
        }],
        trophies: vec![
            Trophy {
                id: "first-goal-merged".into(),
                title: "First goal merged".into(),
                detail: "A whole goal landed".into(),
                earned_at: Some(NOW - 3 * HOUR),
                progress: None,
            },
            Trophy {
                id: "landed-25".into(),
                title: "25 tickets landed".into(),
                detail: "Land 25 tickets".into(),
                earned_at: None,
                progress: Some((12, 25)),
            },
        ],
        checks: vec![ProjectCheck {
            project: "shop".into(),
            verify: vec![vec!["cargo".into(), "test".into()]],
            running: vec!["SHOP-3".into()],
            latest: Some(CheckResult {
                ticket: "SHOP-2".into(),
                passed: true,
                at_ms: NOW - 5 * 60_000,
            }),
        }],
        days,
    }
}

/// The inner HTML of one tab's pane, from the panel's.
pub(super) fn pane<'a>(html: &'a str, tab: &str) -> &'a str {
    crate::probe::find(html, "data-show", &format!("$tab == '{tab}'")).inner_html()
}

pub(super) async fn html_of(office: Office, extra: Extra, sel: &str) -> String {
    html_on(office, extra, sel, "needs").await
}

pub(super) async fn html_on(office: Office, extra: Extra, sel: &str, tab: &str) -> String {
    let cx = Cx::default();
    let (sel, tab) = (sel.to_string(), tab.to_string());
    ui::html(
        &cx,
        view! { cx => panel(office: office, detail: Detail::default(), extra: extra, sel: sel, tab: tab, history: HISTORY_STEP) },
    )
    .await
    .unwrap()
}
