//! Everything a ticket's runs said and did, one JSON line per event, in
//! `<data>/runs/<ticket>.jsonl`. The board's events stay in the database.

use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use studio_runtime::RunEvent;

pub(crate) struct RunLog {
    path: PathBuf,
}

impl RunLog {
    pub(crate) fn new(data_dir: &Path, ticket: &str) -> RunLog {
        RunLog {
            path: data_dir.join("runs").join(format!("{ticket}.jsonl")),
        }
    }

    /// Where the ticket's MCP config goes, next to its log.
    pub(crate) fn mcp_config_path(&self) -> PathBuf {
        self.path.with_extension("mcp.json")
    }

    pub(crate) fn event(&self, event: &RunEvent) {
        self.write(json!({ "at": crate::now_ms(), "event": event }));
    }

    /// A note from the studio itself (checks output, errors).
    pub(crate) fn note(&self, text: &str) {
        self.write(json!({ "at": crate::now_ms(), "note": text }));
    }

    /// The session id of the most recent run, to resume it.
    pub(crate) fn last_session(&self) -> Option<String> {
        let text = fs::read_to_string(&self.path).ok()?;
        text.lines().rev().find_map(|line| {
            let value: Value = serde_json::from_str(line).ok()?;
            let event = &value["event"];
            let session = match event["type"].as_str()? {
                "started" => event["session"].as_str(),
                "finished" => event["session"].as_str(),
                _ => None,
            }?;
            (!session.is_empty()).then(|| session.to_string())
        })
    }

    // A log line that can't be written must not stop the ticket.
    fn write(&self, value: Value) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{value}");
        }
    }
}
