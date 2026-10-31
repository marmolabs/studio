use clap::{Parser, Subcommand};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use studio_config::StudioPaths;
use studio_config::draft::{Draft, from_denial, from_feedback};
use studiod::api::{FileKind, Request, call};
use studiod::export;

mod mcp;
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
        /// Hire to plan it (default: the first with the producer role).
        #[arg(long)]
        hire: Option<String>,
        /// A short name for the goal; it also names the goal's branch.
        text: String,
        /// The full brief for the producer.
        #[arg(short, long, default_value = "", conflicts_with = "detail_file")]
        detail: String,
        /// Read the brief from a file instead.
        #[arg(long)]
        detail_file: Option<PathBuf>,
    },
    /// Every ticket and its state.
    Board,
    /// Run the studio-tools MCP server on stdio, for one ticket's agent.
    Mcp {
        #[arg(long)]
        ticket: String,
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
        Command::Mcp { ticket, socket } => {
            let socket = socket.unwrap_or_else(|| studiod::api::socket_path(&paths.data_dir));
            return mcp::serve(&socket, &ticket);
        }
        Command::Project(ProjectCommand::Add {
            path,
            name,
            branch,
            daily_usd,
            pack,
        }) => return add_project(paths, &path, name, branch, daily_usd, pack),
        Command::Pack(PackCommand::Add { path }) => return add_pack(paths, &path),
        Command::Rule(RuleCommand::Test { rule, since }) => {
            return test_rule(paths, &rule, &since, json);
        }
        Command::Up { .. } | Command::Down => return Ok(()),
        Command::Status => Request::Status,
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
    };
    let is_gc = matches!(request, Request::Gc);
    let is_board = matches!(request, Request::Tickets);
    let is_asks = matches!(request, Request::Asks);
    let is_budget = matches!(request, Request::Budget);
    let is_diff = matches!(request, Request::Diff { .. });
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
    } else if is_diff {
        match response.data.as_str() {
            Some(diff) => print!("{diff}"),
            None => println!("its worktree is gone: the change was landed or cleaned up"),
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
        println!(
            "{} running, {} waiting for you, ${:.2} of ${:.2} spent in 24h",
            data["running"],
            data["asks"],
            data["spent_24h"].as_f64().unwrap_or_default(),
            data["daily_usd"].as_f64().unwrap_or_default(),
        );
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
        println!("projects: {}", list("projects"));
        println!("hires: {}", list("hires"));
        if let Some(states) = data["tickets"].as_object().filter(|t| !t.is_empty()) {
            let counts: Vec<String> = states.iter().map(|(s, n)| format!("{n} {s}")).collect();
            println!("tickets: {}", counts.join(", "));
        }
    } else {
        println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
    }
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

fn print_board(tickets: &Value, etas: &Value, now_ms: u64) {
    let tickets = tickets.as_array().cloned().unwrap_or_default();
    if tickets.is_empty() {
        println!("No tickets yet. Create one with `studio ticket`.");
    }
    for t in tickets {
        let state = match &t["state"] {
            Value::String(s) => s.clone(),
            Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        let mut title = t["title"].as_str().unwrap_or_default().to_string();
        if t["kind"] == "plan" {
            title.push_str(" (plan)");
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

fn add_project(
    paths: &StudioPaths,
    path: &std::path::Path,
    name: Option<String>,
    branch: Option<String>,
    daily_usd: Option<f64>,
    pack: Option<String>,
) -> Result<(), String> {
    let repo = studio_git::Repo::open(path).map_err(|e| e.to_string())?;
    let branch = match branch {
        Some(branch) => branch,
        None => repo
            .current_branch()
            .map_err(|e| e.to_string())?
            .ok_or("HEAD is detached; pass --branch")?,
    };
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
    println!(
        "added {} ({}, work starts from {branch})",
        added.name,
        added.path.display()
    );
    if let Some(checks) = added.created_checks {
        println!("wrote {}: check its verify commands", checks.display());
    }
    println!("next: `studio up`, then give it a ticket from the board");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Draft, draft_name, eta_text, parse_since, ruleable};
    use serde_json::json;

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
