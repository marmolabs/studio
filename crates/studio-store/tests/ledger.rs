#![allow(clippy::unwrap_used)]

use studio_core::ids::TicketId;
use studio_store::{Spend, Store};

fn spend(at_ms: u64, ticket: &str, project: &str, hire: &str, session: &str, usd: f64) -> Spend {
    Spend {
        at_ms,
        ticket_id: TicketId::new(ticket),
        project: project.into(),
        hire: hire.into(),
        runtime: "claude-code".into(),
        model: "haiku".into(),
        session: Some(session.into()),
        usd,
        reported: true,
        input_tokens: 10,
        output_tokens: 20,
    }
}

#[test]
fn totals_by_window_project_hire_ticket_and_session() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .record_spend(&spend(100, "A-1", "a", "dev", "s1", 0.5))
        .unwrap();
    store
        .record_spend(&spend(200, "A-1", "a", "dev", "s1", 0.25))
        .unwrap();
    store
        .record_spend(&spend(300, "B-1", "b", "art", "s2", 1.0))
        .unwrap();

    let all = store.spent_since(0).unwrap();
    assert!((all.total - 1.75).abs() < 1e-9);
    assert!((all.by_project["a"] - 0.75).abs() < 1e-9);
    assert!((all.by_hire["art"] - 1.0).abs() < 1e-9);

    let recent = store.spent_since(200).unwrap();
    assert!((recent.total - 1.25).abs() < 1e-9);

    assert!((store.ticket_spent(&TicketId::new("A-1")).unwrap() - 0.75).abs() < 1e-9);
    assert!((store.spent_by_ticket().unwrap()["A-1"] - 0.75).abs() < 1e-9);
    assert!((store.session_spent("s2").unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(store.session_spent("nope").unwrap(), 0.0);
    assert_eq!(store.ledger_since(250).unwrap().len(), 1);
}
