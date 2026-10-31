#![allow(clippy::unwrap_used)]

use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, RunId, TicketId};
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
