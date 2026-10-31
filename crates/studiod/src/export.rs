//! A ticket as Markdown, for a PR description or the record: what
//! `studio export` prints and the web UI's export link serves.

use serde_json::Value;

/// The summary of the last finished run in a run log, if it gave one.
pub fn summary(log: &str) -> Option<String> {
    log.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        let event = &value["event"];
        (event["type"] == "finished")
            .then(|| event["summary"].as_str().map(str::to_string))
            .flatten()
    })
}

/// What export needs from the daemon and the run log.
pub struct Export<'a> {
    pub ticket: &'a Value,
    pub spend_usd: f64,
    pub events: &'a Value,
    pub summary: Option<&'a str>,
    pub diffstat: Option<&'a str>,
    /// The `Evidence` API's list of `{attempt, name, path}`.
    pub evidence: &'a Value,
}

pub fn render(e: &Export) -> String {
    let t = e.ticket;
    let text = |key: &str| t[key].as_str().unwrap_or_default();
    let state = match &t["state"] {
        Value::String(s) => s.clone(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    };
    let mut out = format!("# {}\n\n", text("title"));
    out.push_str(&format!("- Ticket: {}\n", text("id")));
    out.push_str(&format!("- Project: {}\n", text("project")));
    out.push_str(&format!("- Hire: {}\n", text("hire")));
    out.push_str(&format!("- State: {state}\n"));
    out.push_str(&format!("- Spend: ${:.2}\n", e.spend_usd));
    for (heading, body) in [("Goal", text("goal")), ("Detail", text("detail"))] {
        if !body.trim().is_empty() {
            out.push_str(&format!("\n## {heading}\n\n{}\n", body.trim()));
        }
    }
    let events = e.events.as_array().cloned().unwrap_or_default();
    if !events.is_empty() {
        out.push_str("\n## Timeline\n\n");
        for event in &events {
            out.push_str(&format!(
                "- {} {}\n",
                utc(event["at_ms"].as_u64().unwrap_or(0)),
                describe(&event["event"])
            ));
        }
    }
    if let Some(summary) = e.summary.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("\n## Summary\n\n{}\n", summary.trim()));
    }
    if let Some(stat) = e.diffstat.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("\n## Changes\n\n```\n{}\n```\n", stat.trim_end()));
    }
    let evidence = e.evidence.as_array().cloned().unwrap_or_default();
    if !evidence.is_empty() {
        out.push_str("\n## Evidence\n\n");
        for file in &evidence {
            out.push_str(&format!(
                "- attempt {}: {} ({})\n",
                file["attempt"],
                file["name"].as_str().unwrap_or_default(),
                file["path"].as_str().unwrap_or_default()
            ));
        }
    }
    out
}

/// "checks failed: detail" for an event.
fn describe(event: &Value) -> String {
    let mut line = event["type"].as_str().unwrap_or("?").replace('_', " ");
    let extra = ["reason", "class", "detail"]
        .iter()
        .find_map(|k| event[k].as_str())
        .or_else(|| event["reason"].as_str());
    if let Some(extra) = extra.filter(|x| !x.is_empty()) {
        let first = extra.lines().next().unwrap_or_default();
        line.push_str(&format!(": {first}"));
    }
    line
}

/// "2026-10-30 12:34 UTC" for epoch milliseconds.
fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dates_are_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_782_822_840_000), "2026-06-30 12:34 UTC");
    }

    #[test]
    fn the_summary_is_the_last_runs() {
        let log = [
            r#"{"event":{"type":"finished","summary":"First"}}"#,
            r#"{"event":{"type":"text","text":"More"}}"#,
            r#"{"event":{"type":"finished","summary":"Second"}}"#,
        ]
        .join("\n");
        assert_eq!(summary(&log).as_deref(), Some("Second"));
        assert_eq!(summary("not json"), None);
    }

    #[test]
    fn renders_the_record() {
        let ticket = json!({
            "id": "GAME-1", "project": "game", "hire": "dev", "title": "Add a level",
            "goal": "First level", "detail": "", "state": "landed"
        });
        let events = json!([
            { "at_ms": 0, "event": { "type": "created" } },
            { "at_ms": 60_000, "event": { "type": "checks_failed", "detail": "boom\nmore" } },
        ]);
        let md = render(&Export {
            ticket: &ticket,
            spend_usd: 0.5,
            events: &events,
            summary: Some("Added it"),
            diffstat: Some(" a.rs | 2 +-\n"),
            evidence: &json!([{ "attempt": 0, "name": "title.png", "path": "/w/title.png" }]),
        });
        assert!(md.starts_with("# Add a level\n"));
        assert!(md.contains("- Spend: $0.50"));
        assert!(md.contains("## Goal\n\nFirst level"));
        assert!(!md.contains("## Detail"));
        assert!(md.contains("- 1970-01-01 00:01 UTC checks failed: boom"));
        assert!(md.contains("## Summary\n\nAdded it"));
        assert!(md.contains("```\n a.rs | 2 +-\n```"));
        assert!(md.contains("## Evidence\n\n- attempt 0: title.png (/w/title.png)"));
    }
}
