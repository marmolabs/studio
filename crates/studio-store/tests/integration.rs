#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Barrier};
use std::thread;

use rusqlite::Connection;
use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, RunId, TicketId};
use studio_core::ticket::{HeldReason, Ticket, TicketState};
use studio_store::Store;

fn tid(id: &str) -> TicketId {
    TicketId::new(id)
}

fn pid(id: &str) -> ProjectId {
    ProjectId::new(id)
}

fn hid(id: &str) -> HireId {
    HireId::new(id)
}

fn created_event(id: &str, project: &str, created_at: u64) -> Event {
    Event::Created {
        id: tid(id),
        project: pid(project),
        title: format!("title-{id}"),
        goal: format!("goal-{id}"),
        hire: hid("h1"),
        base_branch: "main".into(),
        created_at,
        kind: Default::default(),
        parent: None,
        detail: String::new(),
        source: None,
    }
}

fn created(id: &str, project: &str, created_at: u64) -> (TicketId, u64, Event) {
    (tid(id), created_at, created_event(id, project, created_at))
}

#[test]
fn full_happy_path() {
    let mut store = Store::open_in_memory().unwrap();

    let (id, at, event) = created("t-1", "p-1", 1);
    let ticket = store.append(&id, at, &event).unwrap();
    assert_eq!(ticket.state, TicketState::Queued);

    let ticket = store.append(&id, 2, &Event::Claimed).unwrap();
    assert_eq!(ticket.state, TicketState::Claimed);

    let ticket = store
        .append(
            &id,
            3,
            &Event::RunStarted {
                run: RunId::new("r1"),
            },
        )
        .unwrap();
    assert_eq!(ticket.state, TicketState::Working);

    let ticket = store.append(&id, 4, &Event::ChecksStarted).unwrap();
    assert_eq!(ticket.state, TicketState::Checking);

    let ticket = store.append(&id, 5, &Event::ChecksPassed).unwrap();
    assert_eq!(ticket.state, TicketState::Reviewing);

    let ticket = store.append(&id, 6, &Event::ReviewPassed).unwrap();
    assert_eq!(ticket.state, TicketState::Ready);

    let ticket = store.append(&id, 7, &Event::Landed).unwrap();
    assert_eq!(ticket.state, TicketState::Landed);

    assert_eq!(
        store.ticket(&id).unwrap().map(|t| t.state),
        Some(TicketState::Landed)
    );
    assert_eq!(store.tickets().unwrap().len(), 1);
    assert_eq!(store.events(&id).unwrap().len(), 7);
}

#[test]
fn invalid_transition_leaves_store_untouched() {
    let mut store = Store::open_in_memory().unwrap();

    let (id, at, event) = created("t-1", "p-1", 1);
    store.append(&id, at, &event).unwrap();

    let before_events = store.events(&id).unwrap();
    let before_ticket = store.ticket(&id).unwrap();

    let result = store.append(&id, 2, &Event::Landed);
    assert!(result.is_err());

    assert_eq!(store.events(&id).unwrap(), before_events);
    assert_eq!(store.ticket(&id).unwrap(), before_ticket);
}

#[test]
fn duplicate_created_rejected() {
    let mut store = Store::open_in_memory().unwrap();

    let (id, at, event) = created("t-1", "p-1", 1);
    store.append(&id, at, &event).unwrap();

    let result = store.append(&id, 2, &event);
    assert!(result.is_err());
    assert_eq!(store.events(&id).unwrap().len(), 1);
}

#[test]
fn rebuild_projections_matches_live_projection() {
    let mut store = Store::open_in_memory().unwrap();

    let (a, _, a_event) = created("t-a", "p-1", 10);
    store.append(&a, 1, &a_event).unwrap();
    store.append(&a, 2, &Event::Claimed).unwrap();
    store
        .append(
            &a,
            3,
            &Event::RunStarted {
                run: RunId::new("r1"),
            },
        )
        .unwrap();
    store
        .append(
            &a,
            4,
            &Event::Held {
                reason: HeldReason::Budget,
            },
        )
        .unwrap();
    store.append(&a, 5, &Event::Resumed).unwrap();

    // First check attempt fails and retries.
    store.append(&a, 6, &Event::ChecksStarted).unwrap();
    store
        .append(
            &a,
            7,
            &Event::ChecksFailed {
                detail: "oops".into(),
            },
        )
        .unwrap();
    store.append(&a, 8, &Event::ChecksStarted).unwrap();
    store.append(&a, 9, &Event::ChecksPassed).unwrap();

    // Review fails and retries through the full cycle again.
    store
        .append(
            &a,
            10,
            &Event::ReviewFailed {
                detail: "fix".into(),
            },
        )
        .unwrap();
    store.append(&a, 11, &Event::ChecksStarted).unwrap();
    store.append(&a, 12, &Event::ChecksPassed).unwrap();
    store.append(&a, 13, &Event::ReviewPassed).unwrap();
    store.append(&a, 14, &Event::Landed).unwrap();

    let (b, _, b_event) = created("t-b", "p-1", 5);
    store.append(&b, 20, &b_event).unwrap();
    store.append(&b, 21, &Event::Claimed).unwrap();
    store
        .append(
            &b,
            22,
            &Event::RunStarted {
                run: RunId::new("r3"),
            },
        )
        .unwrap();
    store.append(&b, 23, &Event::ChecksStarted).unwrap();
    store.append(&b, 24, &Event::ChecksPassed).unwrap();
    store.append(&b, 25, &Event::ReviewPassed).unwrap();
    store
        .append(
            &b,
            26,
            &Event::ChangesRequested {
                detail: "redo".into(),
            },
        )
        .unwrap();
    store.append(&b, 27, &Event::Claimed).unwrap();
    store.append(&b, 28, &Event::Cancelled).unwrap();

    let before = store.tickets().unwrap();

    store.rebuild_projections().unwrap();

    let after = store.tickets().unwrap();
    assert_eq!(before, after);
    assert_eq!(after.len(), 2);
}

#[test]
fn persistence_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("studio.db");

    let (id, at, event) = created("t-1", "p-1", 1);
    {
        let mut store = Store::open(&path).unwrap();
        store.append(&id, at, &event).unwrap();
        store.append(&id, 2, &Event::Claimed).unwrap();
    }

    let store = Store::open(&path).unwrap();
    let ticket = store.ticket(&id).unwrap().unwrap();
    assert_eq!(ticket.state, TicketState::Claimed);
    assert_eq!(store.events(&id).unwrap().len(), 2);
}

#[test]
fn claim_next_alternates_and_exhausts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("studio.db");

    {
        let mut store = Store::open(&path).unwrap();
        for i in 0..5 {
            let id = format!("t-{i}");
            let (tid, at, event) = created(&id, "p-1", (i + 1) as u64);
            store.append(&tid, at, &event).unwrap();
        }
    }

    let barrier = Arc::new(Barrier::new(2));
    let path_a = path.clone();
    let path_b = path.clone();
    let barrier_a = Arc::clone(&barrier);
    let barrier_b = Arc::clone(&barrier);

    let handle_a = thread::spawn(move || {
        let mut store = Store::open(&path_a).unwrap();
        barrier_a.wait();
        let mut claimed = Vec::new();
        while let Some(ticket) = store.claim_next(100).unwrap() {
            claimed.push(ticket.id.to_string());
            thread::yield_now();
        }
        claimed
    });

    let handle_b = thread::spawn(move || {
        let mut store = Store::open(&path_b).unwrap();
        barrier_b.wait();
        let mut claimed = Vec::new();
        while let Some(ticket) = store.claim_next(100).unwrap() {
            claimed.push(ticket.id.to_string());
            thread::yield_now();
        }
        claimed
    });

    let mut a = handle_a.join().unwrap();
    let mut b = handle_b.join().unwrap();

    a.sort();
    b.sort();

    let all: Vec<String> = a.iter().chain(b.iter()).cloned().collect();
    let mut unique = all.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 5);
    assert_eq!(all.len(), 5);

    let mut store = Store::open(&path).unwrap();
    assert!(store.claim_next(200).unwrap().is_none());
}

#[test]
fn events_since_order_and_limit() {
    let mut store = Store::open_in_memory().unwrap();

    let (a, _, a_event) = created("t-a", "p-1", 1);
    store.append(&a, 1, &a_event).unwrap();
    store.append(&a, 2, &Event::Claimed).unwrap();

    let (b, _, b_event) = created("t-b", "p-1", 2);
    store.append(&b, 3, &b_event).unwrap();
    store.append(&b, 4, &Event::Claimed).unwrap();
    store
        .append(
            &b,
            5,
            &Event::RunStarted {
                run: RunId::new("r1"),
            },
        )
        .unwrap();

    let since = store.events_since(0, 10).unwrap();
    assert_eq!(since.len(), 5);
    assert!(since.windows(2).all(|w| w[0].seq < w[1].seq));

    let limited = store.events_since(0, 2).unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].seq, 1);
    assert_eq!(limited[1].seq, 2);

    let offset = store.events_since(2, 10).unwrap();
    assert_eq!(offset.len(), 3);
    assert_eq!(offset[0].seq, 3);
}

#[test]
fn in_flight_states() {
    let mut store = Store::open_in_memory().unwrap();

    let cases = [
        ("t-claimed", vec![Event::Claimed]),
        (
            "t-working",
            vec![
                Event::Claimed,
                Event::RunStarted {
                    run: RunId::new("r1"),
                },
            ],
        ),
        (
            "t-checking",
            vec![
                Event::Claimed,
                Event::RunStarted {
                    run: RunId::new("r2"),
                },
                Event::ChecksStarted,
            ],
        ),
        (
            "t-reviewing",
            vec![
                Event::Claimed,
                Event::RunStarted {
                    run: RunId::new("r3"),
                },
                Event::ChecksStarted,
                Event::ChecksPassed,
            ],
        ),
        (
            "t-ready",
            vec![
                Event::Claimed,
                Event::RunStarted {
                    run: RunId::new("r4"),
                },
                Event::ChecksStarted,
                Event::ChecksPassed,
                Event::ReviewPassed,
            ],
        ),
    ];

    for (i, (id, events)) in cases.iter().enumerate() {
        let (tid, at, event) = created(id, "p-1", (i + 1) as u64);
        store.append(&tid, at, &event).unwrap();
        for (j, ev) in events.iter().enumerate() {
            store.append(&tid, at + j as u64 + 1, ev).unwrap();
        }
    }

    let in_flight = store.in_flight().unwrap();
    let ids: Vec<String> = in_flight.iter().map(|t| t.id.to_string()).collect();
    assert!(ids.contains(&"t-claimed".to_string()));
    assert!(ids.contains(&"t-working".to_string()));
    assert!(ids.contains(&"t-checking".to_string()));
    assert!(ids.contains(&"t-reviewing".to_string()));
    assert!(!ids.contains(&"t-ready".to_string()));
}

#[test]
fn migrates_from_schema_version_1() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("studio.db");

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta VALUES ('schema_version', '1');
             CREATE TABLE events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 ticket_id TEXT NOT NULL,
                 at_ms INTEGER NOT NULL,
                 event TEXT NOT NULL
             );
             CREATE TABLE tickets (
                 id TEXT PRIMARY KEY,
                 project TEXT NOT NULL,
                 state_kind TEXT NOT NULL,
                 ticket TEXT NOT NULL,
                 last_seq INTEGER NOT NULL
             );",
        )
        .unwrap();
        let ticket = Ticket {
            id: tid("t-1"),
            project: pid("p-1"),
            title: "title".into(),
            goal: "goal".into(),
            hire: hid("h1"),
            state: TicketState::Queued,
            attempts: 0,
            base_branch: "main".into(),
            created_at: 123,
            held_from: None,
            feedback: None,
            kind: Default::default(),
            parent: None,
            detail: String::new(),
            source: None,
        };
        let json = serde_json::to_string(&ticket).unwrap();
        conn.execute(
            "INSERT INTO tickets (id, project, state_kind, ticket, last_seq) VALUES (?1, ?2, ?3, ?4, ?5)",
            ("t-1", "p-1", "queued", &json, 0),
        )
        .unwrap();
    }

    let store = Store::open(&path).unwrap();
    let ticket = store.ticket(&tid("t-1")).unwrap().unwrap();
    assert_eq!(ticket.created_at, 123);

    let conn = Connection::open(&path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "3");
}

#[test]
fn claim_takes_only_a_queued_ticket() {
    let mut store = Store::open_in_memory().unwrap();
    for (n, at) in [("t-1", 1), ("t-2", 2)] {
        let (id, at, event) = created(n, "p-1", at);
        store.append(&id, at, &event).unwrap();
    }
    let second = store.claim(&tid("t-2"), 10).unwrap().unwrap();
    assert_eq!(second.state, TicketState::Claimed);
    assert!(store.claim(&tid("t-2"), 11).unwrap().is_none());
    assert!(store.claim(&tid("nope"), 11).unwrap().is_none());
    let first = store.claim_next(12).unwrap().unwrap();
    assert_eq!(first.id, tid("t-1"));
}

#[test]
fn max_ask_number_reads_opened_asks() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.max_ask_number().unwrap(), 0);
    let (id, at, event) = created("t-1", "p-1", 1);
    store.append(&id, at, &event).unwrap();
    store.append(&id, 2, &Event::Claimed).unwrap();
    store
        .append(
            &id,
            3,
            &Event::RunStarted {
                run: RunId::new("r"),
            },
        )
        .unwrap();
    for ask in ["a7", "a12", "a3"] {
        store
            .append(
                &id,
                4,
                &Event::AskOpened {
                    ask: studio_core::ids::AskId::new(ask),
                },
            )
            .unwrap();
        store
            .append(
                &id,
                5,
                &Event::AskResolved {
                    ask: studio_core::ids::AskId::new(ask),
                    approved: true,
                },
            )
            .unwrap();
    }
    assert_eq!(store.max_ask_number().unwrap(), 12);
}
