//! The local API: one JSON request per line in, one JSON response per line out,
//! over a Unix socket in the studio's data directory.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use studio_core::ticket::Ticket;

pub use studio_store::{
    KindTotal, SpendKind, SpendLine, StoredEvent, by_kind, dollars, model_name, model_names,
    warning,
};

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
        /// The version this goal is a milestone towards, such as `0.4`.
        #[serde(default)]
        version: Option<String>,
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
    /// Everything the office shows, read at one moment, as an `OfficeView`:
    /// what `Status`, `Tickets`, `Asks`, `RecentEvents` (of `OFFICE_KINDS`,
    /// for the last 24 hours), `TicketSpends`, `MergedTickets` and
    /// `HiresToday` answer, the size of each ready ticket's change, and who
    /// is judging each ticket in review, what each live run is doing and
    /// which ready tickets the owner must review, and each project's
    /// conversation at a glance, the one-line results of finished tasks, each
    /// lead's standup, the checks of each goal ready to ship and how each
    /// project's setup stands. One request instead of eight, for pages that
    /// stay open all day.
    Office,
    /// When each ticket's current active, held, asking or failed state began.
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
    /// Sets a task's scheduling priority; larger values run first.
    SetPriority {
        ticket: String,
        priority: u32,
    },
    /// Replaces a queued or failed task's dependencies.
    SetDependencies {
        ticket: String,
        depends_on: Vec<String>,
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
    /// What runs cost, added up by kind, project, hire and model as
    /// `SpendLine`s, the dearest first. With `tickets`, only what those
    /// tickets cost, over all time; without, everything in the last
    /// `hours` hours (24 when not said).
    SpendBreakdown {
        #[serde(default)]
        tickets: Vec<String>,
        #[serde(default)]
        hours: Option<u64>,
    },
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
    /// `git diff --stat` of the ticket's change: its worktree against its
    /// base; once the work landed and its worktree is gone, its branch
    /// against the commit it started from, while the branch is kept; and for
    /// a goal's plan ticket, the goal's branch against the base the goal
    /// ships to, however many of its tasks landed. Null when git has none to
    /// give.
    Diffstat {
        ticket: String,
    },
    /// `Diffstat` for several tickets at once, by ticket id.
    Diffstats {
        tickets: Vec<String>,
    },
    /// The ticket's full change as a unified diff, cut at 1 MB, from the same
    /// place as `Diffstat`; null when git has none to give.
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
    /// nothing new and work already running carries on. The choice is the file `closed` in the data folder, so it survives
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
    /// with guessed checks, or null when the project had one. With `check`,
    /// everything is validated the same way and the answer is the same, but
    /// nothing is written: `checks` is the file that would be.
    AddProject {
        path: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        branch: Option<String>,
        /// A daily limit of the project's own, in USD.
        #[serde(default)]
        daily_usd: Option<f64>,
        /// A pack whose project template becomes the project's `.studio.toml`.
        #[serde(default)]
        pack: Option<String>,
        /// Only check: answer what would be added, and write nothing.
        #[serde(default)]
        check: bool,
    },
    /// Makes `hire` the lead of `project` (none clears it), written to the
    /// project's entry in `studio.toml` and applied at once.
    SetProjectLead {
        project: String,
        #[serde(default)]
        hire: Option<String>,
    },
    /// Starts releasing `project` as `version`: the final checks run, then
    /// the release waits for the owner's `ApproveRelease` before the
    /// project's `release` commands run and the version is tagged. Answers
    /// the `ReleaseView`.
    StartRelease {
        project: String,
        version: String,
    },
    /// The owner's go-ahead for the release waiting on them in `project`.
    ApproveRelease {
        project: String,
    },
    /// Stops the release under way in `project`, before it publishes.
    CancelRelease {
        project: String,
    },
    /// Every release, newest first, of `project` or of all projects.
    Releases {
        #[serde(default)]
        project: Option<String>,
    },
    /// The owner's message to the lead of `project`: it is stored in the
    /// project's conversation and the lead answers it in a turn of its own,
    /// which `Conversation` shows. A project without a lead is an error that
    /// says to give it one. Answers the stored `TalkMessage`.
    Talk {
        project: String,
        text: String,
    },
    /// The messages of `project`'s conversation, oldest first, and whether
    /// its lead is working on a reply: only those after the message numbered
    /// `after`, when it is given. Answers the `ConversationView`.
    Conversation {
        project: String,
        #[serde(default)]
        after: Option<u64>,
    },
    /// The owner's message to one hire, kept in that hire's persistent,
    /// read-only conversation. Answers the stored `TalkMessage`.
    TalkToHire {
        hire: String,
        text: String,
    },
    /// The messages of one hire's conversation, oldest first, and whether the
    /// hire is working on a reply. Answers the `ConversationView`.
    HireConversation {
        hire: String,
        #[serde(default)]
        after: Option<u64>,
    },
    /// Asks a project's lead to review its work and propose what to do next.
    ReviewOperations {
        project: String,
    },
    /// Approves or rejects one proposed manager action.
    DecideOperation {
        action: String,
        approve: bool,
        #[serde(default)]
        note: Option<String>,
    },
    /// Changes the manager review schedule and how much it may do itself.
    SetOperations {
        #[serde(default)]
        autopilot: Option<bool>,
        #[serde(default)]
        review_minutes: Option<u64>,
        #[serde(default)]
        autonomy: Option<Autonomy>,
        #[serde(default)]
        mode: Option<RuntimeMode>,
    },
    /// Posts an owner message or announcement in the company or a project room.
    TalkToRoom {
        room: String,
        text: String,
    },
    /// Reads a room's persistent messages and whether a mentioned hire is replying.
    Room {
        room: String,
        #[serde(default)]
        after: Option<u64>,
        /// Read messages before this id for history pagination.
        #[serde(default)]
        before: Option<u64>,
        /// Maximum messages in a page; the backend applies a bounded default.
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Tells a task's agent something: a running task hears it in its live
    /// session, one that waits in checks, review or ready goes back with it as
    /// a note, a queued one keeps it for its next run, and a finished one is an
    /// error. Answers `{"task", "how"}`, where `how` is `live`, `sent_back` or
    /// `queued`.
    Tell {
        task: String,
        text: String,
    },
    /// What the lead of `project` does to one of its tasks while it talks with
    /// the owner (the `send_back`, `pause`, `resume`, `cancel` and `tell`
    /// tools): the same as the owner's `RequestChanges`, `Pause`, `Resume`,
    /// `Cancel` and `Tell`, except that the task must be one of `project`'s
    /// own (any other is refused: "GARD-3 isn't one of shop's tasks.") and a
    /// message reaches the agent as coming from the lead, by name. Answers
    /// what the owner's request answers.
    LeadActs {
        project: String,
        task: String,
        act: LeadAct,
    },
    /// Adds the git repository at `path` as a project, as `AddProject` does,
    /// and has its lead set it up: the lead reads the repository and proposes
    /// how the project runs, and the studio tries what it proposes. The lead is
    /// the hire `lead`, or a new hire made from `new_lead`; one of them must be
    /// given. Answers the `SetupView`, which `SetupStatus` keeps answering as
    /// the setup goes on.
    SetupProject {
        path: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        lead: Option<String>,
        #[serde(default)]
        new_lead: Option<NewLead>,
    },
    /// How the setup of `project` stands. Answers the `SetupView`.
    SetupStatus {
        project: String,
    },
    /// The owner's go-ahead for what the lead proposed to set `project` up:
    /// `choice` is written to the project's `.studio.toml` and the daily limit
    /// to `studio.toml`, and with `plan_first` the lead starts the project's
    /// first goal. Answers the `SetupView`, now accepted.
    AcceptSetup {
        project: String,
        choice: SetupChoice,
    },
    /// The lead's proposal for how `project` runs (the `propose_setup` tool):
    /// stored when it is sound, after which the studio tries it. Only a
    /// project whose lead is reading it takes one, and only once. Answers
    /// a line for the lead.
    ProposeSetup {
        project: String,
        proposal: SetupProposal,
    },
}

/// What a lead does to a task of its project (see `Request::LeadActs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum LeadAct {
    /// Sends the task back to its agent with a note on what to change.
    SendBack { note: String },
    /// Stops the task's running agent and holds the task.
    Pause,
    /// Continues a held task.
    Resume,
    /// Cancels the task for good.
    Cancel,
    /// Tells the task's agent something.
    Tell { text: String },
}

/// One message of a lead or direct-hire conversation, kept as a line of its
/// JSONL file under `<data>/talk/` or `<data>/people/`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TalkMessage {
    /// Counts up from 1 in each conversation.
    pub id: u64,
    /// When it was posted, in epoch milliseconds.
    pub at: u64,
    pub from: TalkFrom,
    pub text: String,
    /// What it shows under its words, if anything.
    #[serde(default)]
    pub card: Option<TalkCard>,
}

/// Who a message is from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "who", rename_all = "snake_case")]
pub enum TalkFrom {
    Owner,
    /// The lead, by hire id.
    Lead {
        hire: String,
    },
    /// The studio itself, such as when it reports what it did for the lead.
    Studio,
}

/// What a message shows under its words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TalkCard {
    /// A goal the lead planned and the tasks it filed for it.
    Plan {
        /// The goal's plan ticket.
        goal: String,
        title: String,
        /// The version the goal is a milestone towards, such as `0.4`.
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        tasks: Vec<PlanTask>,
    },
    /// Something the lead did to a task: `action` is `sent back`, `paused`,
    /// `resumed`, `cancelled` or `told`, and `note` what it said, if
    /// anything.
    Acted {
        task: String,
        title: String,
        action: String,
        note: String,
    },
    /// How a project runs, as its lead proposed it when setting it up; the
    /// project's `SetupView` has the details.
    Setup { project: String },
}

/// A task of a planned goal, as its card lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanTask {
    pub title: String,
    /// The hire it was given to, if it was given to one.
    #[serde(default)]
    pub hire: Option<String>,
}

/// The answer to `Request::Conversation` or `Request::HireConversation`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConversationView {
    /// Oldest first.
    #[serde(default)]
    pub messages: Vec<TalkMessage>,
    /// The lead or hire is working on a reply.
    pub thinking: bool,
}

/// How much a manager may do after one of its reviews.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Every proposed action waits for the owner.
    #[default]
    Supervised,
    /// Feedback and messages may go ahead; other actions wait.
    Balanced,
    /// Ordinary task management may go ahead; cancellation still waits.
    Autonomous,
}

/// Whether new runs use configured models or local zero-cost simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    #[default]
    Live,
    Simulation,
}

/// Persistent controls for the company's manager loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OperationsSettings {
    pub autopilot: bool,
    pub review_minutes: u64,
    pub autonomy: Autonomy,
    pub mode: RuntimeMode,
}

impl Default for OperationsSettings {
    fn default() -> Self {
        Self {
            autopilot: false,
            review_minutes: 15,
            autonomy: Autonomy::Supervised,
            mode: RuntimeMode::Live,
        }
    }
}

/// One review a project lead made of its work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManagerReview {
    pub id: String,
    pub project: String,
    pub lead: String,
    pub at: u64,
    pub summary: String,
    pub usd: f64,
    /// The review gave no answer: the summary says why.
    #[serde(default)]
    pub failed: bool,
}

/// What a manager proposes to do. The same command executes after approval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ManagerAct {
    StartGoal {
        goal: String,
        detail: String,
    },
    CreateTask {
        title: String,
        detail: String,
        #[serde(default)]
        hire: Option<String>,
    },
    SendBack {
        task: String,
        note: String,
    },
    Pause {
        task: String,
    },
    Resume {
        task: String,
    },
    Tell {
        task: String,
        text: String,
    },
    SetPriority {
        task: String,
        priority: u32,
    },
    SetDependencies {
        task: String,
        depends_on: Vec<String>,
    },
    Cancel {
        task: String,
    },
}

impl ManagerAct {
    pub fn task(&self) -> Option<&str> {
        match self {
            Self::StartGoal { .. } | Self::CreateTask { .. } => None,
            Self::SendBack { task, .. }
            | Self::Pause { task }
            | Self::Resume { task }
            | Self::Tell { task, .. }
            | Self::SetPriority { task, .. }
            | Self::SetDependencies { task, .. }
            | Self::Cancel { task } => Some(task),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::StartGoal { .. } => "Start a goal",
            Self::CreateTask { .. } => "Delegate a task",
            Self::SendBack { .. } => "Request changes",
            Self::Pause { .. } => "Pause work",
            Self::Resume { .. } => "Resume work",
            Self::Tell { .. } => "Message an employee",
            Self::SetPriority { .. } => "Change priority",
            Self::SetDependencies { .. } => "Change dependencies",
            Self::Cancel { .. } => "Cancel work",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    #[default]
    Pending,
    /// Claimed by one executor; not another approval's to run.
    Executing,
    Executed,
    Rejected,
    Failed,
}

/// One proposed action and its outcome, kept as an audit record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManagerAction {
    pub id: String,
    pub review: String,
    pub project: String,
    pub lead: String,
    pub title: String,
    pub reason: String,
    pub act: ManagerAct,
    pub state: OperationState,
    pub at: u64,
    #[serde(default)]
    pub resolved_at: Option<u64>,
    /// What came of it; while the action is pending, why it waits when that
    /// is not simply the autonomy level.
    #[serde(default)]
    pub result: String,
    /// Whether the studio went ahead with it on its own, by the autonomy
    /// level, rather than on the owner's approval.
    #[serde(default)]
    pub automatic: bool,
}

/// What every company page needs of the manager loop.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OperationsView {
    pub settings: OperationsSettings,
    /// Projects whose leads are reviewing now.
    #[serde(default)]
    pub reviewing: Vec<String>,
    /// Newest first, capped in the backend.
    #[serde(default)]
    pub reviews: Vec<ManagerReview>,
    /// Newest first, including their outcome.
    #[serde(default)]
    pub actions: Vec<ManagerAction>,
    /// A storage problem that blocks changes and new model runs.
    #[serde(default)]
    pub problem: Option<String>,
}

impl OperationsView {
    pub fn pending(&self) -> usize {
        self.actions
            .iter()
            .filter(|action| action.state == OperationState::Pending)
            .count()
    }
}

/// A room in the company sidebar and its latest activity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub last: Option<TalkMessage>,
    pub count: u64,
    pub thinking: bool,
}

/// One bounded page of a room's persistent history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoomView {
    #[serde(default)]
    pub messages: Vec<TalkMessage>,
    pub thinking: bool,
    pub total: u64,
    pub has_more: bool,
}

/// A hire to make for a project, as its lead.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewLead {
    /// What to call it, such as "Nora".
    pub name: String,
    /// The model it runs on.
    pub model: String,
}

/// What a new lead is told it is.
const LEAD_PROMPT: &str = "You are the lead of a project at a small software studio, and you speak \
for it to its owner. Keep your words plain and short. You read the project's code, plan its work \
and check what comes back; you never change files yourself.";

impl NewLead {
    /// The hire this makes, as its id and the text of its file: a lead that
    /// runs on `model`, with the tool it needs to file the tasks of a plan.
    /// The id is the name in lowercase, with a dash for anything but a letter
    /// or a digit. Refused, in plain words, for a name or a model no hire can
    /// be made of. Whether another hire has the name is for the caller to
    /// say.
    pub fn hire_file(&self) -> Result<(String, String), String> {
        let name = self.name.trim();
        let model = self.model.trim();
        if name.is_empty() || name.chars().count() > 40 || name.contains('\n') {
            return Err("give the lead a short name".to_string());
        }
        if model.is_empty() {
            return Err("pick a model for the lead".to_string());
        }
        let id: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        if id.is_empty() {
            return Err("use letters or digits in the lead's name".to_string());
        }
        let quote = |text: &str| Value::String(text.to_string()).to_string();
        let text = format!(
            "---\nname: {}\nrole: lead\nruntime: claude-code\nmodel: {}\ntools: [file_ticket]\n---\n{LEAD_PROMPT}\n",
            quote(name),
            quote(model),
        );
        Ok((id, text))
    }
}

/// How a project's setup through its lead stands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SetupView {
    pub project: String,
    /// The hire doing the setup.
    #[serde(default)]
    pub lead: Option<String>,
    pub state: SetupState,
    /// What the lead proposed, once it has.
    #[serde(default)]
    pub proposal: Option<SetupProposal>,
    /// Why the setup failed, in a line.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    /// The lead is reading the repository.
    #[default]
    Reading,
    /// The studio is trying what the lead proposed.
    Checking,
    /// Everything proposed has been tried; it waits for the owner's choice.
    Ready,
    /// The owner chose, and the choice is written to the project's files.
    Accepted,
    /// The setup could not go on; the `SetupView`'s `error` says why.
    Failed,
}

/// How the lead proposes a project runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SetupProposal {
    /// What the lead found, in a sentence or two.
    #[serde(default)]
    pub summary: String,
    /// The checks that show a change works.
    #[serde(default)]
    pub verify: Vec<ProposedCommand>,
    /// The command that takes the pictures a change is judged by, if there is
    /// one.
    #[serde(default)]
    pub capture: Option<ProposedCommand>,
    /// The commands that release the project, each a program with its
    /// arguments.
    #[serde(default)]
    pub release: Vec<Vec<String>>,
    /// Where the release commands were found, such as "your CI".
    #[serde(default)]
    pub release_source: String,
    /// Paths the owner is asked about before an agent touches them.
    #[serde(default)]
    pub protected: Vec<ProtectedPath>,
    /// What the project may spend in a day, in USD.
    #[serde(default)]
    pub daily_usd: Option<f64>,
    /// What the lead would plan first.
    #[serde(default)]
    pub first_goal: Option<FirstGoal>,
}

/// A command the lead proposes, and what the studio found when it tried it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposedCommand {
    /// A program with its arguments.
    #[serde(default)]
    pub command: Vec<String>,
    pub why: String,
    /// What happened when the studio ran it in a fresh checkout; none before
    /// it has.
    #[serde(default)]
    pub tried: Option<CommandRun>,
}

/// One try of a proposed command.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandRun {
    pub passed: bool,
    pub seconds: u64,
    /// What it came to in a few words, such as "112 pass".
    pub summary: String,
    /// The last lines of what it printed.
    pub output_tail: String,
}

/// A path the owner is asked about before an agent touches it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProtectedPath {
    /// A glob, such as `migrations/**`.
    pub glob: String,
    pub why: String,
}

/// The first goal a lead offers to plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FirstGoal {
    pub text: String,
    /// The version it is a milestone towards, such as `0.9`.
    #[serde(default)]
    pub version: Option<String>,
    pub why: String,
}

/// What the owner picked from a proposal, written to the project's files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SetupChoice {
    /// The checks to keep, each a program with its arguments.
    #[serde(default)]
    pub verify: Vec<Vec<String>>,
    /// The capture command, if it is kept; empty for none.
    #[serde(default)]
    pub capture: Vec<String>,
    #[serde(default)]
    pub release: Vec<Vec<String>>,
    /// The globs the owner is asked about.
    #[serde(default)]
    pub protected: Vec<String>,
    #[serde(default)]
    pub daily_usd: Option<f64>,
    /// The lead plans its first goal at once.
    pub plan_first: bool,
}

/// What a project's conversation holds, for the office to show without the
/// whole thread.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TalkSummary {
    /// The latest message, if there is one.
    #[serde(default)]
    pub last: Option<TalkMessage>,
    /// The lead is working on a reply.
    pub thinking: bool,
    /// How many messages the conversation has.
    pub count: u64,
}

/// A lead's few words on how its project is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Standup {
    /// When it was written, in epoch milliseconds.
    pub at: u64,
    pub text: String,
}

/// The checks the studio ran on a goal's branch once all its tasks were done.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShipCheck {
    pub state: CheckState,
    /// What the checks came to in a line, such as "112 pass" or the check
    /// that failed.
    pub summary: String,
    /// When the checks last changed state, in epoch milliseconds: when they
    /// began, or when they ended.
    pub at: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    #[default]
    Running,
    Passed,
    Failed,
}

/// A project as the office shows it beyond its tickets: who leads it and
/// how its releases stand.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectView {
    pub name: String,
    /// The project's folder.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub lead: Option<String>,
    /// Whether the project has release commands.
    #[serde(default)]
    pub releasable: bool,
    /// The release under way, if any.
    #[serde(default)]
    pub release: Option<ReleaseView>,
    /// The latest release that published.
    #[serde(default)]
    pub last_release: Option<ReleaseView>,
}

/// One release of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseView {
    pub project: String,
    pub version: String,
    pub state: ReleaseState,
    /// In order: the final checks, the owner's go-ahead, each release
    /// command, and the tag.
    pub steps: Vec<ReleaseStep>,
    pub started_at: u64,
    #[serde(default)]
    pub finished_at: Option<u64>,
    /// Why it failed, in a line.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    /// The final checks are running.
    Checking,
    /// It waits for the owner's go-ahead.
    Waiting,
    /// The release commands are running.
    Publishing,
    Released,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseStep {
    pub name: String,
    pub done: bool,
    #[serde(default)]
    pub at: Option<u64>,
}

/// Where a project's tickets work, under its repository: a tool call's path
/// holds this just before the ticket's own folder.
pub const WORKTREES_DIR: &str = ".studio/worktrees/";

/// The most agents that work at once, one at each desk the office holds.
/// `Request::SetMaxRuns` allows up to this, and a larger `max_runs` in
/// `studio.toml` runs this many.
pub const MAX_RUNS_LIMIT: u64 = studio_config::config::MAX_RUNS_LIMIT;

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
pub(crate) struct Estimate {
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

/// The events the office reads: work starting, asking, going to checks,
/// being sent back, held or ended.
pub const OFFICE_KINDS: [&str; 10] = [
    "claimed",
    "ask_opened",
    "ask_resolved",
    "checks_started",
    "checks_failed",
    "review_failed",
    "held",
    "landed",
    "failed",
    "cancelled",
];

/// The answer to `Request::Office`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfficeView {
    /// As `Request::Status` answers.
    pub status: Value,
    pub tickets: Vec<Ticket>,
    pub asks: Vec<AskView>,
    /// The `OFFICE_KINDS` events of the last 24 hours, oldest first: the
    /// newest `RECENT_EVENTS` of them.
    pub events: Vec<StoredEvent>,
    /// When each ticket's current active, held, asking or failed state began.
    #[serde(default)]
    pub state_since: BTreeMap<String, u64>,
    /// What each ticket has cost, as `Request::TicketSpends` answers.
    pub spend: BTreeMap<String, f64>,
    /// What the last 24 hours cost, by kind of run, project, hire and model, as
    /// `Request::SpendBreakdown` answers.
    #[serde(default)]
    pub kinds: Vec<SpendLine>,
    /// Landed work tickets whose goal branch is merged.
    pub merged: Vec<String>,
    /// Lines added and removed by each ready ticket, by ticket id; one
    /// without a worktree or a change is left out.
    pub sizes: BTreeMap<String, (u64, u64)>,
    /// As `Request::HiresToday` answers, except that what the run logs say
    /// (reviews, critiques, approvals) is at most 30 seconds old.
    pub today: BTreeMap<String, HireDay>,
    /// Who is judging each ticket in review, by ticket id; a review whose
    /// judge hasn't started has none.
    pub judges: BTreeMap<String, JudgeView>,
    /// What each ticket with a run under way (claimed, working, in checks,
    /// in review or asking) is doing, by ticket id, from the end of its run
    /// log. The office's panel for the picked ticket reads its live steps
    /// here.
    #[serde(default)]
    pub runs: BTreeMap<String, RunView>,
    /// Ready tickets that no reviewer on another model read, so they are the
    /// owner's to review.
    #[serde(default)]
    pub unreviewed: Vec<String>,
    /// Every project in `studio.toml` order, with its lead and releases.
    #[serde(default)]
    pub projects: Vec<ProjectView>,
    /// Each project's conversation with its lead at a glance, by project
    /// name; a project nobody has talked about is left out.
    #[serde(default)]
    pub talk: BTreeMap<String, TalkSummary>,
    /// A line on what each finished task came to (done, ready or shipped),
    /// by ticket id: the first sentence of its agent's last words.
    #[serde(default)]
    pub results: BTreeMap<String, String>,
    /// Each project's latest standup from its lead, by project name.
    #[serde(default)]
    pub standups: BTreeMap<String, Standup>,
    /// The checks of each goal whose tasks are all done, by the goal's plan
    /// ticket.
    #[serde(default)]
    pub ship_checks: BTreeMap<String, ShipCheck>,
    /// How each project's setup through its lead stands, by project name; a
    /// project that was never set up that way is left out.
    #[serde(default)]
    pub setups: BTreeMap<String, SetupView>,
    /// The manager review loop and its audit trail.
    #[serde(default)]
    pub operations: OperationsView,
    /// Company and project rooms, in display order.
    #[serde(default)]
    pub rooms: Vec<RoomSummary>,
    /// When this was read, in epoch milliseconds.
    pub now: u64,
}

/// What a ticket's run is doing, from the end of its run log.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    /// Its latest tool call or message.
    #[serde(default)]
    pub latest: Option<StepView>,
    /// Its latest tool calls, oldest first: at most `RUN_CALLS`.
    #[serde(default)]
    pub calls: Vec<StepView>,
    /// The agent's latest words, on one line.
    #[serde(default)]
    pub said: Option<String>,
    /// For a ticket in checks: the project checks started so far, oldest
    /// first.
    #[serde(default)]
    pub checks: Vec<CheckView>,
}

/// One step of a run: a tool call or a message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepView {
    /// The tool's name, or `None` for a message.
    pub tool: Option<String>,
    /// A tool call's input, with each text field cut to 400 characters and
    /// a part over 2000 bytes left out (null). Null for a message.
    #[serde(default)]
    pub input: Value,
    /// A message's words on one line, cut to 400 characters; empty for a
    /// tool call.
    #[serde(default)]
    pub text: String,
    /// When it was logged, in epoch milliseconds.
    pub at: u64,
}

/// A project check a ticket's checks started.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckView {
    /// The command with its arguments.
    pub command: String,
    /// When it started, in epoch milliseconds.
    pub at: u64,
}

/// The most tool calls a `RunView` lists.
pub const RUN_CALLS: usize = 3;

/// Who is judging a ticket in review: its reviewer, or a gate's critic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgeView {
    pub hire: String,
    pub model: String,
    /// The gate, for a critic.
    #[serde(default)]
    pub gate: Option<String>,
    /// When the judge started, in epoch milliseconds.
    pub at: u64,
}

/// The most events `Request::RecentEvents` returns.
pub const RECENT_EVENTS: usize = 2000;

/// The most calls `Request::ApproverCalls` returns.
pub(crate) const APPROVER_CALLS: usize = 500;

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
    /// What it asks the owner, as a plain question such as "Send the launch
    /// email?"; empty when none was written.
    #[serde(default)]
    pub title: String,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn plan() -> TalkCard {
        TalkCard::Plan {
            goal: "SHOP-9".into(),
            title: "Checkout flow".into(),
            version: Some("0.5".into()),
            tasks: vec![PlanTask {
                title: "Add a cart drawer".into(),
                hire: Some("dev".into()),
            }],
        }
    }

    /// What `OfficeView` needs of an older studiod's reply.
    fn office_parts() -> Value {
        json!({
            "status": {}, "tickets": [], "asks": [], "events": [], "spend": {},
            "merged": [], "sizes": {}, "today": {}, "judges": {}, "now": 5,
        })
    }

    #[test]
    fn a_message_says_who_it_is_from_and_what_it_shows() {
        let message = TalkMessage {
            id: 3,
            at: 1_700_000_000_000,
            from: TalkFrom::Lead {
                hire: "nora".into(),
            },
            text: "Planned it.".into(),
            card: Some(plan()),
        };
        let wire = json!({
            "id": 3, "at": 1_700_000_000_000_u64,
            "from": {"who": "lead", "hire": "nora"},
            "text": "Planned it.",
            "card": {
                "type": "plan", "goal": "SHOP-9", "title": "Checkout flow", "version": "0.5",
                "tasks": [{"title": "Add a cart drawer", "hire": "dev"}],
            },
        });
        assert_eq!(serde_json::to_value(&message).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<TalkMessage>(wire).unwrap(),
            message
        );
        // The owner and the studio have no hire to name.
        let from = |from: TalkFrom| serde_json::to_value(from).unwrap();
        assert_eq!(from(TalkFrom::Owner), json!({"who": "owner"}));
        assert_eq!(from(TalkFrom::Studio), json!({"who": "studio"}));
    }

    #[test]
    fn a_card_may_leave_out_what_is_optional() {
        let card = |value| serde_json::from_value::<TalkCard>(value).unwrap();
        assert_eq!(
            card(json!({"type": "plan", "goal": "G-1", "title": "Saves"})),
            TalkCard::Plan {
                goal: "G-1".into(),
                title: "Saves".into(),
                version: None,
                tasks: Vec::new(),
            }
        );
        assert_eq!(
            card(json!({"type": "setup", "project": "garden"})),
            TalkCard::Setup {
                project: "garden".into()
            }
        );
        let acted = json!({
            "type": "acted", "task": "G-2", "title": "Menu art",
            "action": "sent back", "note": "Too many colours.",
        });
        assert!(matches!(card(acted), TalkCard::Acted { action, .. } if action == "sent back"));
        let message: TalkMessage = serde_json::from_value(
            json!({"id": 1, "at": 2, "from": {"who": "owner"}, "text": "Hi"}),
        )
        .unwrap();
        assert_eq!(message.card, None);
    }

    #[test]
    fn the_new_requests_are_named_by_cmd() {
        let wire = |request: Request| serde_json::to_value(request).unwrap();
        assert_eq!(
            wire(Request::Talk {
                project: "shop".into(),
                text: "Hi".into()
            }),
            json!({"cmd": "talk", "project": "shop", "text": "Hi"})
        );
        assert_eq!(
            wire(Request::Conversation {
                project: "shop".into(),
                after: Some(7)
            }),
            json!({"cmd": "conversation", "project": "shop", "after": 7})
        );
        assert_eq!(
            wire(Request::Tell {
                task: "SHOP-3".into(),
                text: "Use three colours.".into()
            }),
            json!({"cmd": "tell", "task": "SHOP-3", "text": "Use three colours."})
        );
        assert_eq!(
            wire(Request::SetupStatus {
                project: "garden".into()
            }),
            json!({"cmd": "setup_status", "project": "garden"})
        );
        // What a lead does to a task of its project names the act by `do`.
        let acts = |act| Request::LeadActs {
            project: "shop".into(),
            task: "SHOP-3".into(),
            act,
        };
        assert_eq!(
            wire(acts(LeadAct::Pause)),
            json!({"cmd": "lead_acts", "project": "shop", "task": "SHOP-3", "act": {"do": "pause"}})
        );
        assert_eq!(
            wire(acts(LeadAct::SendBack {
                note: "Fewer colours.".into()
            })),
            json!({
                "cmd": "lead_acts", "project": "shop", "task": "SHOP-3",
                "act": {"do": "send_back", "note": "Fewer colours."},
            })
        );
        let read: Request = serde_json::from_value(json!({
            "cmd": "lead_acts", "project": "shop", "task": "SHOP-3",
            "act": {"do": "tell", "text": "Use three."},
        }))
        .unwrap();
        assert_eq!(
            read,
            acts(LeadAct::Tell {
                text: "Use three.".into()
            })
        );
    }

    #[test]
    fn what_a_request_may_leave_out_is_none_or_empty() {
        let request = |value| serde_json::from_value::<Request>(value).unwrap();
        assert_eq!(
            request(json!({"cmd": "conversation", "project": "shop"})),
            Request::Conversation {
                project: "shop".into(),
                after: None
            }
        );
        assert_eq!(
            request(json!({"cmd": "setup_project", "path": "~/games/garden"})),
            Request::SetupProject {
                path: "~/games/garden".into(),
                name: None,
                lead: None,
                new_lead: None,
            }
        );
        assert_eq!(
            request(json!({
                "cmd": "setup_project", "path": "~/games/garden",
                "new_lead": {"name": "Mira", "model": "opus"},
            })),
            Request::SetupProject {
                path: "~/games/garden".into(),
                name: None,
                lead: None,
                new_lead: Some(NewLead {
                    name: "Mira".into(),
                    model: "opus".into()
                }),
            }
        );
        assert_eq!(
            request(json!({
                "cmd": "accept_setup", "project": "garden",
                "choice": {"plan_first": true, "verify": [["./test.sh"]]},
            })),
            Request::AcceptSetup {
                project: "garden".into(),
                choice: SetupChoice {
                    verify: vec![vec!["./test.sh".into()]],
                    plan_first: true,
                    ..SetupChoice::default()
                },
            }
        );
        // The lead's proposal needs little to be read: the rest has defaults.
        assert_eq!(
            request(json!({
                "cmd": "propose_setup", "project": "garden",
                "proposal": {"verify": [{"command": ["./test.sh"], "why": "Tests."}]},
            })),
            Request::ProposeSetup {
                project: "garden".into(),
                proposal: SetupProposal {
                    verify: vec![ProposedCommand {
                        command: vec!["./test.sh".into()],
                        why: "Tests.".into(),
                        tried: None,
                    }],
                    ..SetupProposal::default()
                },
            }
        );
    }

    #[test]
    fn a_setup_goes_from_reading_to_accepted() {
        let state = |state: SetupState| serde_json::to_value(state).unwrap();
        assert_eq!(state(SetupState::Reading), json!("reading"));
        assert_eq!(state(SetupState::Checking), json!("checking"));
        assert_eq!(state(SetupState::Ready), json!("ready"));
        assert_eq!(state(SetupState::Accepted), json!("accepted"));
        assert_eq!(state(SetupState::Failed), json!("failed"));
        // A setup starts by reading, with nothing proposed yet.
        let view = SetupView::default();
        assert_eq!(view.state, SetupState::Reading);
        assert!(view.proposal.is_none() && view.lead.is_none() && view.error.is_none());

        let proposal = SetupProposal {
            summary: "A small game with tests.".into(),
            verify: vec![ProposedCommand {
                command: vec!["./test.sh".into()],
                why: "It runs the tests.".into(),
                tried: Some(CommandRun {
                    passed: true,
                    seconds: 12,
                    summary: "112 pass".into(),
                    output_tail: "ok".into(),
                }),
            }],
            release: vec![vec!["./package.sh".into()]],
            release_source: "your CI".into(),
            protected: vec![ProtectedPath {
                glob: "saves/**".into(),
                why: "Players' data.".into(),
            }],
            daily_usd: Some(10.0),
            first_goal: Some(FirstGoal {
                text: "Fix the three crashes".into(),
                version: Some("0.9".into()),
                why: "They are in TODO.md.".into(),
            }),
            ..SetupProposal::default()
        };
        let view = SetupView {
            project: "garden".into(),
            lead: Some("mira".into()),
            state: SetupState::Ready,
            proposal: Some(proposal),
            error: None,
        };
        let wire = serde_json::to_value(&view).unwrap();
        assert_eq!(wire["state"], "ready");
        assert_eq!(
            wire["proposal"]["verify"][0]["tried"]["summary"],
            "112 pass"
        );
        assert_eq!(wire["proposal"]["capture"], json!(null));
        assert_eq!(serde_json::from_value::<SetupView>(wire).unwrap(), view);
    }

    #[test]
    fn a_reply_from_an_older_studiod_still_reads() {
        let office: OfficeView = serde_json::from_value(office_parts()).unwrap();
        assert!(office.talk.is_empty() && office.results.is_empty());
        assert!(office.standups.is_empty() && office.ship_checks.is_empty());
        assert!(office.setups.is_empty());
        let ask: AskView = serde_json::from_value(json!({
            "id": "a1", "ticket": "T-1", "hire": "dev", "kind": "question",
            "tool": "AskUserQuestion", "input": {},
        }))
        .unwrap();
        assert_eq!(ask.title, "");
    }

    #[test]
    fn the_office_carries_what_the_team_pages_show() {
        let mut parts = office_parts();
        parts.as_object_mut().unwrap().extend(
            json!({
                "talk": {"shop": {"last": null, "thinking": true, "count": 4}},
                "results": {"SHOP-2": "Added the cart."},
                "standups": {"shop": {"at": 9, "text": "2 tasks done."}},
                "ship_checks": {"SHOP-1": {"state": "failed", "summary": "lint failed", "at": 8}},
                "setups": {"garden": {"project": "garden", "state": "checking"}},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let office: OfficeView = serde_json::from_value(parts).unwrap();
        assert_eq!(
            office.talk["shop"],
            TalkSummary {
                last: None,
                thinking: true,
                count: 4
            }
        );
        assert_eq!(office.results["SHOP-2"], "Added the cart.");
        assert_eq!(office.standups["shop"].text, "2 tasks done.");
        assert_eq!(office.ship_checks["SHOP-1"].state, CheckState::Failed);
        assert_eq!(office.setups["garden"].state, SetupState::Checking);
    }

    #[test]
    fn a_new_lead_is_a_hire_on_its_model_with_the_tool_to_plan_goals() {
        let new = |name: &str, model: &str| NewLead {
            name: name.into(),
            model: model.into(),
        };
        // The id is the name in lowercase, a dash for what is not a letter or a
        // digit; the file quotes what the owner typed.
        let (id, text) = new(" Mira  Lee ", "opus").hire_file().unwrap();
        assert_eq!(id, "mira--lee");
        assert!(
            text.starts_with(
                "---\nname: \"Mira  Lee\"\nrole: lead\nruntime: claude-code\nmodel: \"opus\"\ntools: [file_ticket]\n---\nYou are the lead of a project"
            ),
            "{text}"
        );
        studio_config::check_hire(&id, &text).unwrap();
        // A name that could end the quote and add a line of its own stays a name.
        let (_, tricky) = new("Mira\": x", "opus").hire_file().unwrap();
        assert!(
            tricky.contains("name: \"Mira\\\": x\"\nrole: lead\n"),
            "{tricky}"
        );
        // What no hire can be made of is said in plain words.
        for (name, model, said) in [
            ("", "opus", "short name"),
            ("   ", "opus", "short name"),
            ("two\nlines", "opus", "short name"),
            (&"x".repeat(41), "opus", "short name"),
            ("日本", "opus", "letters or digits"),
            ("Mira", " ", "model"),
        ] {
            let why = new(name, model).hire_file().unwrap_err();
            assert!(why.contains(said), "{name:?}: {why}");
        }
    }
}
