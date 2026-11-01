use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use studio_core::error::{ReplayError, TransitionError};
use studio_core::event::Event;
use studio_core::ids::TicketId;
use studio_core::machine;
use studio_core::ticket::Ticket;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("transition error: {0}")]
    Transition(#[from] TransitionError),
    #[error("replay error: {0}")]
    Replay(#[from] ReplayError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown ticket: {0}")]
    UnknownTicket(String),
    #[error("duplicate create for ticket: {0}")]
    DuplicateCreate(String),
    #[error("id mismatch: expected {expected}, got {got}")]
    IdMismatch { expected: String, got: String },
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub seq: i64,
    pub ticket_id: TicketId,
    pub at_ms: u64,
    pub event: Event,
}

/// One run's spend, as recorded in the ledger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    pub at_ms: u64,
    pub ticket_id: TicketId,
    pub project: String,
    pub hire: String,
    pub runtime: String,
    pub model: String,
    pub session: Option<String>,
    pub usd: f64,
    /// True when the runtime reported the cost; false when the studio estimated it.
    pub reported: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Spend since some moment, in total and per project, hire, model and ticket.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SpendTotals {
    pub total: f64,
    pub by_project: std::collections::BTreeMap<String, f64>,
    pub by_hire: std::collections::BTreeMap<String, f64>,
    pub by_model: std::collections::BTreeMap<String, f64>,
    pub by_ticket: std::collections::BTreeMap<String, f64>,
    /// Runs per hire: each run books one ledger row.
    pub runs_by_hire: std::collections::BTreeMap<String, u64>,
}

/// A work ticket's landing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landing {
    pub ticket_id: TicketId,
    /// When it first landed, in epoch milliseconds.
    pub at_ms: u64,
    /// The local calendar day of that moment, as `YYYY-MM-DD`.
    pub date: String,
}

/// How a project's checks last ended, on one of its tickets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksResult {
    pub project: String,
    pub ticket_id: TicketId,
    pub passed: bool,
    /// Epoch milliseconds.
    pub at_ms: u64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Store> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;")?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Store> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn append(&mut self, ticket_id: &TicketId, at_ms: u64, event: &Event) -> Result<Ticket> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ticket = append_in_tx(&tx, ticket_id, at_ms, event)?;
        tx.commit()?;
        Ok(ticket)
    }

    pub fn ticket(&self, id: &TicketId) -> Result<Option<Ticket>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ticket FROM tickets WHERE id = ?1")?;
        let mut rows = stmt.query([&id.to_string()])?;
        match rows.next()? {
            Some(row) => {
                let json: String = row.get(0)?;
                Ok(Some(serde_json::from_str(&json)?))
            }
            None => Ok(None),
        }
    }

    pub fn tickets(&self) -> Result<Vec<Ticket>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ticket FROM tickets ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        collect_tickets(rows)
    }

    pub fn tickets_in(&self, state_kind: &str) -> Result<Vec<Ticket>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ticket FROM tickets WHERE state_kind = ?1 ORDER BY created_at, id")?;
        let rows = stmt.query_map([state_kind], |row| row.get::<_, String>(0))?;
        collect_tickets(rows)
    }

    pub fn events(&self, ticket_id: &TicketId) -> Result<Vec<StoredEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, ticket_id, at_ms, event FROM events WHERE ticket_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([&ticket_id.to_string()], event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn events_since(&self, seq: i64, limit: usize) -> Result<Vec<StoredEvent>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT seq, ticket_id, at_ms, event FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
        )?;
        let rows = stmt.query_map((seq, limit), event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Every ticket's events at or after `at_ms` that `keep` accepts, oldest
    /// first: the newest `limit` of them when there are more. Reads back from
    /// the newest and stops at the first older event, so it never scans the
    /// whole history.
    pub fn events_from(
        &self,
        at_ms: u64,
        limit: usize,
        keep: impl Fn(&Event) -> bool,
    ) -> Result<Vec<StoredEvent>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, ticket_id, at_ms, event FROM events ORDER BY seq DESC")?;
        let rows = stmt.query_map([], event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            let event = row?;
            if event.at_ms < at_ms || events.len() == limit {
                break;
            }
            if keep(&event.event) {
                events.push(event);
            }
        }
        events.reverse();
        Ok(events)
    }

    /// The whole history of every ticket with an event at or after
    /// `since_ms` or a run under way (claimed, working, checking, reviewing
    /// or asking), oldest first.
    pub fn activity_since(&self, since_ms: u64) -> Result<Vec<StoredEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, ticket_id, at_ms, event FROM events WHERE ticket_id IN (
                 SELECT ticket_id FROM events WHERE at_ms >= ?1
                 UNION SELECT id FROM tickets
                 WHERE state_kind IN ('claimed', 'working', 'checking', 'reviewing', 'asking')
             ) ORDER BY seq",
        )?;
        let rows = stmt.query_map([since_ms as i64], event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    pub fn claim_next(&mut self, at_ms: u64) -> Result<Option<Ticket>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare(
            "SELECT ticket FROM tickets WHERE state_kind = 'queued' ORDER BY created_at, id LIMIT 1",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut queued = collect_tickets(rows)?;
        let Some(ticket) = queued.pop() else {
            drop(stmt);
            tx.rollback()?;
            return Ok(None);
        };
        let ticket = append_in_tx(&tx, &ticket.id, at_ms, &Event::Claimed)?;
        drop(stmt);
        tx.commit()?;
        Ok(Some(ticket))
    }

    /// Claims this ticket if it is still queued.
    pub fn claim(&mut self, id: &TicketId, at_ms: u64) -> Result<Option<Ticket>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match load_projection(&tx, id)? {
            Some(ticket) if ticket.state.kind() == "queued" => {
                let ticket = append_in_tx(&tx, id, at_ms, &Event::Claimed)?;
                tx.commit()?;
                Ok(Some(ticket))
            }
            _ => Ok(None),
        }
    }

    /// The highest number N among the `aN` ask ids opened so far, or 0.
    pub fn max_ask_number(&self) -> Result<u64> {
        let mut stmt = self.conn.prepare(
            "SELECT json_extract(event, '$.ask') FROM events
             WHERE json_extract(event, '$.type') = 'ask_opened'",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut max = 0;
        for row in rows {
            if let Some(n) = row?.strip_prefix('a').and_then(|n| n.parse::<u64>().ok()) {
                max = max.max(n);
            }
        }
        Ok(max)
    }

    pub fn in_flight(&self) -> Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare(
            "SELECT ticket FROM tickets WHERE state_kind IN ('claimed', 'working', 'checking', 'reviewing') ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        collect_tickets(rows)
    }

    pub fn record_spend(&mut self, spend: &Spend) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ledger (at_ms, ticket_id, project, hire, runtime, model, session, usd,
                                 reported, input_tokens, output_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                spend.at_ms as i64,
                spend.ticket_id.to_string(),
                spend.project,
                spend.hire,
                spend.runtime,
                spend.model,
                spend.session,
                spend.usd,
                spend.reported,
                spend.input_tokens as i64,
                spend.output_tokens as i64,
            ],
        )?;
        Ok(())
    }

    /// Spend recorded at or after `since_ms`.
    pub fn spent_since(&self, since_ms: u64) -> Result<SpendTotals> {
        let mut totals = SpendTotals::default();
        let mut stmt = self.conn.prepare(
            "SELECT project, hire, model, ticket_id, SUM(usd), COUNT(*) FROM ledger WHERE at_ms >= ?1
             GROUP BY project, hire, model, ticket_id",
        )?;
        let rows = stmt.query_map([since_ms as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (project, hire, model, ticket, usd, runs) = row?;
            totals.total += usd;
            *totals.runs_by_hire.entry(hire.clone()).or_default() += runs.max(0) as u64;
            *totals.by_project.entry(project).or_default() += usd;
            *totals.by_hire.entry(hire).or_default() += usd;
            *totals.by_model.entry(model).or_default() += usd;
            *totals.by_ticket.entry(ticket).or_default() += usd;
        }
        Ok(totals)
    }

    /// Everything a ticket has cost, across all its runs.
    pub fn ticket_spent(&self, ticket_id: &TicketId) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(usd), 0) FROM ledger WHERE ticket_id = ?1",
            [ticket_id.to_string()],
            |row| row.get(0),
        )?)
    }

    /// What each ticket with any spend has cost, by ticket id.
    pub fn spent_by_ticket(&self) -> Result<std::collections::BTreeMap<String, f64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ticket_id, SUM(usd) FROM ledger GROUP BY ticket_id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// What one hire's runs on one ticket have spent.
    pub fn ticket_hire_spent(&self, ticket_id: &TicketId, hire: &str) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(usd), 0) FROM ledger WHERE ticket_id = ?1 AND hire = ?2",
            [ticket_id.to_string(), hire.to_string()],
            |row| row.get(0),
        )?)
    }

    /// Spend and work time of the hire's most recent landed tickets, newest
    /// first, as `(usd, ms)`. Work time runs from the first claim to landing.
    pub fn landed_samples(&self, hire: &str, limit: usize) -> Result<Vec<(f64, u64)>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT l.at_ms - c.at_ms,
                    (SELECT COALESCE(SUM(usd), 0) FROM ledger WHERE ticket_id = l.ticket_id)
             FROM events l
             JOIN tickets t ON t.id = l.ticket_id
             JOIN events c ON c.seq = (
                 SELECT MIN(seq) FROM events
                 WHERE ticket_id = l.ticket_id AND json_extract(event, '$.type') = 'claimed')
             WHERE t.state_kind = 'landed'
               AND json_extract(t.ticket, '$.hire') = ?1
               AND json_extract(l.event, '$.type') = 'landed'
             ORDER BY l.seq DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map((hire, limit), |row| {
            Ok((row.get::<_, f64>(1)?, row.get::<_, i64>(0)?.max(0) as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// When each ticket last changed: its latest event's time, by ticket id.
    pub fn updated_at(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT t.id, e.at_ms FROM tickets t JOIN events e ON e.seq = t.last_seq")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// When each running ticket's latest run began (its last claim) and each
    /// held ticket was held (its last hold), by ticket id.
    pub fn state_since(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.ticket_id, MAX(e.at_ms) FROM tickets t JOIN events e ON e.ticket_id = t.id
             WHERE t.state_kind IN ('claimed', 'working', 'checking', 'reviewing', 'held')
               AND json_extract(e.event, '$.type') =
                   CASE t.state_kind WHEN 'held' THEN 'held' ELSE 'claimed' END
             GROUP BY e.ticket_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// When the ticket was first claimed, if it ever was.
    pub fn started_at(&self, ticket_id: &TicketId) -> Result<Option<u64>> {
        let at: Option<i64> = self.conn.query_row(
            "SELECT MIN(at_ms) FROM events
             WHERE ticket_id = ?1 AND json_extract(event, '$.type') = 'claimed'",
            [ticket_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(at.map(|at| at as u64))
    }

    /// The model a runtime session last ran on, from the ledger.
    pub fn session_model(&self, session: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT model FROM ledger WHERE session = ?1 ORDER BY id DESC LIMIT 1")?;
        let mut rows = stmt.query([session])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    /// What has been recorded for one runtime session so far.
    pub fn session_spent(&self, session: &str) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(usd), 0) FROM ledger WHERE session = ?1",
            [session],
            |row| row.get(0),
        )?)
    }

    /// Ledger rows at or after `since_ms`, oldest first.
    pub fn ledger_since(&self, since_ms: u64) -> Result<Vec<Spend>> {
        let mut stmt = self.conn.prepare(
            "SELECT at_ms, ticket_id, project, hire, runtime, model, session, usd, reported,
                    input_tokens, output_tokens
             FROM ledger WHERE at_ms >= ?1 ORDER BY at_ms, id",
        )?;
        let rows = stmt.query_map([since_ms as i64], |row| {
            Ok(Spend {
                at_ms: row.get::<_, i64>(0)? as u64,
                ticket_id: TicketId::new(row.get::<_, String>(1)?),
                project: row.get(2)?,
                hire: row.get(3)?,
                runtime: row.get(4)?,
                model: row.get(5)?,
                session: row.get(6)?,
                usd: row.get(7)?,
                reported: row.get(8)?,
                input_tokens: row.get::<_, i64>(9)? as u64,
                output_tokens: row.get::<_, i64>(10)? as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Spend on each of the `days` calendar days up to the one holding
    /// `now_ms`, oldest first, as the day (`YYYY-MM-DD`), when it began
    /// (epoch ms) and its total. Days follow SQLite's `localtime`, the time
    /// zone of this process.
    pub fn spend_by_local_day(&self, now_ms: u64, days: u32) -> Result<Vec<(String, u64, f64)>> {
        if days == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE back(n) AS (
                 SELECT 0 UNION ALL SELECT n + 1 FROM back WHERE n + 1 < ?2
             ),
             day(d, start_s) AS (
                 SELECT date(?1, 'unixepoch', 'localtime', 'start of day', printf('-%d days', n)),
                        CAST(strftime('%s', ?1, 'unixepoch', 'localtime', 'start of day',
                                      printf('-%d days', n), 'utc') AS INTEGER)
                 FROM back
             ),
             spent(d, usd) AS (
                 SELECT date(at_ms / 1000, 'unixepoch', 'localtime'), SUM(usd) FROM ledger
                 WHERE at_ms >= (SELECT MIN(start_s) FROM day) * 1000 GROUP BY 1
             )
             SELECT day.d, day.start_s, COALESCE(spent.usd, 0.0)
             FROM day LEFT JOIN spent USING (d) ORDER BY day.d",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![(now_ms / 1000) as i64, i64::from(days)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u64 * 1000,
                    row.get::<_, f64>(2)?,
                ))
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The calendar day (`YYYY-MM-DD`) holding `ms`, in the time zone of this
    /// process, as `spend_by_local_day` counts days.
    pub fn local_date(&self, ms: u64) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT date(?1, 'unixepoch', 'localtime')",
            [(ms / 1000) as i64],
            |row| row.get(0),
        )?)
    }

    /// Every work ticket that landed (plan tickets are left out), each with
    /// the time of its first landing and the local day of it, oldest first.
    pub fn landings(&self) -> Result<Vec<Landing>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.ticket_id, MIN(e.at_ms),
                    date(MIN(e.at_ms) / 1000, 'unixepoch', 'localtime')
             FROM events e JOIN tickets t ON t.id = e.ticket_id
             WHERE json_extract(e.event, '$.type') = 'landed'
               AND COALESCE(json_extract(t.ticket, '$.kind'), 'work') <> 'plan'
             GROUP BY e.ticket_id ORDER BY MIN(e.at_ms), e.ticket_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Landing {
                ticket_id: TicketId::new(row.get::<_, String>(0)?),
                at_ms: row.get::<_, i64>(1)?.max(0) as u64,
                date: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The latest checks result among each project's tickets, by project: its
    /// newest `checks_passed` or `checks_failed` event.
    pub fn latest_checks(&self) -> Result<Vec<ChecksResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.project, e.ticket_id, e.at_ms, json_extract(e.event, '$.type')
             FROM events e JOIN tickets t ON t.id = e.ticket_id
             WHERE e.seq IN (
                 SELECT MAX(e2.seq) FROM events e2 JOIN tickets t2 ON t2.id = e2.ticket_id
                 WHERE json_extract(e2.event, '$.type') IN ('checks_passed', 'checks_failed')
                 GROUP BY t2.project
             ) ORDER BY t.project",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ChecksResult {
                project: row.get(0)?,
                ticket_id: TicketId::new(row.get::<_, String>(1)?),
                at_ms: row.get::<_, i64>(2)?.max(0) as u64,
                passed: row.get::<_, String>(3)? == "checks_passed",
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn rebuild_projections(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM tickets", [])?;
        let mut stmt = tx.prepare("SELECT seq, ticket_id, event FROM events ORDER BY seq")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut by_ticket: std::collections::BTreeMap<String, Vec<(i64, String)>> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (seq, ticket_id, event_json) = row?;
            by_ticket
                .entry(ticket_id)
                .or_default()
                .push((seq, event_json));
        }
        for (ticket_id, raw_events) in by_ticket {
            let last_seq = raw_events.last().map(|(seq, _)| *seq).unwrap_or(0);
            let events: Vec<Event> = raw_events
                .into_iter()
                .map(|(_, json)| serde_json::from_str(&json).map_err(StoreError::Json))
                .collect::<Result<Vec<_>>>()?;
            let ticket = machine::replay(&events)?;
            if ticket_id != ticket.id.to_string() {
                return Err(StoreError::IdMismatch {
                    expected: ticket_id,
                    got: ticket.id.to_string(),
                });
            }
            upsert_projection(&tx, &ticket, last_seq)?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }
}

fn append_in_tx(
    tx: &Transaction,
    ticket_id: &TicketId,
    at_ms: u64,
    event: &Event,
) -> Result<Ticket> {
    let existing = load_projection(tx, ticket_id)?;
    let ticket = match (existing, event) {
        (Some(_), Event::Created { id, .. }) => {
            return Err(StoreError::DuplicateCreate(id.to_string()));
        }
        (None, Event::Created { id, .. }) => {
            if id != ticket_id {
                return Err(StoreError::IdMismatch {
                    expected: ticket_id.to_string(),
                    got: id.to_string(),
                });
            }
            machine::replay(std::slice::from_ref(event))?
        }
        (None, _) => return Err(StoreError::UnknownTicket(ticket_id.to_string())),
        (Some(ticket), _) => machine::apply(&ticket, event)?,
    };
    let event_json = serde_json::to_string(event)?;
    tx.execute(
        "INSERT INTO events (ticket_id, at_ms, event) VALUES (?1, ?2, ?3)",
        (&ticket_id.to_string(), &(at_ms as i64), &event_json),
    )?;
    let seq = tx.last_insert_rowid();
    upsert_projection(tx, &ticket, seq)?;
    Ok(ticket)
}

fn load_projection(tx: &Transaction, id: &TicketId) -> Result<Option<Ticket>> {
    let mut stmt = tx.prepare("SELECT ticket FROM tickets WHERE id = ?1")?;
    let mut rows = stmt.query([&id.to_string()])?;
    match rows.next()? {
        Some(row) => {
            let json: String = row.get(0)?;
            Ok(Some(serde_json::from_str(&json)?))
        }
        None => Ok(None),
    }
}

fn upsert_projection(tx: &Transaction, ticket: &Ticket, seq: i64) -> Result<()> {
    let ticket_json = serde_json::to_string(ticket)?;
    tx.execute(
        "INSERT OR REPLACE INTO tickets (id, project, state_kind, created_at, ticket, last_seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            &ticket.id.to_string(),
            &ticket.project.to_string(),
            ticket.state.kind(),
            &(ticket.created_at as i64),
            &ticket_json,
            &seq,
        ),
    )?;
    Ok(())
}

fn event_from_row(row: &rusqlite::Row) -> rusqlite::Result<StoredEvent> {
    let event_json: String = row.get(3)?;
    let event = serde_json::from_str(&event_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(StoredEvent {
        seq: row.get(0)?,
        ticket_id: TicketId::new(row.get::<_, String>(1)?),
        at_ms: row.get::<_, i64>(2)? as u64,
        event,
    })
}

fn collect_tickets(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Result<Vec<Ticket>> {
    rows.map(|row| {
        let json: String = row?;
        Ok(serde_json::from_str::<Ticket>(&json)?)
    })
    .collect()
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;
    let version: String = tx
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "0".to_string());
    let version: i64 = version.parse().unwrap_or(0);
    if version < 1 {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ticket_id TEXT NOT NULL,
                at_ms INTEGER NOT NULL,
                event TEXT NOT NULL
            )",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_events_ticket_id ON events(ticket_id)",
            [],
        )?;
        tx.execute(
            "CREATE TABLE IF NOT EXISTS tickets (
                id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                state_kind TEXT NOT NULL,
                ticket TEXT NOT NULL,
                last_seq INTEGER NOT NULL
            )",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_tickets_state_kind ON tickets(state_kind)",
            [],
        )?;
    }
    if version < 2 {
        tx.execute(
            "ALTER TABLE tickets ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        let updates: Vec<(i64, String)> = {
            let mut stmt = tx.prepare("SELECT id, ticket FROM tickets")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.map(|row| {
                let (id, json): (String, String) = row?;
                let ticket: Ticket = serde_json::from_str(&json)?;
                Ok((ticket.created_at as i64, id))
            })
            .collect::<Result<Vec<_>>>()?
        };
        for (created_at, id) in updates {
            tx.execute(
                "UPDATE tickets SET created_at = ?1 WHERE id = ?2",
                (created_at, &id),
            )?;
        }
    }
    if version < 3 {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS ledger (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                at_ms INTEGER NOT NULL,
                ticket_id TEXT NOT NULL,
                project TEXT NOT NULL,
                hire TEXT NOT NULL,
                runtime TEXT NOT NULL,
                model TEXT NOT NULL,
                session TEXT,
                usd REAL NOT NULL,
                reported INTEGER NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL
            )",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_ledger_at ON ledger(at_ms)",
            [],
        )?;
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_ledger_ticket ON ledger(ticket_id)",
            [],
        )?;
    }
    if version < 4 {
        // The board and Live read events by time every second.
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_events_at ON events(at_ms)",
            [],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', '4')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}
