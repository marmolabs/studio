//! The studios the gallery photographs: real studiods in temporary homes,
//! with git repositories as their projects and a scripted runtime for their
//! hires, driven with the requests the web app and the CLI send.
//!
//! The busy studio is built by running real tickets to the state each
//! picture needs: a ticket's agent is a script, its checks are a shell
//! script that a ticket's marks can make hang or fail, and a held run stays
//! held until the gallery ends. Everything happens within a minute or two, so
//! every time on the pages reads "just now".

use crate::Result;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use studio_config::StudioPaths;
use studio_runtime::{
    Capabilities, Cost, EnvReport, FakeRuntime, Outcome, Run, RunEvent, RunSpec, Runtime,
    RuntimeError,
};
use studiod::Studio;
use studiod::api::{Request, Response};
use tokio::sync::Notify;

/// A studio the gallery serves.
pub struct Live {
    pub studio: Arc<Studio>,
    pub data_dir: PathBuf,
}

/// The tickets the pictures name, found by what they are rather than by number.
pub struct Ids {
    pub working: String,
    pub asking: String,
    pub checks: String,
    pub review: String,
    pub ready: String,
    pub held: String,
    pub failed: String,
    pub planner: String,
    pub goal: String,
    pub landed: String,
    pub landed_merged: String,
}

pub struct Studios {
    pub busy: Live,
    pub idle: Live,
    pub empty: Live,
    pub ids: Ids,
}

/// The most time any one wait may take; it only catches a hang.
const WAIT: Duration = Duration::from_secs(90);

pub async fn open(dir: &Path) -> Result<Studios> {
    let shop = repo(dir, "shop", SHOP_FILES, SHOP_CHECKS)?;
    let editor = repo(dir, "editor", EDITOR_FILES, EDITOR_CHECKS)?;
    let repos = [("shop", shop), ("editor", editor)];

    let (idle_paths, idle_studio) = home(dir, "idle", &repos, Brain::new())?;
    let empty_paths = StudioPaths::resolve(Some(dir.join("empty")));
    studio_config::init(&empty_paths)?;
    let empty = Studio::open(empty_paths.clone(), Brain::new().runtimes())?;

    let brain = Brain::new();
    let (busy_paths, busy_studio) = home(dir, "busy", &repos, brain.clone())?;
    let mut driver = Driver::new(Arc::clone(&busy_studio), brain, &repos);
    let ids = driver.busy_office().await?;
    Ok(Studios {
        busy: Live {
            studio: busy_studio,
            data_dir: busy_paths.data_dir,
        },
        idle: Live {
            studio: idle_studio,
            data_dir: idle_paths.data_dir,
        },
        empty: Live {
            studio: empty,
            data_dir: empty_paths.data_dir,
        },
        ids,
    })
}

// ---- repositories and homes ------------------------------------------------

const CHECK_SCRIPT: &str = "#!/bin/sh
# Stands in for a project's real checks. A ticket's marks, kept beside its
# worktree, make a step hang (hold-<step>) or fail with a message (fail-<step>).
marks=\"$(dirname \"$PWD\")/../marks/$(basename \"$PWD\")\"
while [ -e \"$marks/hold-$1\" ]; do sleep 0.2; done
if [ -e \"$marks/fail-$1\" ]; then cat \"$marks/fail-$1\"; exit 1; fi
exit 0
";

const SHOP_FILES: &[(&str, &str)] = &[
    ("README.md", "Corner Shop\n"),
    ("scripts/check.sh", CHECK_SCRIPT),
    (
        "src/main.rs",
        "fn main() {\n    println!(\"Corner Shop\");\n}\n",
    ),
    ("src/cart.rs", "pub type Cart = Vec<(ProductId, u32)>;\n"),
    (
        "templates/cart.html",
        "<main class=\"cart-page\">\n  <h1>Your cart</h1>\n</main>\n",
    ),
];

const SHOP_CHECKS: &str = r#"verify = [
    ["sh", "scripts/check.sh", "fmt"],
    ["sh", "scripts/check.sh", "lint"],
    ["sh", "scripts/check.sh", "test"],
]

[[gates]]
name = "design"
paths = ["src/landing.rs", "design/**"]
hire = "design-critic"
prompt = "Judge the page the way a shopper sees it."
rounds = 2
"#;

const EDITOR_FILES: &[(&str, &str)] = &[
    ("README.md", "Product editor\n"),
    ("scripts/check.sh", CHECK_SCRIPT),
    ("src/main.rs", "fn main() {}\n"),
];

const EDITOR_CHECKS: &str = r#"verify = [
    ["sh", "scripts/check.sh", "fmt"],
    ["sh", "scripts/check.sh", "test"],
]
"#;

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(())
}

/// A repository on `main` holding `files`, with the project's checks beside
/// them (not committed, as a project keeps its own `.studio.toml`).
fn repo(dir: &Path, name: &str, files: &[(&str, &str)], checks: &str) -> Result<PathBuf> {
    let path = dir.join(name);
    std::fs::create_dir_all(&path)?;
    git(&path, &["init", "-q", "-b", "main"])?;
    for (key, value) in [
        ("user.name", "Owner"),
        ("user.email", "owner@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(&path, &["config", key, value])?;
    }
    for (file, text) in files {
        write(&path.join(file), text)?;
    }
    git(&path, &["add", "-A"])?;
    git(&path, &["commit", "-q", "-m", "Start the project"])?;
    write(&path.join(".studio.toml"), checks)?;
    Ok(path)
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn hire(name: &str, role: &str, model: &str, extra: &str, about: &str) -> String {
    format!(
        "---\nname: {name}\nrole: {role}\nruntime: claude-code\nmodel: {model}\n{extra}---\n{about}\n"
    )
}

/// What the studio spent on each of the 13 days before today, oldest first.
const EARLIER_DAYS: [f64; 13] = [
    3.1, 4.8, 2.2, 6.9, 5.3, 8.4, 4.1, 0.0, 1.2, 7.6, 6.2, 9.1, 5.8,
];

/// Writes those days into the ledger before the studio opens: history from
/// before the gallery began, which no request could make.
fn earlier_days(paths: &StudioPaths) -> Result<()> {
    let mut store = studio_store::Store::open(paths.data_dir.join("studio.db"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    for (n, usd) in EARLIER_DAYS.iter().enumerate() {
        let days_ago = (EARLIER_DAYS.len() - n) as u64;
        store.record_spend(&studio_store::Spend {
            at_ms: now - days_ago * 86_400_000 - 1_800_000,
            ticket_id: studio_core::ids::TicketId::new("SHOP-0"),
            project: "shop".into(),
            hire: "dev".into(),
            runtime: "claude-code".into(),
            model: "sonnet".into(),
            session: None,
            usd: *usd,
            reported: true,
            input_tokens: 0,
            output_tokens: 0,
        })?;
    }
    Ok(())
}

/// A studio home with the starter files, then the gallery's own hires, rules
/// and projects, opened on `brain`.
fn home(
    dir: &Path,
    name: &str,
    repos: &[(&str, PathBuf)],
    brain: Brain,
) -> Result<(StudioPaths, Arc<Studio>)> {
    let paths = StudioPaths::resolve(Some(dir.join(name)));
    studio_config::init(&paths)?;
    earlier_days(&paths)?;
    let hires = paths.config_dir.join("hires");
    std::fs::remove_file(hires.join("engineer.md"))?;
    let files = [
        (
            "dev",
            hire(
                "Dev",
                "engineer",
                "sonnet",
                "escalate: [opus]\neffort: medium\ndaily_usd: 8.0\n",
                "Writes code, with tests, in small steps.",
            ),
        ),
        (
            "designer",
            hire(
                "Designer",
                "designer",
                "sonnet",
                "daily_usd: 2.0\n",
                "Page layouts, images and icons.",
            ),
        ),
        (
            "producer",
            hire(
                "Producer",
                "producer",
                "sonnet",
                "tools: [file_ticket]\n",
                "Plans goals as small tickets.",
            ),
        ),
        (
            "writer",
            hire(
                "Writer",
                "writer",
                "sonnet",
                "",
                "Words for the store: emails and page copy.",
            ),
        ),
        (
            "design-critic",
            hire(
                "Design critic",
                "critic",
                "opus",
                "escalate: [sonnet]\n",
                "Design critic for pages and images.",
            ),
        ),
        (
            "reviewer",
            hire(
                "Reviewer",
                "reviewer",
                "opus",
                "escalate: [sonnet]\n",
                "Reads every diff before it lands.",
            ),
        ),
        (
            "approver",
            hire(
                "Approver",
                "approver",
                "haiku",
                "",
                "Decides tool calls no rule covers.",
            ),
        ),
        (
            "router",
            hire(
                "Router",
                "router",
                "haiku",
                "",
                "Picks the hire for new tickets.",
            ),
        ),
        // A file that does not load, as the hires page shows it.
        ("scribe", "---\nname: Scribe\nrole\n---\nWrites.\n".into()),
    ];
    for (id, text) in files {
        write(&hires.join(format!("{id}.md")), &text)?;
    }
    write(
        &paths.config_dir.join("rules/house-style.md"),
        "---\nkind: guide\nlevel: warn\n---\nRead CONTRIBUTING.md before you start.\n",
    )?;
    let mut config = String::from(
        "[budget]\ndaily_usd = 10.0\nticket_usd = 2.0\nrun_usd = 0.4\n\n[concurrency]\nmax_runs = 8\n",
    );
    for (name, path) in repos {
        let daily = if *name == "shop" { 8.0 } else { 4.0 };
        config.push_str(&format!(
            "\n[[projects]]\nname = \"{name}\"\npath = \"{}\"\nintegration_branch = \"main\"\ndaily_usd = {daily}\n",
            path.display()
        ));
        let rules = path.join(".studio/rules");
        if *name == "shop" {
            write(
                &rules.join("protect-payments.md"),
                "---\nkind: protect\nlevel: ask\nwhen:\n  paths: [\"src/payments/**\"]\n---\nPayment code moves real money. Ask before\nchanging anything under src/payments.\n",
            )?;
            write(
                &rules.join("run-the-checks.md"),
                "---\nkind: check\nlevel: block\ncommand: [sh, scripts/check.sh, rules]\n---\nfmt, lint and tests must pass.\n",
            )?;
        }
    }
    write(&paths.config_dir.join("studio.toml"), &config)?;
    let studio = Studio::open(paths.clone(), brain.runtimes())?;
    Ok((paths, studio))
}

// ---- the scripted runtime --------------------------------------------------

/// The one runtime every hire names. It hands each run to the scripted
/// [`FakeRuntime`] set for it: an author's by its ticket's worktree, a
/// reviewer's or a gate's likewise, an approver's by what the call contains.
#[derive(Clone)]
struct Brain(Arc<Mutex<Tables>>);

struct Tables {
    authors: HashMap<String, FakeRuntime>,
    reviews: HashMap<String, FakeRuntime>,
    gates: HashMap<String, FakeRuntime>,
    approvals: Vec<(String, FakeRuntime)>,
    /// What runs nobody scripted do: an author, a reviewer or gate that
    /// passes, an approver that allows.
    author: FakeRuntime,
    pass: FakeRuntime,
    allow: FakeRuntime,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn cost(usd: f64) -> Cost {
    Cost {
        usd,
        reported: true,
        ..Cost::default()
    }
}

/// A run that ends with `text` as its reply, as reviewers and approvers do.
fn reply(text: &str) -> RunEvent {
    RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some("reply".into()),
        cost: cost(0.01),
        summary: Some(text.into()),
    }
}

impl Brain {
    fn new() -> Brain {
        Brain(Arc::new(Mutex::new(Tables {
            authors: HashMap::new(),
            reviews: HashMap::new(),
            gates: HashMap::new(),
            approvals: Vec::new(),
            author: FakeRuntime::new(vec![RunEvent::Finished {
                outcome: Outcome::Success,
                session: None,
                cost: cost(0.1),
                summary: Some("Done.".into()),
            }]),
            pass: FakeRuntime::new(vec![reply("Does what the ticket asks.\nVERDICT: PASS")]),
            allow: FakeRuntime::new(vec![reply("Fine for this ticket.\nDECISION: ALLOW: fine")]),
        })))
    }

    fn runtimes(&self) -> HashMap<String, Arc<dyn Runtime>> {
        let mut all: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        all.insert("claude-code".into(), Arc::new(self.clone()));
        all
    }
}

impl Runtime for Brain {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn capabilities(&self) -> Capabilities {
        FakeRuntime::default().capabilities()
    }

    fn check_env(&self) -> EnvReport {
        FakeRuntime::default().check_env()
    }

    fn start(&self, spec: RunSpec) -> std::result::Result<Run, RuntimeError> {
        let ticket = spec
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prompt = spec.prompt.as_str();
        let tables = lock(&self.0);
        let fake = (if prompt.contains("Decide whether the agent may make this tool call.") {
            tables
                .approvals
                .iter()
                .find(|(needle, _)| prompt.contains(needle.as_str()))
                .map_or(&tables.allow, |(_, fake)| fake)
        } else if prompt.starts_with("You are the ") && prompt.contains(" gate for ") {
            tables.gates.get(&ticket).unwrap_or(&tables.pass)
        } else if prompt.starts_with("Review this change for") {
            tables.reviews.get(&ticket).unwrap_or(&tables.pass)
        } else {
            tables.authors.get(&ticket).unwrap_or(&tables.author)
        })
        .clone();
        drop(tables);
        fake.start(spec)
    }
}

// ---- what an agent does ----------------------------------------------------

/// One thing an agent does in its run.
#[derive(Clone)]
enum Beat {
    Say(&'static str),
    Read(&'static str),
    Edit(&'static str),
    Write(&'static str),
    Search(&'static str),
    /// A command, which waits for the approver or a rule.
    Run(&'static str),
    /// A page read, which waits likewise.
    Fetch(&'static str),
    /// A change to a protected file, which waits for the owner.
    Protected(&'static str),
}

fn story(slug: &str, beats: &[Beat], usd: f64) -> Vec<RunEvent> {
    let mut events = vec![RunEvent::Started {
        session: format!("s-{slug}"),
        model: None,
    }];
    for (n, beat) in beats.iter().enumerate() {
        let call = |name: &str, input: Value| RunEvent::ToolCall {
            id: format!("t{n}"),
            name: name.into(),
            input,
        };
        let ask = |tool: &str, input: Value| RunEvent::Permission {
            request_id: format!("p{n}"),
            tool: tool.into(),
            input,
        };
        match beat {
            Beat::Say(text) => events.push(RunEvent::Text {
                text: (*text).into(),
            }),
            Beat::Read(file) => events.push(call("Read", json!({"file_path": file}))),
            Beat::Edit(file) => events.push(call("Edit", json!({"file_path": file}))),
            Beat::Write(file) => events.push(call("Write", json!({"file_path": file}))),
            Beat::Search(pattern) => events.push(call("Grep", json!({"pattern": pattern}))),
            Beat::Run(command) => {
                let input = json!({"command": command});
                events.push(call("Bash", input.clone()));
                events.push(ask("Bash", input));
            }
            Beat::Fetch(url) => {
                let input = json!({"url": url});
                events.push(call("WebFetch", input.clone()));
                events.push(ask("WebFetch", input));
            }
            Beat::Protected(file) => events.push(ask(
                "Edit",
                json!({"file_path": file,
                       "old_string": "const MAX_RETRIES: u32 = 1;",
                       "new_string": "const MAX_RETRIES: u32 = 3;"}),
            )),
        }
    }
    events.push(RunEvent::Finished {
        outcome: Outcome::Success,
        session: Some(format!("s-{slug}")),
        cost: cost(usd),
        summary: Some("Done.".into()),
    });
    events
}

/// Where a ticket ends up in the picture.
#[derive(Clone)]
enum Fate {
    /// Passes review and lands on its goal's branch.
    Landed,
    /// Ready, sent back once with these notes, ready again.
    Revised(&'static str),
    /// Its agent is at work.
    Working,
    /// Its agent waits for the owner's answer.
    Asking,
    /// Its checks are running.
    Checking,
    /// Its reviewer is reading.
    Reviewing,
    /// The reviewer sends it back each time until it fails.
    FailsReview(&'static str),
    /// Its checks fail each time until it fails.
    FailsChecks(&'static str),
    /// Its checks fail until the ticket has spent its cap and is held.
    OverBudget(&'static str),
    /// Waits for a desk or its goal.
    Queued,
}

#[derive(Clone)]
struct Job {
    title: &'static str,
    hire: &'static str,
    detail: &'static str,
    fate: Fate,
    beats: Vec<Beat>,
    /// What the agent leaves in its worktree.
    files: Vec<(&'static str, &'static str)>,
    /// What one run costs, in USD.
    usd: f64,
}

fn job(title: &'static str, hire: &'static str, fate: Fate) -> Job {
    Job {
        title,
        hire,
        detail: "",
        fate,
        beats: vec![Beat::Say("Working through the ticket.")],
        files: Vec::new(),
        usd: 0.15,
    }
}

impl Job {
    fn detail(mut self, detail: &'static str) -> Job {
        self.detail = detail;
        self
    }

    fn beats(mut self, beats: &[Beat]) -> Job {
        self.beats = beats.to_vec();
        self
    }

    fn files(mut self, files: &[(&'static str, &'static str)]) -> Job {
        self.files = files.to_vec();
        self
    }

    fn usd(mut self, usd: f64) -> Job {
        self.usd = usd;
        self
    }
}

// ---- driving a studio ------------------------------------------------------

struct Driver {
    studio: Arc<Studio>,
    brain: Brain,
    repos: HashMap<String, PathBuf>,
    /// Tickets made so far, by project: the next id follows from it.
    made: HashMap<String, usize>,
    /// What lets each ticket's agent finish.
    holds: HashMap<String, Arc<Notify>>,
}

impl Driver {
    fn new(studio: Arc<Studio>, brain: Brain, repos: &[(&str, PathBuf)]) -> Driver {
        Driver {
            studio,
            brain,
            repos: repos
                .iter()
                .map(|(name, path)| ((*name).to_string(), path.clone()))
                .collect(),
            made: HashMap::new(),
            holds: HashMap::new(),
        }
    }

    async fn call(&self, request: Request) -> Result<Value> {
        let Response { ok, data, error } = self.studio.handle(request).await;
        if ok {
            Ok(data)
        } else {
            Err(error.unwrap_or_else(|| "the studio refused".into()).into())
        }
    }

    async fn state(&self, id: &str) -> Result<Value> {
        let tickets = self.call(Request::Tickets).await?;
        let ticket = tickets
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["id"] == id)
            .ok_or_else(|| format!("no ticket {id}"))?;
        Ok(ticket["state"].clone())
    }

    async fn is(&self, id: &str, state: &str) -> Result<bool> {
        let now = self.state(id).await?;
        Ok(now == state || now.get(state).is_some())
    }

    async fn until(&self, what: &str, mut check: impl AsyncFnMut() -> Result<bool>) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if check().await? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let tickets = self.call(Request::Tickets).await?;
        let states: Vec<String> = tickets
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| format!("{} {}", t["id"], t["state"]))
            .collect();
        Err(format!("waited for {what} and it never came; {}", states.join(", ")).into())
    }

    async fn reach(&self, id: &str, state: &str) -> Result<()> {
        self.until(&format!("{id} to be {state}"), async || {
            self.is(id, state).await
        })
        .await
    }

    /// Lets the ticket's agent finish, over and over, until the ticket is
    /// `state`: it may run more than once on its way.
    async fn finish_until(&self, id: &str, state: &str) -> Result<()> {
        self.until(&format!("{id} to be {state}"), async || {
            if let Some(hold) = self.holds.get(id) {
                hold.notify_one();
            }
            self.is(id, state).await
        })
        .await
    }

    fn next_id(&mut self, project: &str) -> String {
        let count = self.made.entry(project.to_string()).or_default();
        *count += 1;
        let prefix: String = project
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(4)
            .collect();
        format!("{}-{count}", prefix.to_ascii_uppercase())
    }

    fn worktree(&self, project: &str, id: &str) -> PathBuf {
        self.repos[project]
            .join(".studio/worktrees")
            .join(id.to_ascii_lowercase())
    }

    /// Waits for the ticket's worktree and leaves `files` in it.
    async fn plant(&self, project: &str, id: &str, files: &[(&str, &str)]) -> Result<()> {
        let path = self.worktree(project, id);
        self.until(&format!("{id}'s worktree"), async || Ok(path.exists()))
            .await?;
        for (file, text) in files {
            write(&path.join(file), text)?;
        }
        Ok(())
    }

    /// Marks a step of the ticket's checks, which the check script reads.
    fn mark(&self, project: &str, id: &str, mark: &str, text: &str) -> Result<()> {
        let dir = self.repos[project]
            .join(".studio/marks")
            .join(id.to_ascii_lowercase());
        write(&dir.join(mark), text)
    }

    /// Sets what the ticket's runs do, before the ticket exists.
    fn script(&mut self, id: &str, job: &Job) {
        let slug = id.to_ascii_lowercase();
        let author = FakeRuntime::new(story(&slug, &job.beats, job.usd));
        let (author, hold) = author.held();
        self.holds.insert(id.to_string(), hold);
        let mut tables = lock(&self.brain.0);
        tables.authors.insert(slug.clone(), author);
        match job.fate {
            Fate::FailsReview(notes) => {
                let reviewer = FakeRuntime::new(vec![reply(&format!("{notes}\nVERDICT: FAIL"))]);
                tables.reviews.insert(slug, reviewer);
            }
            Fate::Reviewing => {
                let (reviewer, _) =
                    FakeRuntime::new(vec![reply("Reading the change.\nVERDICT: PASS")]).held();
                tables.reviews.insert(slug, reviewer);
            }
            _ => {}
        }
    }

    /// Makes the ticket with `CreateTicket` and drives it to its fate.
    async fn single(&mut self, project: &str, goal: &str, job: Job) -> Result<String> {
        let id = self.next_id(project);
        self.script(&id, &job);
        let made = self
            .call(Request::CreateTicket {
                project: project.into(),
                title: job.title.into(),
                goal: goal.into(),
                hire: job.hire.into(),
                base: None,
                detail: job.detail.into(),
            })
            .await?;
        if made["id"] != id.as_str() {
            return Err(format!("expected {id}, the studio made {made}").into());
        }
        self.drive(project, &id, &job).await?;
        Ok(id)
    }

    /// Plans a goal: a producer files the `jobs` and the plan lands, or, with
    /// no jobs, the producer stays at work. Then each ticket is driven.
    async fn plan(
        &mut self,
        project: &str,
        goal: &str,
        jobs: Vec<Job>,
    ) -> Result<(String, Vec<String>)> {
        let plan = self.next_id(project);
        let kids: Vec<String> = jobs.iter().map(|_| self.next_id(project)).collect();
        let mut beats = vec![Beat::Say(
            "Reading the code to split the goal into small tickets.",
        )];
        beats.extend(jobs.iter().map(|_| Beat::Read("src/main.rs")));
        let plan_job = job("", "producer", Fate::Working).beats(&beats).usd(0.05);
        self.script(&plan, &plan_job);
        for (kid, job) in kids.iter().zip(&jobs) {
            self.script(kid, job);
        }
        let made = self
            .call(Request::CreateGoal {
                project: project.into(),
                text: goal.into(),
                hire: None,
                detail: String::new(),
            })
            .await?;
        if made["id"] != plan.as_str() {
            return Err(format!("expected {plan}, the studio made {made}").into());
        }
        self.reach(&plan, "working").await?;
        if jobs.is_empty() {
            return Ok((plan, kids));
        }
        for (kid, job) in kids.iter().zip(&jobs) {
            let filed = self
                .call(Request::FileTicket {
                    ticket: plan.clone(),
                    title: job.title.into(),
                    detail: job.detail.into(),
                    hire: Some(job.hire.into()),
                })
                .await?;
            if filed["id"] != kid.as_str() {
                return Err(format!("expected {kid}, the studio made {filed}").into());
            }
        }
        self.finish_until(&plan, "landed").await?;
        for (kid, job) in kids.iter().zip(&jobs) {
            self.drive(project, kid, job).await?;
        }
        Ok((plan, kids))
    }

    async fn drive(&mut self, project: &str, id: &str, job: &Job) -> Result<()> {
        let files = job.files.as_slice();
        match &job.fate {
            Fate::Queued => self.reach(id, "queued").await,
            Fate::Working => self.reach(id, "working").await,
            Fate::Asking => {
                self.reach(id, "asking").await?;
                self.plant(project, id, files).await
            }
            Fate::Checking => {
                self.mark(project, id, "hold-test", "")?;
                self.plant(project, id, files).await?;
                self.finish_until(id, "checking").await
            }
            Fate::Reviewing => {
                self.plant(project, id, files).await?;
                self.finish_until(id, "reviewing").await
            }
            Fate::Landed | Fate::Revised(_) => {
                self.plant(project, id, files).await?;
                self.finish_until(id, "ready").await?;
                if let Fate::Revised(notes) = job.fate {
                    self.call(Request::RequestChanges {
                        ticket: id.into(),
                        feedback: notes.into(),
                    })
                    .await?;
                    self.finish_until(id, "ready").await?;
                }
                if matches!(job.fate, Fate::Landed) {
                    self.call(Request::Land { ticket: id.into() }).await?;
                }
                Ok(())
            }
            Fate::FailsReview(_) => {
                self.plant(project, id, files).await?;
                self.finish_until(id, "failed").await
            }
            Fate::FailsChecks(message) => {
                self.mark(project, id, "fail-lint", message)?;
                self.plant(project, id, files).await?;
                self.finish_until(id, "failed").await
            }
            Fate::OverBudget(message) => {
                self.mark(project, id, "fail-test", message)?;
                self.plant(project, id, files).await?;
                self.finish_until(id, "held").await
            }
        }
    }

    /// What the approver says to a call with this in it.
    fn approver(&mut self, needle: &str, decision: &str) {
        let fake = FakeRuntime::new(vec![reply(decision)]);
        lock(&self.brain.0)
            .approvals
            .push((needle.to_string(), fake));
    }

    /// The busy office: work in every state the pictures show.
    async fn busy_office(&mut self) -> Result<Ids> {
        use Beat::{Edit, Fetch, Protected, Read, Run, Say, Search, Write};
        println!("building the busy studio");
        self.studio.spawn_scheduler();
        for (needle, decision) in [
            (
                "cargo run -- --port 8080",
                "Starting the store opens port 8080 on this machine, so it is yours to decide.\nDECISION: OWNER: starting the store opens port 8080 on this machine, so it is yours to decide",
            ),
            (
                "cargo run --bin mailer",
                "Sending mail reaches outside the project.\nDECISION: OWNER: sending the launch email goes to a real address, so it is yours to decide",
            ),
            (
                "rm -rf ~/.cargo/registry/cache",
                "Clearing the shared cache slows every other build.\nDECISION: DENY: clearing the shared Cargo cache is outside the ticket and slows every other build",
            ),
            (
                "cargo test",
                "Runs the tests in the worktree.\nDECISION: ALLOW: runs the tests inside the worktree",
            ),
            (
                "cargo clippy",
                "Lints the worktree.\nDECISION: ALLOW: lints the worktree; nothing leaves it",
            ),
            (
                "doc.rust-lang.org",
                "Public documentation.\nDECISION: ALLOW: reads public documentation",
            ),
            (
                "git diff --stat",
                "Reads the branch's own changes.\nDECISION: ALLOW: reads the branch's own changes",
            ),
        ] {
            self.approver(needle, decision);
        }

        // Work that is over, goal by goal.
        let (_, nav) = self
            .plan(
                "shop",
                "Polish the navigation",
                vec![
                    job(
                        "Landing page with Shop now and Sign out",
                        "dev",
                        Fate::Landed,
                    )
                    .beats(&[
                        Say("Adding a landing page with a Shop now link."),
                        Write("src/landing.rs"),
                        Edit("src/main.rs"),
                        Run("cargo test --workspace"),
                    ])
                    .files(&[("src/landing.rs", LANDING_V1)])
                    .usd(0.2),
                    job("Breadcrumbs on product pages", "dev", Fate::Landed)
                        .files(&[("src/breadcrumbs.rs", "pub fn trail() {}\n")]),
                ],
            )
            .await?;
        self.call(Request::Merge {
            ticket: nav[0].clone(),
        })
        .await?;

        let (look, _) = self
            .plan(
                "shop",
                "Give the store its look: logo, product cards, launch email",
                vec![
                    job("Draw the store logo", "designer", Fate::Landed)
                        .files(&[("design/logo.txt", "A shopping basket in two colours.\n")])
                        .usd(0.12),
                    job(
                        "Design the product cards",
                        "designer",
                        Fate::FailsReview(
                            "Design gate: the palette is too wide for the product cards.",
                        ),
                    )
                    .files(&[("design/cards.txt", "Cards in nine colours.\n")])
                    .usd(0.1),
                    job("Write the launch email", "writer", Fate::Asking)
                        .beats(&[
                            Say("Drafting the launch email from the store's voice."),
                            Write("templates/email/launch.html"),
                            Run("cargo run --bin mailer -- send --template templates/email/launch.html --to owner@example.com --subject \"Corner Shop is open\""),
                        ])
                        .files(&[("templates/email/launch.html", "<h1>Corner Shop is open</h1>\n")]),
                ],
            )
            .await?;

        self.plan(
            "shop",
            "Save carts between sessions",
            vec![
                job("Store carts in the database", "dev", Fate::Landed)
                    .files(&[("src/carts.rs", "pub fn save() {}\n")])
                    .usd(0.2),
            ],
        )
        .await?;

        let (_, checkout) = self
            .plan(
                "shop",
                "Checkout flow",
                vec![
                    job("Count the items in the cart", "dev", Fate::Landed)
                        .files(&[("src/cart_count.rs", "pub fn count() {}\n")]),
                    job("Add a cart drawer", "dev", Fate::Asking)
                        .detail(DRAWER)
                        .beats(&[
                            Say("Adding an item sends the shopper to `/cart`, away from the product. I'll put prices on the cart's lines in `cart.rs` and turn `cart.html` into a drawer:\n\n- `Line`: a product with its price and quantity\n- `total(cart)`: the sum of every line\n- `cart-drawer`: the lines and total, opened in place"),
                            Read("templates/cart.html"),
                            Read("src/cart.rs"),
                            Search("add_to_cart"),
                            Edit("src/cart.rs"),
                            Edit("templates/cart.html"),
                            Run("cargo test --locked cart::"),
                            Say("The tests pass. Before I hand it over I'd like to try the drawer in a browser."),
                            Run("cargo run -- --port 8080"),
                        ])
                        .files(&[("src/cart.rs", CART_V2), ("templates/cart.html", CART_HTML_V2)]),
                    job("Validate the payment form", "dev", Fate::Queued),
                    job("Design the order confirmation page", "designer", Fate::Queued),
                ],
            )
            .await?;
        let _ = checkout;

        // Single tickets.
        let footer = self
            .single(
                "shop",
                "",
                job("Fix the broken footer links", "dev", Fate::Landed).files(&[(
                    "templates/footer.html",
                    "<a href=\"/contact\">Contact</a>\n",
                )]),
            )
            .await?;
        let _ = footer;
        let failed = self
            .single(
                "shop",
                "",
                job(
                    "Load the product list faster",
                    "dev",
                    Fate::FailsChecks("cargo clippy: 2 warnings"),
                )
                .files(&[("src/products.rs", "pub fn list() {}\n")])
                .usd(0.2),
            )
            .await?;
        let held = self
            .single(
                "shop",
                "",
                job(
                    "Add gift card support",
                    "dev",
                    Fate::OverBudget("cargo test: 1 failed"),
                )
                .files(&[("src/gift.rs", "pub fn redeem() {}\n")])
                .usd(1.0),
            )
            .await?;
        let ready = self
            .single(
                "shop",
                "",
                job(
                    "Add a landing page",
                    "dev",
                    Fate::Revised(
                        "The **Sign out** button needs a confirm step:\n\n- ask before signing out\n- keep `Esc` as cancel",
                    ),
                )
                .beats(&[
                    Say("Added a **landing page** with a Shop now link and a Sign out button:\n\n- `src/landing.rs` renders the page\n- `main.rs` serves it at `/`"),
                    Edit("src/landing.rs"),
                    Edit("src/main.rs"),
                    Run("cargo test --workspace"),
                ])
                .files(&[("src/landing.rs", LANDING_V2)])
                .usd(0.25),
            )
            .await?;

        // The editor's goal: planned, one ticket landed, one at work, two waiting.
        let (_, editor) = self
            .plan(
                "editor",
                "Ship a product editor: add items, undo mistakes, save drafts",
                vec![
                    job("Add a form for new items", "dev", Fate::Landed)
                        .files(&[("src/form.rs", "pub fn form() {}\n")])
                        .usd(0.2),
                    job("Add undo and redo", "dev", Fate::Working),
                    job("Save drafts as JSON files", "dev", Fate::Queued),
                    job("Show how many drafts are saved", "dev", Fate::Queued),
                ],
            )
            .await?;

        // Work in flight.
        let working = self
            .single(
                "shop",
                "Speed up search results",
                job("Fix the search cache", "dev", Fate::Working)
                    .beats(&[
                        Say("Repeat searches miss the cache and run the full query again. Let me look at the search code."),
                        Read("src/search/cache.rs"),
                        Read("src/search/query.rs"),
                        Search("cache_key"),
                        Say("The cache is keyed by the raw query, so `Mugs` and `mugs` miss each other. I'll key it by the trimmed, lowercased words instead."),
                        Edit("src/search/cache.rs"),
                        Run("cargo test -p shop search::cache"),
                        Run("rm -rf ~/.cargo/registry/cache"),
                        Say("That was denied, so I'll leave the cache alone and run the tests again."),
                        Run("cargo test --workspace"),
                    ]),
            )
            .await?;
        self.single(
            "shop",
            "",
            job("Retry payments that time out", "dev", Fate::Asking).beats(&[
                Say("Payments that time out are tried once. Three tries is the usual."),
                Read("src/payments/retry.rs"),
                Protected("src/payments/retry.rs"),
            ]),
        )
        .await?;
        let checks = self
            .single(
                "shop",
                "",
                job("Resize product images", "dev", Fate::Checking)
                    .beats(&[
                        Say("Product images are resized to three widths now. Handing over to the checks."),
                        Run("cargo clippy --all-targets -- -D warnings"),
                        Fetch("https://doc.rust-lang.org/std/fs/fn.read_dir.html"),
                    ])
                    .files(&[("src/images.rs", "pub fn widths() -> [u32; 3] { [320, 640, 1280] }\n")]),
            )
            .await?;
        let review = self
            .single(
                "shop",
                "",
                job("Add an order history page", "dev", Fate::Reviewing)
                    .beats(&[Write("src/orders.rs"), Run("git diff --stat main...HEAD")])
                    .files(&[("src/orders.rs", "pub fn history() {}\n")]),
            )
            .await?;
        let (planner, _) = self
            .plan(
                "shop",
                "Add a settings page for email and notification preferences",
                Vec::new(),
            )
            .await?;
        // Every desk is taken now, so this one waits.
        self.single("shop", "", job("Add a contact page", "dev", Fate::Queued))
            .await?;

        let goal = look;
        let landed_merged = nav[0].clone();
        let landed = editor[0].clone();
        Ok(Ids {
            working,
            asking: checkout[1].clone(),
            checks,
            review,
            ready,
            held,
            failed,
            planner,
            goal,
            landed,
            landed_merged,
        })
    }
}

const LANDING_V1: &str = "pub fn page(html: &mut Html) {
    html.heading(\"Corner Shop\");
    html.link(\"Shop now\", \"/products\");
}
";

const LANDING_V2: &str = "pub fn page(html: &mut Html, user: Option<&User>) {
    html.heading(\"Corner Shop\");
    html.link(\"Shop now\", \"/products\");
    if let Some(user) = user {
        html.text(&format!(\"Signed in as {}\", user.name));
        html.button(\"Sign out\", \"/sign-out\");
    }
}
";

const CART_V2: &str = "pub type Cart = Vec<Line>;

pub fn total(cart: &Cart) -> u32 {
    cart.iter().map(|line| line.price * line.quantity).sum()
}
";

const CART_HTML_V2: &str = "<aside class=\"cart-drawer\" aria-label=\"Cart\">
  <h2>Your cart: {{ total }}</h2>
</aside>
";

const DRAWER: &str = "Adding an item opens a drawer with the cart's lines and total, so the shopper stays on the page. The drawer closes when they click outside it or go to checkout. The total comes from `cart::total`, so the drawer and the checkout page always agree.\n\n**Done when** the drawer lists every line with its quantity and price, its total matches the checkout page, and `cart::total` has tests.";
