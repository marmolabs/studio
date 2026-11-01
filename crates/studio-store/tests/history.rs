#![allow(clippy::unwrap_used)]

use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, RunId, TicketId};
use studio_core::ticket::TicketKind;
use studio_store::{Spend, Store};

fn land(store: &mut Store, id: &str, hire: &str, claimed: u64, landed: u64, usd: f64) {
    let id = TicketId::new(id);
    let created = Event::Created {
        id: id.clone(),
        project: ProjectId::new("p"),
        title: "t".into(),
        goal: String::new(),
        hire: HireId::new(hire),
        base_branch: "main".into(),
        created_at: claimed - 10,
        kind: Default::default(),
        parent: None,
        detail: String::new(),
        source: None,
    };
    store.append(&id, claimed - 10, &created).unwrap();
    store.append(&id, claimed, &Event::Claimed).unwrap();
    let run = Event::RunStarted {
        run: RunId::new("r"),
    };
    store.append(&id, claimed + 1, &run).unwrap();
    store
        .append(&id, claimed + 2, &Event::ChecksStarted)
        .unwrap();
    store
        .append(&id, claimed + 3, &Event::ChecksPassed)
        .unwrap();
    store
        .append(&id, claimed + 4, &Event::ReviewPassed)
        .unwrap();
    store.append(&id, landed, &Event::Landed).unwrap();
    store
        .record_spend(&Spend {
            at_ms: claimed + 1,
            ticket_id: id,
            project: "p".into(),
            hire: hire.into(),
            runtime: "claude".into(),
            model: "m".into(),
            session: Some(format!("s-{claimed}")),
            usd,
            reported: true,
            input_tokens: 0,
            output_tokens: 0,
        })
        .unwrap();
}

#[test]
fn landed_samples_are_per_hire_newest_first_and_limited() {
    let mut store = Store::open_in_memory().unwrap();
    land(&mut store, "A-1", "dev", 100, 1_100, 0.5);
    land(&mut store, "A-2", "dev", 200, 2_200, 1.5);
    land(&mut store, "A-3", "art", 300, 9_300, 9.0);
    land(&mut store, "A-4", "dev", 400, 3_400, 2.0);
    // Still open: not a sample.
    let open = TicketId::new("A-5");
    let created = Event::Created {
        id: open.clone(),
        project: ProjectId::new("p"),
        title: "t".into(),
        goal: String::new(),
        hire: HireId::new("dev"),
        base_branch: "main".into(),
        created_at: 1,
        kind: Default::default(),
        parent: None,
        detail: String::new(),
        source: None,
    };
    store.append(&open, 1, &created).unwrap();

    let all = store.landed_samples("dev", 20).unwrap();
    assert_eq!(all, vec![(2.0, 3_000), (1.5, 2_000), (0.5, 1_000)]);
    let two = store.landed_samples("dev", 2).unwrap();
    assert_eq!(two.len(), 2);
    assert!(store.landed_samples("nobody", 20).unwrap().is_empty());
}

#[test]
fn started_at_and_session_model() {
    let mut store = Store::open_in_memory().unwrap();
    land(&mut store, "A-1", "dev", 100, 1_100, 0.5);
    assert_eq!(store.started_at(&TicketId::new("A-1")).unwrap(), Some(100));
    assert_eq!(store.started_at(&TicketId::new("nope")).unwrap(), None);
    assert_eq!(store.session_model("s-100").unwrap().as_deref(), Some("m"));
    assert_eq!(store.session_model("none").unwrap(), None);
}

#[test]
fn recent_events_span_tickets_and_stop_at_the_window() {
    let mut store = Store::open_in_memory().unwrap();
    land(&mut store, "A-1", "dev", 100, 1_100, 0.5);
    land(&mut store, "A-2", "dev", 2_000, 3_000, 1.5);
    let recent = store.events_from(2_000, 100, |_| true).unwrap();
    let seen: Vec<(String, u64)> = recent
        .iter()
        .map(|e| (e.ticket_id.to_string(), e.at_ms))
        .collect();
    assert_eq!(
        seen,
        [2_000, 2_001, 2_002, 2_003, 2_004, 3_000].map(|at| ("A-2".to_string(), at))
    );
    // Too many: the newest are kept, still oldest first.
    let newest = store.events_from(0, 2, |_| true).unwrap();
    assert_eq!(
        newest.iter().map(|e| e.at_ms).collect::<Vec<_>>(),
        [2_004, 3_000]
    );
    assert!(store.events_from(5_000, 10, |_| true).unwrap().is_empty());
    // Only the kinds asked for, and the limit counts only those.
    let landed = store
        .events_from(0, 1, |e| matches!(e, Event::Landed))
        .unwrap();
    assert_eq!(landed.iter().map(|e| e.at_ms).collect::<Vec<_>>(), [3_000]);
    let both = store
        .events_from(0, 10, |e| matches!(e, Event::Landed))
        .unwrap();
    assert_eq!(
        both.iter().map(|e| e.at_ms).collect::<Vec<_>>(),
        [1_100, 3_000]
    );
}

fn created(store: &mut Store, id: &str, project: &str, kind: TicketKind) {
    let id = TicketId::new(id);
    let event = Event::Created {
        id: id.clone(),
        project: ProjectId::new(project),
        title: "t".into(),
        goal: String::new(),
        hire: HireId::new("dev"),
        base_branch: "main".into(),
        created_at: 1,
        kind,
        parent: None,
        detail: String::new(),
        source: None,
    };
    store.append(&id, 1, &event).unwrap();
}

/// Takes a created ticket to the point where it is at work.
fn working(store: &mut Store, id: &str) {
    let id = TicketId::new(id);
    store.append(&id, 2, &Event::Claimed).unwrap();
    let run = Event::RunStarted {
        run: RunId::new("r"),
    };
    store.append(&id, 3, &run).unwrap();
}

#[test]
fn landings_are_work_tickets_with_their_first_landing_and_local_day() {
    let mut store = Store::open_in_memory().unwrap();
    // Noon UTC on two days a week apart.
    let day = 86_400_000_u64;
    let first = 100 * day + day / 2;
    let second = 107 * day + day / 2;
    land(&mut store, "A-1", "dev", first, first + 5_000, 0.5);
    land(&mut store, "A-2", "dev", second, second + 5_000, 0.5);
    created(&mut store, "A-3", "p", TicketKind::Plan);
    working(&mut store, "A-3");
    store
        .append(&TicketId::new("A-3"), second, &Event::Landed)
        .unwrap();
    created(&mut store, "A-4", "p", TicketKind::Work);

    let landings = store.landings().unwrap();
    assert_eq!(
        landings
            .iter()
            .map(|l| (l.ticket_id.to_string(), l.at_ms))
            .collect::<Vec<_>>(),
        [
            ("A-1".to_string(), first + 5_000),
            ("A-2".to_string(), second + 5_000)
        ]
    );
    assert_eq!(landings[0].date, store.local_date(first + 5_000).unwrap());
    assert_ne!(landings[0].date, landings[1].date);
    assert_eq!(landings[0].date.len(), 10);
}

#[test]
fn the_latest_checks_result_is_kept_for_each_project() {
    let mut store = Store::open_in_memory().unwrap();
    for (id, project) in [("A-1", "a"), ("A-2", "a"), ("B-1", "b"), ("C-1", "c")] {
        created(&mut store, id, project, TicketKind::Work);
        working(&mut store, id);
    }
    let mut at = |id: &str, ms: u64, event: Event| {
        store.append(&TicketId::new(id), ms, &event).unwrap();
    };
    for id in ["A-1", "A-2", "B-1", "C-1"] {
        at(id, 9, Event::ChecksStarted);
    }
    at("A-1", 10, Event::ChecksPassed);
    at("A-2", 20, Event::ChecksFailed { detail: "x".into() });
    // B-1 fails, is sent back, and checks again.
    at("B-1", 30, Event::ChecksFailed { detail: "x".into() });
    at("B-1", 33, Event::ChecksStarted);
    at("B-1", 40, Event::ChecksPassed);

    let latest: Vec<(String, String, bool, u64)> = store
        .latest_checks()
        .unwrap()
        .into_iter()
        .map(|c| (c.project, c.ticket_id.to_string(), c.passed, c.at_ms))
        .collect();
    assert_eq!(
        latest,
        [
            ("a".to_string(), "A-2".to_string(), false, 20),
            ("b".to_string(), "B-1".to_string(), true, 40),
        ]
    );
}
