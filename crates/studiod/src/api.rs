//! The local API: one JSON request per line in, one JSON response per line out,
//! over a Unix socket in the studio's data directory.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    CreateTicket {
        project: String,
        title: String,
        #[serde(default)]
        goal: String,
        /// Empty means the router picks.
        #[serde(default)]
        hire: String,
        #[serde(default)]
        base: Option<String>,
        /// What the ticket asks for in full: context, steps, acceptance criteria.
        #[serde(default)]
        detail: String,
    },
    /// A planning ticket for a producer: it files the tickets that reach the goal.
    CreateGoal {
        project: String,
        text: String,
        #[serde(default)]
        hire: Option<String>,
        /// The owner's full brief for the producer.
        #[serde(default)]
        detail: String,
    },
    Tickets,
    Events {
        #[serde(default)]
        since: i64,
        #[serde(default = "default_limit")]
        limit: usize,
        /// Only this ticket's events.
        #[serde(default)]
        ticket: Option<String>,
    },
    /// Every ticket's events at or after `since_ms` (epoch milliseconds),
    /// oldest first: the newest `limit` of them, at most `RECENT_EVENTS`.
    RecentEvents {
        since_ms: u64,
        #[serde(default)]
        limit: Option<usize>,
        /// Only events of these types, such as `"landed"`; empty means all.
        #[serde(default)]
        kinds: Vec<String>,
    },
    /// The whole history, as events oldest first, of every ticket that did
    /// anything at or after `since_ms` or has a run under way. The office
    /// draws its "Last hour" lanes from it.
    Activity {
        since_ms: u64,
    },
    /// When each running ticket's latest run began and each held ticket was
    /// held, by ticket id.
    StateSince,
    Asks,
    Decide {
        ask: String,
        allow: bool,
        #[serde(default)]
        message: Option<String>,
        /// Option label per question text, for questions.
        #[serde(default)]
        answers: Option<std::collections::BTreeMap<String, String>>,
    },
    Land {
        ticket: String,
    },
    /// Merges a landed ticket's goal branch into the branch its work started
    /// from.
    Merge {
        ticket: String,
    },
    /// Ids of the landed work tickets whose goal branch is already merged
    /// into their base branch. A missing branch or a git error is not merged.
    MergedTickets,
    Cancel {
        ticket: String,
    },
    Retry {
        ticket: String,
    },
    RequestChanges {
        ticket: String,
        feedback: String,
    },
    Resume {
        ticket: String,
    },
    /// Stops a running ticket's agent and holds the ticket until `Resume`.
    Pause {
        ticket: String,
    },
    /// Limits (with each hire's own daily and per-ticket caps), spend over the
    /// last 24 hours, and live reservations.
    Budget,
    /// Spend on each of the last `days` calendar days in the time zone of
    /// the machine studiod runs on, as `DaySpend`s, oldest first. The last
    /// is today so far. At most 90 days.
    SpendByDay {
        #[serde(default = "default_days")]
        days: u32,
    },
    /// A quote and an expected finish time for each ticket still open.
    Estimates,
    /// Total spend for one ticket across all its runs, with what a live run
    /// has spent so far by its meter.
    TicketSpend {
        ticket: String,
    },
    /// Total spend for every ticket, by ticket id, counted as `TicketSpend`.
    TicketSpends,
    /// When each ticket last changed (its latest event), in epoch
    /// milliseconds, by ticket id.
    UpdatedAt,
    /// The guidance that applies to a ticket's hire (the `read_rules` tool).
    ReadRules {
        ticket: String,
    },
    /// Lists skills, or returns one skill's SKILL.md (the `load_skill` tool).
    LoadSkill {
        ticket: String,
        #[serde(default)]
        name: Option<String>,
    },
    /// Asks the owner and answers when they do (the `ask_owner` tool).
    AskOwner {
        ticket: String,
        question: String,
        #[serde(default)]
        options: Vec<String>,
    },
    /// Files a ticket under another (the `file_ticket` tool).
    FileTicket {
        ticket: String,
        title: String,
        detail: String,
        /// Missing or empty means the router picks.
        #[serde(default)]
        hire: Option<String>,
    },
    /// `git diff --stat` of the ticket's worktree against its base, or null
    /// when the worktree is gone.
    Diffstat {
        ticket: String,
    },
    /// `Diffstat` for several tickets at once, by ticket id.
    Diffstats {
        tickets: Vec<String>,
    },
    /// The ticket's full change against its base as a unified diff, cut at
    /// 1 MB, or null when the worktree is gone.
    Diff {
        ticket: String,
    },
    /// The images and clips a ticket's capture runs produced, as `EvidenceFile`s.
    Evidence {
        ticket: String,
    },
    /// Removes worktrees and branches of landed and cancelled tickets, and
    /// worktrees no ticket owns. Returns a line for each thing removed.
    Gc,
    /// Every `.md` file in the studio's hires or rules folder.
    Files {
        kind: FileKind,
    },
    /// Validates and writes one file; a hire takes effect without a restart.
    SaveFile {
        kind: FileKind,
        name: String,
        text: String,
    },
    DeleteFile {
        kind: FileKind,
        name: String,
    },
    /// Every rule file the studio loads, as `RuleFile`s: its own, then each
    /// pack's, then each project's.
    RuleFiles,
    /// What each hire did in the last 24 hours, as `HireDay`s by hire id.
    HiresToday,
    /// The approver's calls over the last 24 hours, as `ApproverCall`s,
    /// newest first: at most `limit`, and at most `APPROVER_CALLS`.
    ApproverCalls {
        #[serde(default = "default_calls")]
        limit: usize,
    },
    /// Replays a rule file's text against the tool calls agents made since
    /// `since` (ms since the epoch), without saving it.
    TestRule {
        text: String,
        #[serde(default)]
        since: u64,
    },
    /// Closes or reopens the office. While it is closed the scheduler starts
    /// nothing new (as `max_runs = 0` does) and work already running carries
    /// on. The choice is the file `closed` in the data folder, so it survives
    /// a restart. Answers `{"closed": bool}`; `Status` reports it too.
    SetOpen {
        open: bool,
    },
    /// How many agents may work at once, from 1 to `MAX_RUNS_LIMIT`. Written
    /// to `[concurrency] max_runs` in `studio.toml`, which keeps its other
    /// lines and comments, and applied at once. Answers `{"max_runs": n}`.
    SetMaxRuns {
        max_runs: u64,
    },
    /// The studio's daily and per-ticket spending limits in USD, written to
    /// `[budget] daily_usd` and `ticket_usd` in `studio.toml` and applied at
    /// once. A limit left out stays as it is; each given one must be positive
    /// and finite. Answers `{"daily_usd": x, "ticket_usd": y}`.
    SetLimits {
        #[serde(default)]
        daily_usd: Option<f64>,
        #[serde(default)]
        ticket_usd: Option<f64>,
    },
    /// What the studio has achieved, as `Trophy`s, from its own history.
    Trophies,
    /// Each project's checks, as `ProjectCheck`s in the order of
    /// `studio.toml`.
    ProjectChecks,
    /// Adds the git repository at `path` as a project, as `studio project add`
    /// does: `name` defaults to the folder's name and `branch`, where work
    /// starts, to the branch checked out there. Written to `studio.toml`,
    /// keeping its comments, and applied at once. Answers `{"name", "path",
    /// "branch", "checks"}`, where `checks` is the `.studio.toml` this wrote
    /// with guessed checks, or null when the project had one.
    AddProject {
        path: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        branch: Option<String>,
    },
}

/// The most agents that work at once, one at each desk the office holds.
/// `Request::SetMaxRuns` allows up to this, and a larger `max_runs` in
/// `studio.toml` runs this many.
pub const MAX_RUNS_LIMIT: u64 = 9;

/// Something the studio has achieved. A trophy with `earned_at` is won; one
/// with `progress` is the next milestone, `(done, needed)` of the way there.
/// Every value comes from the studio's history; none is guessed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trophy {
    /// Stable, such as `streak`, `best-day`, `first-goal-merged`, `landed-25`.
    pub id: String,
    /// Plain words for a heading.
    pub title: String,
    pub detail: String,
    /// When it was won, in epoch milliseconds.
    pub earned_at: Option<u64>,
    pub progress: Option<(u64, u64)>,
}

/// One project's checks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectCheck {
    pub project: String,
    /// The commands in the project's `.studio.toml` `verify`, each a program
    /// with its arguments. Empty when the project has none or no such file.
    pub verify: Vec<Vec<String>>,
    /// Tickets of the project running their checks now.
    pub running: Vec<String>,
    /// The newest result among the project's tickets, if any has been.
    pub latest: Option<CheckResult>,
}

/// How a ticket's checks last ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub ticket: String,
    pub passed: bool,
    /// Epoch milliseconds.
    pub at_ms: u64,
}

/// The two studio-level folders that can be edited through the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Hires,
    Rules,
}

impl FileKind {
    pub fn dir(self) -> &'static str {
        match self {
            FileKind::Hires => "hires",
            FileKind::Rules => "rules",
        }
    }
}

/// One file in an editable folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileView {
    pub name: String,
    pub text: String,
    /// Why the file does not load, if it doesn't.
    #[serde(default)]
    pub problem: Option<String>,
}

/// A rule file and where it comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleFile {
    /// `studio`, `pack` or `project`.
    pub origin: String,
    /// The pack's or project's name; empty for the studio's own rules.
    #[serde(default)]
    pub owner: String,
    pub name: String,
    pub path: String,
    pub text: String,
    /// Why the file does not load, if it doesn't.
    #[serde(default)]
    pub problem: Option<String>,
    /// A pack rule turned off in `studio.toml`.
    #[serde(default)]
    pub disabled: bool,
}

/// What a hire did in the last 24 hours, from ticket events and run logs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HireDay {
    /// Its work tickets that passed review.
    pub done: u32,
    /// Its plan tickets that landed, each a goal broken into tickets.
    pub planned: u32,
    /// Changes it reviewed.
    pub reviews: u32,
    /// Changes it judged as a gate's critic.
    pub critiques: u32,
    /// As the approver: tool calls it allowed, denied, or left to the owner.
    pub allowed: u32,
    pub denied: u32,
    pub to_owner: u32,
    /// As the router: new tickets it gave a hire.
    pub routed: u32,
}

/// One tool call the approver decided, from the run logs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApproverCall {
    pub ticket: String,
    /// When it decided, in epoch milliseconds.
    pub at: u64,
    pub tool: String,
    /// The Bash command, the path or the URL, else the input JSON cut
    /// short; empty when the log has no request for the call.
    #[serde(default)]
    pub what: String,
    /// `allowed`, `denied`, or `to_owner` when it left the call to the owner.
    pub verdict: String,
    pub reason: String,
}

/// What a rule would have done to past tool calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleTest {
    /// The rule's kind and level, e.g. `gate, block`.
    pub rule: String,
    /// Tool calls looked at.
    pub scanned: usize,
    pub hits: Vec<RuleHit>,
}

/// One past tool call a rule would have acted on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleHit {
    pub ticket: String,
    /// When the call was made, in epoch milliseconds.
    pub at: u64,
    pub tool: String,
    /// The Bash command or the path, else the input JSON cut short.
    pub what: String,
    /// The finding's level, in snake case.
    pub level: String,
}

/// One image or clip from a ticket's capture run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub attempt: u32,
    pub name: String,
    /// Absolute path of the file in the ticket's worktree.
    pub path: String,
}

/// What history says about one open ticket. Both are `None` without history
/// for its hire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub ticket: String,
    pub quote_usd: Option<f64>,
    /// Expected finish, in epoch milliseconds.
    pub eta_ms: Option<u64>,
}

/// What was spent on one calendar day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaySpend {
    /// The day, as `YYYY-MM-DD`; empty from an older studiod, whose days
    /// were 24-hour windows ending now.
    #[serde(default)]
    pub date: String,
    /// When the day began, in epoch milliseconds.
    pub start_ms: u64,
    pub usd: f64,
}

/// The most events `Request::RecentEvents` returns.
pub const RECENT_EVENTS: usize = 2000;

/// The most calls `Request::ApproverCalls` returns.
pub const APPROVER_CALLS: usize = 500;

fn default_limit() -> usize {
    200
}

fn default_calls() -> usize {
    50
}

fn default_days() -> u32 {
    14
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(data: impl Serialize) -> Response {
        Response {
            ok: true,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
            error: None,
        }
    }

    pub fn err(message: impl ToString) -> Response {
        Response {
            ok: false,
            data: Value::Null,
            error: Some(message.to_string()),
        }
    }
}

/// Something waiting for the owner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskView {
    pub id: String,
    pub ticket: String,
    pub hire: String,
    /// `permission`, `question`, `change` or `gate`.
    pub kind: String,
    pub tool: String,
    pub input: Value,
    /// The rules that sent this to the owner, if any.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// When it was asked, in epoch milliseconds; 0 from an older studiod.
    #[serde(default)]
    pub at_ms: u64,
}

/// Where the daemon listens: `<data>/studiod.sock`, or a short path in `/tmp`
/// when that would exceed the OS limit on socket paths (about 104 bytes).
pub fn socket_path(data_dir: &Path) -> std::path::PathBuf {
    let preferred = data_dir.join("studiod.sock");
    if preferred.as_os_str().len() < 100 {
        return preferred;
    }
    // FNV-1a of the data dir, so each studio home gets its own socket.
    let hash = data_dir
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    std::path::PathBuf::from(format!("/tmp/studiod-{hash:016x}.sock"))
}

/// Sends one request to a running daemon and waits for its answer.
pub fn call(socket: &Path, request: &Request) -> std::io::Result<Response> {
    let mut stream = UnixStream::connect(socket)?;
    let mut line = serde_json::to_string(request).map_err(std::io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    serde_json::from_str(&reply).map_err(std::io::Error::other)
}
