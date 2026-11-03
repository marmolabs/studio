use clap::{ArgGroup, Args, Parser, Subcommand};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use studio_config::StudioPaths;
use studio_config::draft::{Draft, from_denial, from_feedback};
use studiod::api::{FileKind, Request, call};
use studiod::export;

mod mcp;
mod talk;
mod up;

/// Talk to a running studio (`studiod`).
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Studio folder (default: $STUDIO_HOME, else the platform config dir).
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    /// Print studiod's replies as JSON, for scripts.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter studio.toml, five hires and two rules. Keeps existing files.
    Init,
    /// Manage the projects this studio works on.
    #[command(subcommand)]
    Project(ProjectCommand),
    /// Manage packs: shared hires, rules, skills and project templates.
    #[command(subcommand)]
    Pack(PackCommand),
    /// Work with rules.
    #[command(subcommand)]
    Rule(RuleCommand),
    /// Start studiod in the background if it isn't running, then the web UI.
    Up {
        #[arg(long, default_value_t = 4870)]
        port: u16,
        /// Address for the web UI; your LAN address makes it reachable from
        /// other devices.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Don't open a browser.
        #[arg(long)]
        no_open: bool,
        /// Make a new web UI link; the old one stops working.
        #[arg(long)]
        new_token: bool,
    },
    /// Stop studiod. Running tickets are held until you resume them.
    Down,
    /// What the studio is doing.
    Status,
    /// Close the office: nothing new starts, and work already running carries on.
    Close,
    /// Open the office again after `studio close`.
    Open,
    /// Give a hire a ticket on a project.
    Ticket {
        /// Project name from studio.toml.
        #[arg(short, long)]
        project: String,
        /// Hire name (a file in hires/). Without one, the router picks.
        #[arg(long)]
        hire: Option<String>,
        title: String,
        /// Longer description; tickets with the same goal land on one branch.
        #[arg(short, long, default_value = "")]
        goal: String,
        /// Branch or commit to start from (default: the project's integration branch).
        #[arg(long)]
        base: Option<String>,
        /// The full ask: context, steps, acceptance criteria.
        #[arg(short, long, default_value = "", conflicts_with = "detail_file")]
        detail: String,
        /// Read the detail from a file instead.
        #[arg(long)]
        detail_file: Option<PathBuf>,
    },
    /// Give a producer a goal; it files the tickets that reach it.
    Goal {
        /// Project name from studio.toml.
        #[arg(short, long)]
        project: String,
        /// Hire to plan it (default: the project's lead, else the first with
        /// the producer role).
        #[arg(long)]
        hire: Option<String>,
        /// The version this goal is a milestone towards, such as 0.4.
        #[arg(long)]
        milestone: Option<String>,
        /// A short name for the goal; it also names the goal's branch.
        text: String,
        /// The full brief for the producer.
        #[arg(short, long, default_value = "", conflicts_with = "detail_file")]
        detail: String,
        /// Read the brief from a file instead.
        #[arg(long)]
        detail_file: Option<PathBuf>,
    },
    /// Name the hire who leads a project, or with no hire, clear its lead. A
    /// lead plans the project's goals, assigns its tickets and answers its
    /// team's routine questions.
    Lead {
        /// Project name from studio.toml.
        project: String,
        /// The lead's hire id (default: none, which clears the lead).
        hire: Option<String>,
    },
    /// Talk to the lead of a project. With a message, it is sent and the
    /// lead's reply is shown; without, the last messages are.
    Talk {
        /// Project name from studio.toml.
        project: String,
        /// What to say; the words may be given without quotes.
        text: Vec<String>,
    },
    /// Every ticket and its state.
    Board,
    /// Run the studio-tools MCP server on stdio, for one ticket's agent, for
    /// a lead talking with you about a project, or for a lead setting a
    /// project up: give exactly one of --ticket, --talk and --setup.
    #[command(group(ArgGroup::new("mode").required(true).args(["ticket", "talk", "setup"])))]
    Mcp {
        /// Serve this ticket's agent.
        #[arg(long)]
        ticket: Option<String>,
        /// Serve the lead of this project, in conversation with you.
        #[arg(long)]
        talk: Option<String>,
        /// Serve the lead of this project, setting it up.
        #[arg(long)]
        setup: Option<String>,
        /// studiod's socket; studiod passes it so the server needs no config.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// What's waiting for you.
    Asks,
    /// Approve an ask. For a question, pass `--pick "question=label"`.
    Approve {
        ask: String,
        #[arg(long)]
        pick: Vec<String>,
    },
    /// Deny an ask. Without a message the ticket stops; with `-m`, only that
    /// call is refused and the agent carries on, told your note.
    Deny {
        ask: String,
        #[arg(short, long)]
        message: Option<String>,
        /// Also save a rule drafted from this denial.
        #[arg(long)]
        rule: bool,
    },
    /// A ticket's change against its base, as a unified diff.
    Diff {
        ticket: String,
    },
    /// Merge a ready ticket into its goal branch.
    Land {
        ticket: String,
    },
    /// Merge a landed ticket's goal branch into the branch its work started
    /// from. Your checkout of that branch must have no uncommitted changes.
    Merge {
        ticket: String,
    },
    /// Send a ready ticket back with feedback.
    Changes {
        ticket: String,
        feedback: String,
        /// Also save a guide rule drafted from this feedback.
        #[arg(long)]
        rule: bool,
    },
    Cancel {
        ticket: String,
    },
    /// Queue a failed ticket again.
    Retry {
        ticket: String,
    },
    /// Continue a held ticket.
    Resume {
        ticket: String,
    },
    /// Limits, spend over the last 24 hours, and money reserved by live runs.
    Budget,
    /// A ticket's run log: what the agent said and did.
    Log {
        ticket: String,
    },
    /// Check the studio home, projects and runtimes; works without studiod.
    Doctor,
    /// A ticket as Markdown, for a PR description or the record.
    Export {
        ticket: String,
    },
    /// Remove worktrees and branches that finished tickets left behind.
    Gc,
    /// Release a project: its final checks run, then it waits for your
    /// go-ahead (`studio release approve`), then the project's release
    /// commands run and the version is tagged. Nothing is pushed.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Release(ReleaseArgs),
    /// Releases, newest first.
    Releases {
        /// Only this project's (default: every project's).
        project: Option<String>,
    },
}

#[derive(Args)]
struct ReleaseArgs {
    #[command(subcommand)]
    action: Option<ReleaseAction>,
    /// Project name from studio.toml.
    #[arg(required = true)]
    project: Option<String>,
    /// The version to release, such as 0.4; it is tagged v0.4.
    #[arg(required = true)]
    version: Option<String>,
}

#[derive(Subcommand)]
enum ReleaseAction {
    /// Give the go-ahead to the release that waits for you.
    Approve { project: String },
    /// Stop a release that is checking or waiting for you.
    Cancel { project: String },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Add a git repository as a project.
    Add {
        path: PathBuf,
        /// Defaults to the folder name.
        #[arg(long)]
        name: Option<String>,
        /// Branch work starts from (default: the branch checked out now).
        #[arg(long)]
        branch: Option<String>,
        /// Cap for this project over any rolling 24 hours.
        #[arg(long)]
        daily_usd: Option<f64>,
        /// Pack (by name) whose project.toml becomes the project's .studio.toml
        /// when the repo has none yet.
        #[arg(long)]
        pack: Option<String>,
    },
}

#[derive(Subcommand)]
enum PackCommand {
    /// Add a pack folder (one with a pack.toml) to studio.toml.
    Add { path: PathBuf },
}

#[derive(Subcommand)]
enum RuleCommand {
    /// Show which past tool calls a rule would have acted on.
    Test {
        /// A rule file, or the name of one in the rules folder.
        rule: String,
        /// How far back to look: `<n>d` or `<n>h`.
        #[arg(long, default_value = "7d")]
        since: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let home = cli.home.clone();
    let paths = StudioPaths::resolve(cli.home);
    let result = match cli.command {
        Command::Up {
            port,
            host,
            no_open,
            new_token,
        } => up::up(&paths, home.as_deref(), &host, port, !no_open, new_token),
        Command::Down => up::down(&paths),
        command => run(&paths, command, cli.json),
    };
    if let Err(message) = result {
        eprintln!("studio: {message}");
        std::process::exit(1);
    }
}

fn run(paths: &StudioPaths, command: Command, json: bool) -> Result<(), String> {
    let socket = studiod::api::socket_path(&paths.data_dir);
    let mut draft: Option<Draft> = None;
    let request = match command {
        Command::Log { ticket } => return print_log(paths, &ticket),
        Command::Init => return init(paths),
        Command::Doctor => return doctor(paths),
        Command::Export { ticket } => return export(paths, &ticket),
        Command::Mcp {
            ticket,
            talk,
            setup,
            socket,
        } => {
            let mode = match (ticket, talk, setup) {
                (Some(ticket), None, None) => mcp::Mode::Ticket(ticket),
                (None, Some(project), None) => mcp::Mode::Talk(project),
                (None, None, Some(project)) => mcp::Mode::Setup(project),
                _ => return Err("give exactly one of --ticket, --talk and --setup".into()),
            };
            let socket = socket.unwrap_or_else(|| studiod::api::socket_path(&paths.data_dir));
            return mcp::serve(&socket, &mode);
        }
        Command::Project(ProjectCommand::Add {
            path,
            name,
            branch,
            daily_usd,
            pack,
        }) => return add_project(paths, &path, name, branch, daily_usd, pack),
        Command::Pack(PackCommand::Add { path }) => return add_pack(paths, &path),
        Command::Lead { project, hire } => return set_lead(paths, &project, hire),
        Command::Talk { project, text } => {
            return talk::run(&socket, &project, &text.join(" "), json, now_ms());
        }
        Command::Rule(RuleCommand::Test { rule, since }) => {
            return test_rule(paths, &rule, &since, json);
        }
        Command::Up { .. } | Command::Down => return Ok(()),
        Command::Status => Request::Status,
        Command::Close => Request::SetOpen { open: false },
        Command::Open => Request::SetOpen { open: true },
        Command::Ticket {
            project,
            hire,
            title,
            goal,
            base,
            detail,
            detail_file,
        } => {
            let detail = match detail_file {
                Some(path) => std::fs::read_to_string(&path)
                    .map_err(|e| format!("can't read {}: {e}", path.display()))?,
                None => detail,
            };
            Request::CreateTicket {
                project,
                title,
                goal,
                hire: hire.unwrap_or_default(),
                base,
                detail,
            }
        }
        Command::Goal {
            project,
            hire,
            milestone,
            text,
            detail,
            detail_file,
        } => Request::CreateGoal {
            project,
            text,
            hire,
            detail: match detail_file {
                Some(path) => std::fs::read_to_string(&path)
                    .map_err(|e| format!("can't read {}: {e}", path.display()))?,
                None => detail,
            },
            version: milestone,
        },
        Command::Board => Request::Tickets,
        Command::Asks => Request::Asks,
        Command::Approve { ask, pick } => {
            let answers = pick
                .iter()
                .map(|p| {
                    p.split_once('=')
                        .map(|(q, a)| (q.to_string(), a.to_string()))
                        .ok_or_else(|| format!("--pick expects question=label, got {p}"))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            Request::Decide {
                ask,
                allow: true,
                message: None,
                answers: (!answers.is_empty()).then_some(answers),
            }
        }
        Command::Deny { ask, message, rule } => {
            if rule {
                draft = Some(denial_draft(&socket, &ask, message.as_deref())?);
            }
            Request::Decide {
                ask,
                allow: false,
                message,
                answers: None,
            }
        }
        Command::Diff { ticket } => Request::Diff { ticket },
        Command::Land { ticket } => Request::Land { ticket },
        Command::Merge { ticket } => Request::Merge { ticket },
        Command::Changes {
            ticket,
            feedback,
            rule,
        } => {
            if rule {
                let project = project_of(&socket, &ticket)?;
                draft = Some(from_feedback(&project, &feedback));
            }
            Request::RequestChanges { ticket, feedback }
        }
        Command::Cancel { ticket } => Request::Cancel { ticket },
        Command::Retry { ticket } => Request::Retry { ticket },
        Command::Resume { ticket } => Request::Resume { ticket },
        Command::Budget => Request::Budget,
        Command::Gc => Request::Gc,
        Command::Release(ReleaseArgs {
            action: Some(ReleaseAction::Approve { project }),
            ..
        }) => Request::ApproveRelease { project },
        Command::Release(ReleaseArgs {
            action: Some(ReleaseAction::Cancel { project }),
            ..
        }) => Request::CancelRelease { project },
        Command::Release(ReleaseArgs {
            project: Some(project),
            version: Some(version),
            ..
        }) => Request::StartRelease { project, version },
        Command::Release(_) => {
            return Err("say which project and version: studio release <project> <version>".into());
        }
        Command::Releases { project } => Request::Releases { project },
    };
    let is_gc = matches!(request, Request::Gc);
    let is_board = matches!(request, Request::Tickets);
    let is_asks = matches!(request, Request::Asks);
    let is_budget = matches!(request, Request::Budget);
    let diff_of = match &request {
        Request::Diff { ticket } => Some(ticket.clone()),
        _ => None,
    };
    let is_release = matches!(
        request,
        Request::StartRelease { .. }
            | Request::ApproveRelease { .. }
            | Request::CancelRelease { .. }
    );
    let is_releases = matches!(request, Request::Releases { .. });
    let response = call(&socket, &request)
        .map_err(|e| format!("can't reach studiod at {}: {e}", socket.display()))?;
    if !response.ok {
        return Err(response.error.unwrap_or_else(|| "request failed".into()));
    }
    let saved = draft.map(|draft| save_draft(&socket, draft)).transpose()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&response.data).unwrap_or_default()
        );
    } else if let Some(ticket) = diff_of {
        match response.data.as_str() {
            Some(diff) if !diff.is_empty() => print!("{diff}"),
            shown => println!("{}", nothing_to_show(&socket, &ticket, shown.is_some())),
        }
    } else if is_board {
        // The ETA column is a nicety: without it the board still prints.
        let etas = call(&socket, &Request::Estimates)
            .ok()
            .filter(|r| r.ok)
            .map(|r| r.data)
            .unwrap_or_default();
        print_board(&response.data, &etas, now_ms());
    } else if is_asks {
        print_asks(&response.data);
    } else if is_budget {
        print_budget(&response.data);
    } else if is_gc {
        print_gc(&response.data);
    } else if is_release {
        for line in release_lines(&response.data, now_ms()) {
            println!("{line}");
        }
    } else if is_releases {
        for line in releases_lines(&response.data, now_ms()) {
            println!("{line}");
        }
    } else {
        print_reply(&response.data);
    }
    if let (Some(name), false) = (saved, json) {
        println!("saved rule {name}; edit it in the rules folder or the web UI");
    }
    Ok(())
}

/// Calls studiod and returns the reply's data.
fn fetch(socket: &std::path::Path, request: &Request) -> Result<Value, String> {
    let response = call(socket, request)
        .map_err(|e| format!("can't reach studiod at {}: {e}", socket.display()))?;
    if response.ok {
        Ok(response.data)
    } else {
        Err(response.error.unwrap_or_else(|| "request failed".into()))
    }
}

/// The project a ticket belongs to.
fn project_of(socket: &std::path::Path, ticket: &str) -> Result<String, String> {
    let tickets = fetch(socket, &Request::Tickets)?;
    tickets
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"] == ticket)
        .and_then(|t| t["project"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("no ticket {ticket}"))
}

/// What `studio diff` says when studiod has no change to show for `ticket`,
/// whether it answered with an empty change (`empty`) or with none. The
/// ticket's own record, asked for here, tells what is true of it.
fn nothing_to_show(socket: &std::path::Path, ticket: &str, empty: bool) -> String {
    let tickets = fetch(socket, &Request::Tickets).unwrap_or_default();
    let found = tickets
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"] == ticket);
    no_change(ticket, found, empty)
}

/// Why `id` has no change to show, from what is true of it now: `ticket` is
/// its record, if studiod listed it, and `empty` whether git gave an empty
/// change rather than none. Where studiod reads a change from depends on the
/// ticket: the goal's branch for a goal's plan, the worktree for a task that
/// is not done, and for a task that landed the branch kept after its worktree
/// went, which `studio gc` clears once the goal's branch holds it.
fn no_change(id: &str, ticket: Option<&Value>, empty: bool) -> String {
    let Some(ticket) = ticket else {
        return format!("No change to show for {id}");
    };
    let (yet, why) = if ticket["kind"] == "plan" {
        if empty {
            let base = ticket["base_branch"].as_str().unwrap_or("its base");
            (false, format!("{id}'s goal adds nothing to {base}"))
        } else {
            (true, format!("no task of {id}'s goal has landed"))
        }
    } else {
        match (state_of(ticket).as_str(), empty) {
            ("landed", true) => (false, format!("{id} landed without changing anything")),
            ("landed", false) => (false, format!("{id}'s branch was cleared (studio gc)")),
            ("cancelled", _) => (false, format!("{id} was cancelled")),
            (_, true) => (true, format!("{id} hasn't changed anything")),
            ("queued" | "claimed", false) => (true, format!("{id} hasn't started")),
            _ => (false, format!("{id}'s worktree is gone")),
        }
    };
    let head = if yet {
        "No change yet"
    } else {
        "No change to show"
    };
    format!("{head}: {why}")
}

/// The state of a ticket's record: a plain state is a string, one that
/// carries something (`held`, `asking`, `failed`) an object named by it.
fn state_of(ticket: &Value) -> String {
    match &ticket["state"] {
        Value::String(s) => s.clone(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// A rule drafted from denying `ask`, which must still be waiting.
fn denial_draft(
    socket: &std::path::Path,
    ask: &str,
    message: Option<&str>,
) -> Result<Draft, String> {
    let asks = fetch(socket, &Request::Asks)?;
    let found = asks
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["id"] == ask)
        .ok_or_else(|| format!("no ask {ask} is waiting"))?;
    ruleable(found, ask)?;
    let project = project_of(socket, found["ticket"].as_str().unwrap_or_default())?;
    let tool = found["tool"].as_str().unwrap_or_default();
    Ok(from_denial(
        &project,
        tool,
        &found["input"],
        message.unwrap_or_default(),
    ))
}

/// Only a permission ask can be turned into a rule; a question can't.
fn ruleable(ask: &serde_json::Value, id: &str) -> Result<(), String> {
    if ask["kind"] == "permission" {
        Ok(())
    } else {
        Err(format!(
            "ask {id} is not a permission ask, so it has no rule to draft"
        ))
    }
}

/// The file name for `draft` among `files`, and whether that file already
/// holds the draft. Other text under the name gets `-2`, `-3`... and the
/// name stays within the 64 characters the daemon accepts.
fn draft_name(files: &[serde_json::Value], draft: &Draft) -> Result<(String, bool), String> {
    let mut name = draft.name.clone();
    for n in 2..=100 {
        match files.iter().find(|f| f["name"] == name.as_str()) {
            Some(file) if file["text"] == draft.text.as_str() => return Ok((name, true)),
            Some(_) => {
                let suffix = format!("-{n}");
                let base: String = draft.name.chars().take(64 - suffix.len()).collect();
                name = format!("{}{suffix}", base.trim_end_matches('-'));
            }
            None => return Ok((name, false)),
        }
    }
    Err(format!("too many rules are already named {}", draft.name))
}

/// Saves a draft as a new rule file and returns its name. A file with other
/// text under the same name is never replaced; the draft gets `-2`, `-3`...
fn save_draft(socket: &std::path::Path, draft: Draft) -> Result<String, String> {
    let files = fetch(
        socket,
        &Request::Files {
            kind: FileKind::Rules,
        },
    )?;
    let files = files.as_array().cloned().unwrap_or_default();
    let (name, saved) = draft_name(&files, &draft)?;
    if saved {
        return Ok(name);
    }
    let request = Request::SaveFile {
        kind: FileKind::Rules,
        name: name.clone(),
        text: draft.text,
    };
    fetch(socket, &request)?;
    Ok(name)
}

/// A look-back window of `<n>d` days or `<n>h` hours, like `7d` or `36h`,
/// in milliseconds.
fn parse_since(since: &str) -> Result<u64, String> {
    let bad = || format!("--since expects <n>d or <n>h, got {since}");
    let (n, per_unit) = match since.split_at_checked(since.len().saturating_sub(1)) {
        Some((n, "d")) => (n, 24 * 3_600_000),
        Some((n, "h")) => (n, 3_600_000),
        _ => return Err(bad()),
    };
    let n = n.parse::<u64>().map_err(|_| bad())?;
    n.checked_mul(per_unit).ok_or_else(bad)
}

/// `studio rule test`: what the rule would have done to recent tool calls.
fn test_rule(paths: &StudioPaths, rule: &str, since: &str, json: bool) -> Result<(), String> {
    let window = parse_since(since)?;
    let file = if std::path::Path::new(rule).is_file() {
        PathBuf::from(rule)
    } else {
        paths.config_dir.join("rules").join(format!("{rule}.md"))
    };
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let socket = studiod::api::socket_path(&paths.data_dir);
    let request = Request::TestRule {
        text,
        since: now_ms().saturating_sub(window),
    };
    let data = fetch(&socket, &request)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        );
        return Ok(());
    }
    let hits = data["hits"].as_array().cloned().unwrap_or_default();
    println!(
        "{}: {} of {} tool calls in the last {since} would match",
        data["rule"].as_str().unwrap_or_default(),
        hits.len(),
        data["scanned"].as_u64().unwrap_or(0),
    );
    for hit in &hits {
        let field = |key: &str| hit[key].as_str().unwrap_or_default();
        println!(
            "{}  {}  {}  {}",
            field("ticket"),
            field("level"),
            field("tool"),
            field("what").replace('\n', " ")
        );
    }
    Ok(())
}

/// A one-line answer for the commands that change something, and the next
/// step where there is an obvious one.
fn print_reply(data: &Value) {
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_string();
    if let Some(ask) = data["ask"].as_str() {
        println!("{ask}: done");
    } else if data["into"].is_string() {
        let sha: String = text(data, "sha").chars().take(7).collect();
        println!(
            "merged {} into {} ({sha}); nothing was pushed",
            text(data, "branch"),
            text(data, "into")
        );
        let open: Vec<&str> = data["open"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !open.is_empty() {
            println!("still open on this goal: {}", open.join(", "));
        }
    } else if data["branch"].is_string() {
        let ticket = &data["ticket"];
        let sha: String = text(data, "sha").chars().take(7).collect();
        let id = text(ticket, "id");
        println!("{id} landed on {} ({sha})", text(data, "branch"));
        println!(
            "merge it into {} with `studio merge {id}`",
            text(ticket, "base_branch")
        );
    } else if data["id"].is_string() && data["state"].is_string() {
        let id = text(data, "id");
        println!("{id} {}: {}", text(data, "state"), text(data, "title"));
        if text(data, "state") == "queued" {
            println!("follow it with `studio log {id}`, or `studio board`");
        }
    } else if data["running"].is_number() {
        for line in status_lines(data) {
            println!("{line}");
        }
    } else if let Some(closed) = data["closed"].as_bool() {
        println!("{}", office_line(closed));
    } else {
        println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
    }
}

fn office_line(closed: bool) -> &'static str {
    if closed {
        "the office is closed: nothing new starts, and running work carries on; `studio open` starts it again"
    } else {
        "the office is open"
    }
}

/// What `studio status` prints, a line each.
fn status_lines(data: &Value) -> Vec<String> {
    let releases: Vec<&str> = data["releases_waiting"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let waiting = data["asks"].as_u64().unwrap_or_default() + releases.len() as u64;
    let mut lines = vec![format!(
        "{} running, {waiting} waiting for you, ${:.2} of ${:.2} spent in 24h",
        data["running"],
        data["spent_24h"].as_f64().unwrap_or_default(),
        data["daily_usd"].as_f64().unwrap_or_default(),
    )];
    for project in releases {
        lines.push(format!(
            "{project}'s release waits for your go-ahead: `studio release approve {project}`"
        ));
    }
    if data["closed"] == true {
        lines.push(office_line(true).to_string());
    }
    let list = |key: &str| {
        data[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    let projects: Vec<String> = data["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| match data["leads"][name].as_str() {
            Some(lead) => format!("{name} (led by {lead})"),
            None => name.to_string(),
        })
        .collect();
    lines.push(format!("projects: {}", projects.join(", ")));
    lines.push(format!("hires: {}", list("hires")));
    if let Some(states) = data["tickets"].as_object().filter(|t| !t.is_empty()) {
        let counts: Vec<String> = states.iter().map(|(s, n)| format!("{n} {s}")).collect();
        lines.push(format!("tickets: {}", counts.join(", ")));
    }
    lines
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// "~12m" for a finish time `now_ms` is short of, "—" when there is none.
fn eta_text(eta_ms: Option<u64>, now_ms: u64) -> String {
    let Some(eta) = eta_ms else {
        return "—".to_string();
    };
    let minutes = eta.saturating_sub(now_ms).div_ceil(60_000).max(1);
    if minutes >= 120 {
        format!("~{}h", minutes.div_ceil(60))
    } else {
        format!("~{minutes}m")
    }
}

/// How a release stands, then its steps, and what to do next.
fn release_lines(release: &Value, now_ms: u64) -> Vec<String> {
    let text = |key: &str| release[key].as_str().unwrap_or_default();
    let (project, version) = (text("project"), text("version"));
    let state = match text("state") {
        "checking" => "running its final checks",
        "waiting" => "waiting for your go-ahead",
        "publishing" => "publishing",
        "released" => "released",
        "failed" => "failed",
        "cancelled" => "cancelled",
        other => other,
    };
    let mut lines = vec![format!("{project} {version}: {state}")];
    for step in release["steps"].as_array().into_iter().flatten() {
        let mark = if step["done"] == true { "x" } else { " " };
        let name = step["name"].as_str().unwrap_or_default();
        lines.push(format!("  [{mark}] {name}"));
    }
    if let Some(error) = release["error"].as_str() {
        lines.push(format!("it failed: {error}"));
    }
    match text("state") {
        "checking" | "waiting" => lines.push(format!(
            "approve it with `studio release approve {project}` once it waits for you; \
             stop it with `studio release cancel {project}`"
        )),
        "released" => lines.push(format!(
            "tagged {}; nothing was pushed",
            if version.starts_with('v') {
                version.to_string()
            } else {
                format!("v{version}")
            }
        )),
        _ => {}
    }
    if let Some(at) = release["started_at"].as_u64() {
        lines.push(format!("started {}", ago(at, now_ms)));
    }
    lines
}

/// One line per release.
fn releases_lines(releases: &Value, now_ms: u64) -> Vec<String> {
    let all = releases.as_array().cloned().unwrap_or_default();
    if all.is_empty() {
        return vec![
            "No releases yet. Start one with `studio release <project> <version>`.".into(),
        ];
    }
    all.iter()
        .map(|r| {
            let text = |key: &str| r[key].as_str().unwrap_or_default();
            let when = r["started_at"].as_u64().map(|at| ago(at, now_ms));
            format!(
                "{} {}  {}  {}",
                text("project"),
                text("version"),
                text("state"),
                when.unwrap_or_default()
            )
        })
        .collect()
}

/// "5m ago" for a time `now_ms` is past.
fn ago(at_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(at_ms) / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1..=59 => format!("{minutes}m ago"),
        60..=1439 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

fn print_board(tickets: &Value, etas: &Value, now_ms: u64) {
    let tickets = tickets.as_array().cloned().unwrap_or_default();
    if tickets.is_empty() {
        println!("No tickets yet. Create one with `studio ticket`.");
    }
    for t in tickets {
        let state = state_of(&t);
        let mut title = t["title"].as_str().unwrap_or_default().to_string();
        if t["kind"] == "plan" {
            match t["version"].as_str() {
                Some(version) => title.push_str(&format!(" (plan for {version})")),
                None => title.push_str(" (plan)"),
            }
        }
        if let Some(parent) = t["parent"].as_str() {
            title.push_str(&format!(" (from {parent})"));
        }
        let eta = etas
            .as_array()
            .and_then(|all| all.iter().find(|e| e["ticket"] == t["id"]))
            .and_then(|e| e["eta_ms"].as_u64());
        println!(
            "{:<10} {:<10} {:<10} {:<6} {}",
            t["id"].as_str().unwrap_or_default(),
            state,
            t["hire"].as_str().unwrap_or_default(),
            eta_text(eta, now_ms),
            title
        );
    }
}

fn print_budget(b: &Value) {
    let money = |v: &Value| match v.as_f64() {
        Some(n) if n.abs() < 1.0 => format!("${n:.4}"),
        Some(n) => format!("${n:.2}"),
        None => "-".to_string(),
    };
    let limits = &b["limits"];
    println!(
        "Last 24h: {} of {} ({} runs)",
        money(&b["spent_24h"]["total"]),
        money(&limits["daily_usd"]),
        b["runs_24h"]
    );
    println!(
        "Per ticket {}, per run {}, smallest run {}",
        money(&limits["ticket_usd"]),
        money(&limits["run_usd"]),
        money(&limits["min_run_usd"])
    );
    for (kind, key) in [("project", "by_project"), ("hire", "by_hire")] {
        if let Some(map) = b["spent_24h"][key].as_object() {
            for (name, usd) in map {
                let limit = &limits[format!("{kind}s")][name];
                println!("  {kind} {name}: {} of {}", money(usd), money(limit));
            }
        }
    }
    if let Some(reserved) = b["reserved"].as_object().filter(|r| !r.is_empty()) {
        for (ticket, usd) in reserved {
            println!("  reserved for {ticket}: {}", money(usd));
        }
    }
}

fn print_asks(asks: &Value) {
    let asks = asks.as_array().cloned().unwrap_or_default();
    if asks.is_empty() {
        println!("Nothing is waiting for you.");
    }
    for a in asks {
        println!(
            "{}  {} ({}) wants {}: {}",
            a["id"].as_str().unwrap_or_default(),
            a["hire"].as_str().unwrap_or_default(),
            a["ticket"].as_str().unwrap_or_default(),
            a["tool"].as_str().unwrap_or_default(),
            a["input"]
        );
        for reason in a["reasons"].as_array().into_iter().flatten() {
            println!("    because {}", reason.as_str().unwrap_or_default());
        }
    }
}

fn print_log(paths: &StudioPaths, ticket: &str) -> Result<(), String> {
    let path = paths.data_dir.join("runs").join(format!("{ticket}.jsonl"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(note) = entry["note"].as_str() {
            println!("· {note}");
            continue;
        }
        let e = &entry["event"];
        match e["type"].as_str().unwrap_or_default() {
            "text" => println!("{}", e["text"].as_str().unwrap_or_default()),
            "tool_call" => println!(
                "> {} {}",
                e["name"].as_str().unwrap_or_default(),
                e["input"]
            ),
            "tool_result" if e["is_error"] == true => {
                println!("  ! {}", e["output"].as_str().unwrap_or_default())
            }
            "permission" => println!("? asks to use {}", e["tool"].as_str().unwrap_or_default()),
            "finished" => println!(
                "= finished: {}",
                e["outcome"]["outcome"].as_str().unwrap_or_default()
            ),
            _ => {}
        }
    }
    Ok(())
}

fn print_gc(removed: &Value) {
    let lines = removed.as_array().cloned().unwrap_or_default();
    if lines.is_empty() {
        println!("Nothing to remove.");
    }
    for line in lines {
        println!("{}", line.as_str().unwrap_or_default());
    }
}

fn doctor(paths: &StudioPaths) -> Result<(), String> {
    let mut checks = studiod::doctor::check_home(paths);
    checks.push(studiod::doctor::check_daemon(paths));
    let mut broken = 0;
    for check in &checks {
        if check.ok {
            println!("✓ {}", check.label);
        } else {
            broken += 1;
            println!("✗ {}\n    fix: {}", check.label, check.fix);
        }
    }
    match broken {
        0 => Ok(()),
        n => Err(format!("{n} check(s) failed")),
    }
}

fn export(paths: &StudioPaths, ticket: &str) -> Result<(), String> {
    let socket = studiod::api::socket_path(&paths.data_dir);
    let ask = |request: Request| -> Result<Value, String> {
        let response = call(&socket, &request)
            .map_err(|e| format!("can't reach studiod at {}: {e}", socket.display()))?;
        if response.ok {
            Ok(response.data)
        } else {
            Err(response.error.unwrap_or_else(|| "request failed".into()))
        }
    };
    let tickets = ask(Request::Tickets)?;
    let found = tickets
        .as_array()
        .and_then(|all| all.iter().find(|t| t["id"] == ticket))
        .ok_or_else(|| format!("no ticket {ticket}"))?;
    let spend = ask(Request::TicketSpend {
        ticket: ticket.to_string(),
    })?;
    let events = ask(Request::Events {
        since: 0,
        limit: 10_000,
        ticket: Some(ticket.to_string()),
    })?;
    let diffstat = ask(Request::Diffstat {
        ticket: ticket.to_string(),
    })
    .ok();
    let evidence = ask(Request::Evidence {
        ticket: ticket.to_string(),
    })
    .unwrap_or_default();
    let log = paths.data_dir.join("runs").join(format!("{ticket}.jsonl"));
    let summary = std::fs::read_to_string(log)
        .ok()
        .and_then(|text| export::summary(&text));
    print!(
        "{}",
        export::render(&export::Export {
            ticket: found,
            spend_usd: spend.as_f64().unwrap_or(0.0),
            events: &events,
            summary: summary.as_deref(),
            diffstat: diffstat.as_ref().and_then(Value::as_str),
            evidence: &evidence,
        })
    );
    Ok(())
}

fn init(paths: &StudioPaths) -> Result<(), String> {
    let created = studio_config::init(paths).map_err(|e| e.to_string())?;
    if created.is_empty() {
        println!("{} is already set up", paths.config_dir.display());
    }
    for path in created {
        println!("wrote {}", path.display());
    }
    println!("next: studio project add <path-to-a-git-repo>");
    println!("then: studio up");
    Ok(())
}

fn add_pack(paths: &StudioPaths, path: &std::path::Path) -> Result<(), String> {
    let added = studio_config::add_pack(paths, path).map_err(|e| e.to_string())?;
    println!("added pack {} ({})", added.name, added.path.display());
    println!("a running studiod picks it up within a second");
    Ok(())
}

/// `studio lead`: names or clears a project's lead.
fn set_lead(paths: &StudioPaths, project: &str, hire: Option<String>) -> Result<(), String> {
    match put_project_lead(paths, project, hire)? {
        Some(lead) => println!("{project} is led by {lead}"),
        None => println!("{project} has no lead"),
    }
    Ok(())
}

/// Writes the project's lead into studio.toml and returns it. A running
/// studiod is studio.toml's only writer, so it is asked to; without one,
/// this checks the project and the hire and writes the file itself.
fn put_project_lead(
    paths: &StudioPaths,
    project: &str,
    hire: Option<String>,
) -> Result<Option<String>, String> {
    let socket = studiod::api::socket_path(&paths.data_dir);
    let request = Request::SetProjectLead {
        project: project.to_string(),
        hire: hire.clone(),
    };
    match call(&socket, &request) {
        Ok(response) if response.ok => {
            return Ok(response.data["lead"].as_str().map(str::to_string));
        }
        Ok(response) => {
            return Err(response.error.unwrap_or_else(|| "request failed".into()));
        }
        Err(e) if is_not_running(&e) => {}
        Err(e) => return Err(format!("can't reach studiod at {}: {e}", socket.display())),
    }
    let config = studio_config::StudioConfig::load(paths).map_err(|e| e.to_string())?;
    if !config.projects.iter().any(|p| p.name == project) {
        return Err(format!("no project named {project} in studio.toml"));
    }
    let hire = hire.map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    if let Some(hire) = &hire {
        let known = studio_config::load_pack_hires(&config)
            .into_iter()
            .chain(studio_config::load_hires(paths))
            .any(|(_, found)| found.is_ok_and(|found| found.id.0 == *hire));
        if !known {
            return Err(format!("no hire named {hire}"));
        }
    }
    studio_config::set_project_lead(paths, project, hire.as_deref()).map_err(|e| e.to_string())?;
    Ok(hire)
}

fn add_project(
    paths: &StudioPaths,
    path: &std::path::Path,
    name: Option<String>,
    branch: Option<String>,
    daily_usd: Option<f64>,
    pack: Option<String>,
) -> Result<(), String> {
    let (added, branch) = put_project_in_studio(paths, path, name, branch, daily_usd, pack)?;
    println!(
        "added {} ({}, work starts from {branch})",
        added.name,
        added.path.display()
    );
    if let Some(checks) = added.created_checks {
        println!("wrote {}: check its verify commands", checks.display());
    }
    println!("next: `studio up`, then give it a goal or a ticket in the office");
    Ok(())
}

/// Adds the project to studio.toml and returns it with the branch work starts
/// from. A running studiod is studio.toml's only writer, so it is asked to.
fn put_project_in_studio(
    paths: &StudioPaths,
    path: &std::path::Path,
    name: Option<String>,
    branch: Option<String>,
    daily_usd: Option<f64>,
    pack: Option<String>,
) -> Result<(studio_config::AddedProject, String), String> {
    let repo = studio_git::Repo::open(path).map_err(|e| e.to_string())?;
    let branch = match branch {
        Some(branch) => branch,
        None => repo
            .current_branch()
            .map_err(|e| e.to_string())?
            .ok_or("HEAD is detached; pass --branch")?,
    };
    let socket = studiod::api::socket_path(&paths.data_dir);
    let request = Request::AddProject {
        path: repo.root().to_string_lossy().into_owned(),
        name: name.clone(),
        branch: Some(branch.clone()),
        daily_usd,
        pack: pack.clone(),
        check: false,
    };
    match call(&socket, &request) {
        Ok(response) if response.ok => {
            let data = response.data;
            let added = studio_config::AddedProject {
                name: data["name"].as_str().unwrap_or_default().to_string(),
                path: PathBuf::from(data["path"].as_str().unwrap_or_default()),
                created_checks: data["checks"].as_str().map(PathBuf::from),
            };
            return Ok((added, branch));
        }
        Ok(response) => {
            return Err(response.error.unwrap_or_else(|| "request failed".into()));
        }
        // No studiod is listening: nothing else writes studio.toml.
        Err(e) if is_not_running(&e) => {}
        Err(e) => return Err(format!("can't reach studiod at {}: {e}", socket.display())),
    }
    let added = studio_config::add_project(
        paths,
        studio_config::NewProject {
            path: repo.root(),
            name: name.as_deref(),
            integration_branch: Some(&branch),
            daily_usd,
            pack: pack.as_deref(),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((added, branch))
}

/// Whether a failure to call studiod means no studiod is running.
fn is_not_running(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        Command, Draft, draft_name, eta_text, no_change, nothing_to_show, parse_since,
        put_project_in_studio, put_project_lead, release_lines, releases_lines, ruleable, run,
        status_lines,
    };
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use studio_config::StudioPaths;
    use studiod::api::{Request, Response, socket_path};

    /// A studio home in the temp folder, removed when dropped.
    struct Home {
        dir: PathBuf,
        paths: StudioPaths,
    }

    impl Home {
        fn new(label: &str) -> Home {
            let dir =
                std::env::temp_dir().join(format!("studio-cli-{}-{label}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let paths = StudioPaths::resolve(Some(dir.join("home")));
            std::fs::create_dir_all(&paths.config_dir).unwrap();
            std::fs::create_dir_all(&paths.data_dir).unwrap();
            Home { dir, paths }
        }

        fn studio_toml(&self) -> PathBuf {
            self.paths.config_dir.join("studio.toml")
        }

        /// A git repository with one branch, `master`.
        fn repo(&self) -> PathBuf {
            let repo = self.dir.join("game");
            std::fs::create_dir_all(&repo).unwrap();
            let git = std::process::Command::new("git")
                .args(["init", "-q", "-b", "master"])
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(git.success());
            repo
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A studiod that answers its requests with `replies`, one each in turn,
    /// and hands the requests it got to the test.
    fn fake_studiod_all(
        home: &Home,
        replies: Vec<Response>,
    ) -> std::thread::JoinHandle<Vec<Value>> {
        let listener = UnixListener::bind(socket_path(&home.paths.data_dir)).unwrap();
        std::thread::spawn(move || {
            replies
                .into_iter()
                .map(|reply| {
                    let (stream, _) = listener.accept().unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    let mut writer = &stream;
                    writeln!(writer, "{}", serde_json::to_string(&reply).unwrap()).unwrap();
                    serde_json::from_str(&line).unwrap()
                })
                .collect()
        })
    }

    /// A studiod that answers one request with `reply` and hands the request
    /// it got to the test.
    fn fake_studiod(home: &Home, reply: Response) -> std::thread::JoinHandle<Value> {
        let all = fake_studiod_all(home, vec![reply]);
        std::thread::spawn(move || all.join().unwrap().remove(0))
    }

    fn sent(request: Request) -> Value {
        serde_json::to_value(request).unwrap()
    }

    #[test]
    fn close_and_open_send_set_open() {
        for (command, open) in [(Command::Close, false), (Command::Open, true)] {
            let home = Home::new(&format!("office-{open}"));
            let studiod = fake_studiod(&home, Response::ok(json!({ "closed": !open })));
            run(&home.paths, command, false).unwrap();
            assert_eq!(studiod.join().unwrap(), sent(Request::SetOpen { open }));
        }
    }

    #[test]
    fn release_commands_send_their_requests() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            let args = ["studio", "release"]
                .into_iter()
                .chain(args.iter().copied());
            super::Cli::try_parse_from(args).map(|cli| cli.command)
        };
        let expected = [
            (
                vec!["game", "0.4"],
                Request::StartRelease {
                    project: "game".into(),
                    version: "0.4".into(),
                },
            ),
            (
                vec!["approve", "game"],
                Request::ApproveRelease {
                    project: "game".into(),
                },
            ),
            (
                vec!["cancel", "game"],
                Request::CancelRelease {
                    project: "game".into(),
                },
            ),
        ];
        for (n, (args, request)) in expected.into_iter().enumerate() {
            let home = Home::new(&format!("release-{n}"));
            let studiod = fake_studiod(&home, Response::ok(json!({ "project": "game" })));
            run(&home.paths, parse(&args).unwrap(), false).unwrap();
            assert_eq!(studiod.join().unwrap(), sent(request));
        }
        assert!(parse(&["game"]).is_err());
        assert!(parse(&[]).is_err());

        let home = Home::new("releases-list");
        let studiod = fake_studiod(&home, Response::ok(json!([])));
        let list = super::Cli::try_parse_from(["studio", "releases", "game"])
            .unwrap()
            .command;
        run(&home.paths, list, false).unwrap();
        assert_eq!(
            studiod.join().unwrap(),
            sent(Request::Releases {
                project: Some("game".into())
            })
        );
    }

    #[test]
    fn the_mcp_server_serves_exactly_one_of_a_ticket_a_talk_and_a_setup() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            let args = ["studio", "mcp"].into_iter().chain(args.iter().copied());
            super::Cli::try_parse_from(args).map(|cli| cli.command)
        };
        let served = |args: &[&str]| match parse(args).unwrap() {
            Command::Mcp {
                ticket,
                talk,
                setup,
                socket,
            } => (ticket, talk, setup, socket),
            _ => panic!("not the mcp command"),
        };
        assert_eq!(
            served(&["--ticket", "GAME-1"]),
            (Some("GAME-1".into()), None, None, None)
        );
        assert_eq!(
            served(&["--talk", "shop", "--socket", "/tmp/s.sock"]),
            (None, Some("shop".into()), None, Some("/tmp/s.sock".into()))
        );
        assert_eq!(
            served(&["--setup", "garden"]),
            (None, None, Some("garden".into()), None)
        );
        assert!(parse(&[]).is_err());
        assert!(parse(&["--socket", "/tmp/s.sock"]).is_err());
        assert!(parse(&["--ticket", "GAME-1", "--talk", "shop"]).is_err());
        assert!(parse(&["--talk", "shop", "--setup", "garden"]).is_err());
    }

    #[test]
    fn a_release_is_told_step_by_step() {
        let waiting = json!({
            "project": "game", "version": "0.4", "state": "waiting",
            "started_at": 0,
            "steps": [
                { "name": "Final checks", "done": true },
                { "name": "Your go-ahead", "done": false },
                { "name": "Tag v0.4", "done": false },
            ],
        });
        let lines = release_lines(&waiting, 5 * 60_000);
        assert_eq!(lines[0], "game 0.4: waiting for your go-ahead");
        assert_eq!(lines[1], "  [x] Final checks");
        assert_eq!(lines[2], "  [ ] Your go-ahead");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("studio release approve game"))
        );
        assert_eq!(lines.last().unwrap(), "started 5m ago");

        let failed = json!({
            "project": "game", "version": "0.4", "state": "failed",
            "steps": [], "error": "`make` failed:\nno rule",
        });
        let lines = release_lines(&failed, 0);
        assert_eq!(lines[0], "game 0.4: failed");
        assert!(lines.iter().any(|l| l.starts_with("it failed: `make`")));

        let done =
            json!({ "project": "game", "version": "v1.0", "state": "released", "steps": [] });
        assert!(release_lines(&done, 0).contains(&"tagged v1.0; nothing was pushed".to_string()));
        assert_eq!(releases_lines(&json!([]), 0).len(), 1);
        let listed = releases_lines(&json!([waiting]), 3_600_000);
        assert_eq!(listed, ["game 0.4  waiting  1h ago"]);
    }

    #[test]
    fn status_says_when_the_office_is_closed() {
        let status = |closed: bool| {
            status_lines(&json!({
                "running": 1, "asks": 0, "spent_24h": 0.5, "daily_usd": 5.0,
                "projects": ["game"], "hires": ["dev"], "closed": closed,
            }))
        };
        assert!(status(true).iter().any(|line| line.contains("closed")));
        assert!(status(false).iter().all(|line| !line.contains("closed")));
    }

    #[test]
    fn project_add_goes_through_a_running_studiod() {
        // No studio.toml exists, so a write from here would fail.
        let home = Home::new("add-through");
        let repo = home.repo();
        let root = studio_git::Repo::open(&repo).unwrap().root().to_path_buf();
        let studiod = fake_studiod(
            &home,
            Response::ok(json!({
                "name": "game", "path": root, "branch": "master", "checks": null,
            })),
        );
        let (added, branch) =
            put_project_in_studio(&home.paths, &repo, None, None, None, None).unwrap();
        assert_eq!(
            studiod.join().unwrap(),
            sent(Request::AddProject {
                path: root.to_string_lossy().into_owned(),
                name: None,
                branch: Some("master".into()),
                daily_usd: None,
                pack: None,
                check: false,
            })
        );
        assert_eq!((added.name.as_str(), branch.as_str()), ("game", "master"));
        assert_eq!(added.path, root);
        assert!(added.created_checks.is_none());
        assert!(!home.studio_toml().exists());
    }

    #[test]
    fn project_add_shows_what_studiod_refused() {
        let home = Home::new("add-refused");
        let repo = home.repo();
        let studiod = fake_studiod(&home, Response::err("a project is already named game"));
        let err = put_project_in_studio(&home.paths, &repo, None, None, None, None)
            .err()
            .unwrap();
        studiod.join().unwrap();
        assert_eq!(err, "a project is already named game");
    }

    #[test]
    fn project_add_writes_the_file_itself_when_studiod_is_not_running() {
        let home = Home::new("add-alone");
        let repo = home.repo();
        std::fs::write(
            home.studio_toml(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n",
        )
        .unwrap();
        // A socket file nobody listens on, as a stopped studiod leaves.
        drop(UnixListener::bind(socket_path(&home.paths.data_dir)).unwrap());
        let (added, _) = put_project_in_studio(&home.paths, &repo, None, None, None, None).unwrap();
        assert_eq!(added.name, "game");
        let text = std::fs::read_to_string(home.studio_toml()).unwrap();
        assert!(text.contains("name = \"game\""), "{text}");
    }

    #[test]
    fn a_daily_cap_and_a_pack_go_through_a_running_studiod() {
        let home = Home::new("add-cap");
        let repo = home.repo();
        let root = studio_git::Repo::open(&repo).unwrap().root().to_path_buf();
        let studiod = fake_studiod(
            &home,
            Response::ok(json!({
                "name": "game", "path": root, "branch": "master", "checks": null,
            })),
        );
        put_project_in_studio(
            &home.paths,
            &repo,
            None,
            None,
            Some(2.5),
            Some("art".into()),
        )
        .unwrap();
        let sent = studiod.join().unwrap();
        assert_eq!(
            (&sent["daily_usd"], &sent["pack"]),
            (&json!(2.5), &json!("art"))
        );
        assert!(!home.studio_toml().exists());
    }

    #[test]
    fn a_daily_cap_is_written_here_when_studiod_is_not_running() {
        let home = Home::new("add-cap-alone");
        let repo = home.repo();
        std::fs::write(
            home.studio_toml(),
            "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n",
        )
        .unwrap();
        drop(UnixListener::bind(socket_path(&home.paths.data_dir)).unwrap());
        put_project_in_studio(&home.paths, &repo, None, None, Some(2.5), None).unwrap();
        let text = std::fs::read_to_string(home.studio_toml()).unwrap();
        assert!(text.contains("daily_usd = 2.5"), "{text}");
    }

    #[test]
    fn a_lead_goes_through_a_running_studiod() {
        let home = Home::new("lead-through");
        let studiod = fake_studiod(
            &home,
            Response::ok(json!({ "project": "game", "lead": "boss" })),
        );
        let lead = put_project_lead(&home.paths, "game", Some("boss".into())).unwrap();
        assert_eq!(lead.as_deref(), Some("boss"));
        assert_eq!(
            studiod.join().unwrap(),
            sent(Request::SetProjectLead {
                project: "game".into(),
                hire: Some("boss".into()),
            })
        );
        assert!(!home.studio_toml().exists());
    }

    #[test]
    fn a_lead_shows_what_studiod_refused() {
        let home = Home::new("lead-refused");
        let studiod = fake_studiod(&home, Response::err("no hire named boss"));
        let err = put_project_lead(&home.paths, "game", Some("boss".into()))
            .err()
            .unwrap();
        studiod.join().unwrap();
        assert_eq!(err, "no hire named boss");
    }

    #[test]
    fn a_lead_is_written_here_when_studiod_is_not_running() {
        let home = Home::new("lead-alone");
        std::fs::write(
            home.studio_toml(),
            "# mine\n[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n\n\
             [[projects]]\nname = \"game\"\npath = \"/g\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.paths.config_dir.join("hires")).unwrap();
        std::fs::write(
            home.paths.config_dir.join("hires/boss.md"),
            "---\nname: Boss\nrole: lead\nruntime: claude-code\nmodel: opus\n---\nYou lead.\n",
        )
        .unwrap();
        // No socket file at all, as before the first `studio up`.
        let lead = put_project_lead(&home.paths, "game", Some("boss".into())).unwrap();
        assert_eq!(lead.as_deref(), Some("boss"));
        let text = std::fs::read_to_string(home.studio_toml()).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(
            text.contains("name = \"game\"\nlead = \"boss\"\n"),
            "{text}"
        );

        let err = put_project_lead(&home.paths, "nope", Some("boss".into()));
        assert_eq!(err, Err("no project named nope in studio.toml".into()));
        let err = put_project_lead(&home.paths, "game", Some("ghost".into()));
        assert_eq!(err, Err("no hire named ghost".into()));

        assert_eq!(put_project_lead(&home.paths, "game", None), Ok(None));
        let text = std::fs::read_to_string(home.studio_toml()).unwrap();
        assert!(!text.contains("lead"), "{text}");
    }

    #[test]
    fn status_shows_each_projects_lead() {
        let lines = status_lines(&json!({
            "running": 0, "asks": 0, "spent_24h": 0.0, "daily_usd": 5.0,
            "projects": ["game", "site"], "leads": {"game": "boss"}, "hires": [],
        }));
        assert!(
            lines.contains(&"projects: game (led by boss), site".to_string()),
            "{lines:?}"
        );
    }

    #[test]
    fn a_release_waiting_counts_as_waiting_for_you() {
        let lines = status_lines(&json!({
            "running": 1, "asks": 2, "spent_24h": 0.5, "daily_usd": 5.0,
            "releases_waiting": ["site"], "projects": ["site"], "hires": [],
        }));
        assert_eq!(
            lines[0],
            "1 running, 3 waiting for you, $0.50 of $5.00 spent in 24h"
        );
        assert!(
            lines.contains(
                &"site's release waits for your go-ahead: `studio release approve site`"
                    .to_string()
            ),
            "{lines:?}"
        );
    }

    #[test]
    fn a_diff_with_nothing_to_show_says_what_is_true_of_the_task() {
        fn record(kind: &str, state: Value) -> Value {
            json!({"id": "GARD-3", "kind": kind, "state": state, "base_branch": "main"})
        }
        let work = |state: Value| record("work", state);
        let plan = |state: &str| record("plan", json!(state));
        // What is said of a record when git gave an empty change, or none.
        let says = |ticket: Value, empty: bool| no_change("GARD-3", Some(&ticket), empty);
        let asking = json!({"asking": {"ask": "a1"}});
        let held = json!({"held": {"reason": "paused"}});
        let failed = json!({"failed": {"class": "checks", "detail": "x"}});

        // A task that landed is read from the branch it kept, until `studio
        // gc` clears the branch.
        assert_eq!(
            says(work(json!("landed")), false),
            "No change to show: GARD-3's branch was cleared (studio gc)"
        );
        assert_eq!(
            says(work(json!("landed")), true),
            "No change to show: GARD-3 landed without changing anything"
        );
        // A task that has not begun has no worktree to read yet.
        for state in ["queued", "claimed"] {
            assert_eq!(
                says(work(json!(state)), false),
                "No change yet: GARD-3 hasn't started",
                "{state}"
            );
        }
        // One that has begun and has none has lost it.
        for state in [json!("working"), failed.clone()] {
            assert_eq!(
                says(work(state.clone()), false),
                "No change to show: GARD-3's worktree is gone",
                "{state}"
            );
        }
        // One that has not changed anything yet, wherever it has got to.
        for state in [
            json!("queued"),
            json!("working"),
            asking,
            held,
            json!("checking"),
            json!("reviewing"),
            json!("ready"),
            failed,
        ] {
            assert_eq!(
                says(work(state.clone()), true),
                "No change yet: GARD-3 hasn't changed anything",
                "{state}"
            );
        }
        // A cancelled one, whether or not its worktree is cleared.
        for empty in [false, true] {
            assert_eq!(
                says(work(json!("cancelled")), empty),
                "No change to show: GARD-3 was cancelled"
            );
        }
        // A goal's is its branch against the one it ships to.
        for state in ["working", "landed"] {
            assert_eq!(
                says(plan(state), false),
                "No change yet: no task of GARD-3's goal has landed"
            );
        }
        assert_eq!(
            says(plan("landed"), true),
            "No change to show: GARD-3's goal adds nothing to main"
        );
        // A task studiod does not list.
        assert_eq!(
            no_change("GARD-3", None, false),
            "No change to show for GARD-3"
        );
    }

    #[test]
    fn nothing_to_show_asks_studiod_which_task_it_is() {
        let home = Home::new("nothing-to-show");
        let tickets = json!([
            {"id": "GARD-2", "kind": "work", "state": "queued", "base_branch": "main"},
            {"id": "GARD-3", "kind": "work", "state": "landed", "base_branch": "main"},
        ]);
        let studiod = fake_studiod(&home, Response::ok(tickets));
        let socket = socket_path(&home.paths.data_dir);
        assert_eq!(
            nothing_to_show(&socket, "GARD-3", false),
            "No change to show: GARD-3's branch was cleared (studio gc)"
        );
        assert_eq!(studiod.join().unwrap(), sent(Request::Tickets));

        // With no studiod to ask, it still says there is nothing.
        let home = Home::new("nothing-to-show-alone");
        assert_eq!(
            nothing_to_show(&socket_path(&home.paths.data_dir), "GARD-3", false),
            "No change to show for GARD-3"
        );
    }

    #[test]
    fn studio_diff_asks_for_the_task_only_when_there_is_no_change() {
        let diff = || Command::Diff {
            ticket: "GARD-3".into(),
        };
        let asked = || {
            sent(Request::Diff {
                ticket: "GARD-3".into(),
            })
        };

        let home = Home::new("diff-change");
        let change = "diff --git a/menu.txt b/menu.txt\n+Escape closes it\n";
        let studiod = fake_studiod_all(&home, vec![Response::ok(change)]);
        run(&home.paths, diff(), false).unwrap();
        assert_eq!(studiod.join().unwrap(), [asked()]);

        // A change that is nothing and a change that is not there both lead
        // to asking which task it is.
        for (label, nothing) in [("empty", json!("")), ("none", Value::Null)] {
            let home = Home::new(&format!("diff-{label}"));
            let task = json!([{"id": "GARD-3", "kind": "work", "state": "working"}]);
            let studiod = fake_studiod_all(&home, vec![Response::ok(nothing), Response::ok(task)]);
            run(&home.paths, diff(), false).unwrap();
            assert_eq!(studiod.join().unwrap(), [asked(), sent(Request::Tickets)]);
        }
    }

    fn draft(name: &str, text: &str) -> Draft {
        Draft {
            name: name.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn only_permission_asks_make_rules() {
        assert!(ruleable(&json!({"kind": "permission"}), "ASK-1").is_ok());
        assert!(ruleable(&json!({"kind": "question"}), "ASK-2").is_err());
    }

    #[test]
    fn a_taken_draft_name_gets_a_suffix() {
        let files = [json!({"name": "a", "text": "old"})];
        assert_eq!(
            draft_name(&files, &draft("a", "new")),
            Ok(("a-2".into(), false))
        );
        assert_eq!(draft_name(&[], &draft("a", "new")), Ok(("a".into(), false)));
    }

    #[test]
    fn identical_text_is_already_saved() {
        let files = [
            json!({"name": "a", "text": "old"}),
            json!({"name": "a-2", "text": "new"}),
        ];
        assert_eq!(
            draft_name(&files, &draft("a", "new")),
            Ok(("a-2".into(), true))
        );
    }

    #[test]
    fn a_suffixed_name_stays_within_64_characters() {
        let long = "a".repeat(64);
        let files = [json!({"name": long, "text": "old"})];
        let want = (format!("{}-2", "a".repeat(62)), false);
        assert_eq!(draft_name(&files, &draft(&long, "new")), Ok(want));
    }

    #[test]
    fn a_full_run_of_names_is_an_error() {
        let mut files = vec![json!({"name": "a", "text": "old"})];
        files.extend((2..=99).map(|n| json!({"name": format!("a-{n}"), "text": "old"})));
        assert!(draft_name(&files, &draft("a", "new")).is_err());
    }

    #[test]
    fn since_rejects_a_window_too_big_to_count() {
        assert!(parse_since("999999999999999999d").is_err());
        assert!(parse_since("999999999999999999h").is_err());
    }

    #[test]
    fn since_takes_days_or_hours() {
        assert_eq!(parse_since("7d"), Ok(7 * 24 * 3_600_000));
        assert_eq!(parse_since("36h"), Ok(36 * 3_600_000));
        assert!(parse_since("7").is_err());
        assert!(parse_since("xd").is_err());
        assert!(parse_since("").is_err());
    }

    #[test]
    fn eta_reads_as_minutes_then_hours() {
        assert_eq!(eta_text(None, 0), "—");
        assert_eq!(eta_text(Some(12 * 60_000), 0), "~12m");
        assert_eq!(eta_text(Some(100), 0), "~1m");
        assert_eq!(eta_text(Some(0), 5_000), "~1m");
        assert_eq!(eta_text(Some(3 * 3_600_000), 0), "~3h");
    }
}
