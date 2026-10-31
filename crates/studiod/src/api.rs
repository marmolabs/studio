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
    /// Limits, spend over the last 24 hours, and live reservations.
    Budget,
    /// A quote and an expected finish time for each ticket still open.
    Estimates,
    /// Total spend recorded for one ticket across all its runs.
    TicketSpend {
        ticket: String,
    },
    /// Total spend recorded for every ticket, by ticket id.
    TicketSpends,
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
    /// Replays a rule file's text against the tool calls agents made since
    /// `since` (ms since the epoch), without saving it.
    TestRule {
        text: String,
        #[serde(default)]
        since: u64,
    },
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

fn default_limit() -> usize {
    200
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
