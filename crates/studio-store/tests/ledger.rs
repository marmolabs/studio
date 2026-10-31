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
    assert!((all.by_model["haiku"] - 1.75).abs() < 1e-9);
    assert!((all.by_ticket["A-1"] - 0.75).abs() < 1e-9);
    assert_eq!((all.runs_by_hire["dev"], all.runs_by_hire["art"]), (2, 1));

    let recent = store.spent_since(200).unwrap();
    assert!((recent.total - 1.25).abs() < 1e-9);

    assert!((store.ticket_spent(&TicketId::new("A-1")).unwrap() - 0.75).abs() < 1e-9);
    assert!((store.spent_by_ticket().unwrap()["A-1"] - 0.75).abs() < 1e-9);
    assert!((store.session_spent("s2").unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(store.session_spent("nope").unwrap(), 0.0);
    assert_eq!(store.ledger_since(250).unwrap().len(), 1);
}

#[test]
fn spend_by_local_day_lists_each_day_up_to_today() {
    let mut store = Store::open_in_memory().unwrap();
    let now = 1_793_400_000_000;
    let days = store.spend_by_local_day(now, 14).unwrap();
    assert_eq!(days.len(), 14);
    // Each day starts a day after the one before, give or take a clock change.
    for pair in days.windows(2) {
        assert!(pair[0].0 < pair[1].0);
        let gap = pair[1].1 - pair[0].1;
        assert!((23 * 3_600_000..=25 * 3_600_000).contains(&gap), "{gap}");
    }
    let (first, today) = (days[0].1, days[13].1);
    assert!(today <= now && now - today < 86_400_000);

    store
        .record_spend(&spend(today, "A-1", "a", "dev", "s1", 0.5))
        .unwrap();
    store
        .record_spend(&spend(first, "A-1", "a", "dev", "s1", 0.25))
        .unwrap();
    store
        .record_spend(&spend(first - 1, "A-1", "a", "dev", "s1", 9.0))
        .unwrap();
    let days = store.spend_by_local_day(now, 14).unwrap();
    assert!((days[13].2 - 0.5).abs() < 1e-9);
    assert!((days[0].2 - 0.25).abs() < 1e-9);
    let total: f64 = days.iter().map(|d| d.2).sum();
    assert!((total - 0.75).abs() < 1e-9);
    assert!(store.spend_by_local_day(now, 0).unwrap().is_empty());
}
