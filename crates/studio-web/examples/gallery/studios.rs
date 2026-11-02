//! The studios the gallery photographs: real studiods in temporary homes,
//! with git repositories as their projects and a scripted runtime for their
//! hires, driven with the requests the web app and the CLI send.
//!
//! The busy studio is built by running real tickets to the state each
//! picture needs: a ticket's agent is a script, its checks are a shell
//! script that a ticket's marks can make hang or fail, and a held run stays
//! held until the gallery ends. Everything happens within a minute or two, so
//! every time on the pages reads "just now".
//!
//! The busy studio is a company of four projects, each with a lead and each
//! in a different phase: the shop is building (a milestone under way and asks
//! for the owner), the editor is testing, the admin tool is releasing (its
//! release waits for the owner's go-ahead) and the docs are live. A lead's
//! turns can't run here, so what the leads said is written into the studio's
//! data folder before it is served: the shop's conversation and every lead's
//! standup.
//!
//! The starting studio has five projects that were just added through
//! `/new`, one in each state a setup can be in: its lead reading the
//! repository, the studio trying the checks it proposed, its proposal ready,
//! the setup failed, and the owner's choice accepted.

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
use studiod::api::{
    FirstGoal, NewLead, PlanTask, ProposedCommand, ProtectedPath, Request, Response, SetupChoice,
    SetupProposal, SetupState, TalkCard, TalkFrom, TalkMessage,
};
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
    pub ready: String,
    pub failed: String,
    /// A goal whose every task is done and checked, which waits to ship.
    pub ready_goal: String,
    /// The checkout goal, whose plan the shop's lead put in the conversation.
    pub checkout_goal: String,
    pub landed: String,
    pub landed_merged: String,
}

pub struct Studios {
    pub busy: Live,
    pub idle: Live,
    pub empty: Live,
    /// A studio with one project and a release that waits for the owner.
    pub waiting: Live,
    /// A studio whose one project has no lead yet, with the leads of the
    /// others' to pick from.
    pub leadless: Live,
    /// A studio with a project in each state a setup through its lead can be
    /// in, named by `SETUPS`.
    pub starting: Live,
    pub ids: Ids,
}

/// The most time any one wait may take; it only catches a hang.
const WAIT: Duration = Duration::from_secs(90);

pub async fn open(dir: &Path) -> Result<Studios> {
    let shop = repo(dir, "shop", SHOP_FILES, SHOP_CHECKS)?;
    let editor = repo(dir, "editor", EDITOR_FILES, EDITOR_CHECKS)?;
    let admin = repo(dir, "admin", ADMIN_FILES, RELEASABLE_CHECKS)?;
    let docs = repo(dir, "docs", DOCS_FILES, RELEASABLE_CHECKS)?;
    let repos = [
        ("shop", shop),
        ("editor", editor),
        ("admin", admin),
        ("docs", docs),
    ];

    // The idle studio has the first two projects, each with its lead.
    let (idle_paths, idle_studio) = home(dir, "idle", &repos[..2], Brain::new(), true, &[])?;
    // The leadless one has the shop, which nobody leads; the hires are the
    // same, so there are leads to pick.
    let (leadless_paths, leadless_studio) =
        home(dir, "leadless", &repos[..1], Brain::new(), false, &[])?;
    let empty_paths = StudioPaths::resolve(Some(dir.join("empty")));
    studio_config::init(&empty_paths)?;
    let empty = Studio::open(empty_paths.clone(), Brain::new().runtimes())?;

    // The studio whose only news is a release that waits for the owner.
    let waiting_repo = repo(
        &dir.join("waiting-repos"),
        "admin",
        ADMIN_FILES,
        RELEASABLE_CHECKS,
    )?;
    let waiting_repos = [("admin", waiting_repo)];
    let brain = Brain::new();
    let (waiting_paths, waiting_studio) = home(
        dir,
        "waiting",
        &waiting_repos,
        brain.clone(),
        true,
        &[(
            "admin",
            "v1.2 passed its final checks. It waits for your go-ahead.",
        )],
    )?;
    let mut waiting = Driver::new(Arc::clone(&waiting_studio), brain, &waiting_repos);
    waiting.release("admin", "1.2", false).await?;

    let starting = starting(dir).await?;

    let brain = Brain::new();
    let (busy_paths, busy_studio) = home(dir, "busy", &repos, brain.clone(), false, STANDUPS)?;
    let mut driver = Driver::new(Arc::clone(&busy_studio), brain, &repos);
    let ids = driver.busy_office().await?;
    seed_shop_talk(&busy_paths, &ids)?;
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
        waiting: Live {
            studio: waiting_studio,
            data_dir: waiting_paths.data_dir,
        },
        leadless: Live {
            studio: leadless_studio,
            data_dir: leadless_paths.data_dir,
        },
        starting,
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

const ADMIN_FILES: &[(&str, &str)] = &[
    ("README.md", "Staff admin\n"),
    ("scripts/check.sh", CHECK_SCRIPT),
    ("scripts/package.sh", PACKAGE_SCRIPT),
    ("scripts/publish.sh", PUBLISH_SCRIPT),
    ("src/main.rs", "fn main() {}\n"),
];

const DOCS_FILES: &[(&str, &str)] = &[
    ("README.md", "Help guides\n"),
    ("scripts/check.sh", CHECK_SCRIPT),
    ("scripts/package.sh", PACKAGE_SCRIPT),
    ("scripts/publish.sh", PUBLISH_SCRIPT),
    ("guides/start.md", "# Start here\n"),
];

/// What a release runs, with `VERSION` set.
const PACKAGE_SCRIPT: &str = "#!/bin/sh
echo \"Packaging $VERSION\"
";

const PUBLISH_SCRIPT: &str = "#!/bin/sh
echo \"Publishing $VERSION\"
";

/// Checks that pass, and the commands that release the project.
const RELEASABLE_CHECKS: &str = r#"verify = [
    ["sh", "scripts/check.sh", "fmt"],
    ["sh", "scripts/check.sh", "test"],
]
release = [
    ["sh", "scripts/package.sh"],
    ["sh", "scripts/publish.sh"],
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
/// them (not committed, as a project keeps its own `.studio.toml`); with no
/// checks it has no such file.
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
    if !checks.is_empty() {
        write(&path.join(".studio.toml"), checks)?;
    }
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

/// A project lead: an Opus hire, who answers for `project` and its team.
fn lead(name: &str, project: &str, about: &str) -> String {
    hire(
        name,
        "lead",
        "opus",
        "tools: [file_ticket]\n",
        &format!("Leads {project}. {about}"),
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
/// and projects, opened on `brain`. With `led`, each project names its lead
/// in `studio.toml`; otherwise the leads are set afterwards. `standups` are
/// what the leads said of their projects (project and words), kept before the
/// studio opens, as a lead's one-turn call can't run here.
fn home(
    dir: &Path,
    name: &str,
    repos: &[(&str, PathBuf)],
    brain: Brain,
    led: bool,
    standups: &[(&str, &str)],
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
        (
            "shop-lead",
            lead(
                "Nora",
                "the shop",
                "Plans its goals and hands out its tasks.",
            ),
        ),
        (
            "editor-lead",
            lead("Sam", "the editor", "Keeps its checks and reviews moving."),
        ),
        (
            "admin-lead",
            lead(
                "Ines",
                "the admin tool",
                "Plans its fixes and answers its team.",
            ),
        ),
        (
            "docs-lead",
            lead("Theo", "the docs", "Keeps the help guides current."),
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
        let daily = match *name {
            "shop" => 8.0,
            "editor" => 4.0,
            _ => 3.0,
        };
        config.push_str(&format!(
            "\n[[projects]]\nname = \"{name}\"\npath = \"{}\"\nintegration_branch = \"main\"\ndaily_usd = {daily}\n",
            path.display()
        ));
        if led {
            config.push_str(&format!("lead = \"{name}-lead\"\n"));
        }
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
    if !standups.is_empty() {
        let at = now_ms() - 40 * MINUTE;
        let kept: serde_json::Map<String, Value> = standups
            .iter()
            .map(|(project, text)| ((*project).to_string(), json!({"at": at, "text": text})))
            .collect();
        write(
            &paths.data_dir.join("talk/standups.json"),
            &serde_json::to_string_pretty(&kept)?,
        )?;
    }
    let studio = Studio::open(paths.clone(), brain.runtimes())?;
    Ok((paths, studio))
}

/// What the leads of the busy studio said of their projects.
const STANDUPS: &[(&str, &str)] = &[
    (
        "shop",
        "The logo is done. The product cards went back: nine colours is too many, so Designer is redoing them with three.",
    ),
    (
        "editor",
        "The new-item form is in. Undo and redo is in review.",
    ),
    (
        "admin",
        "v1.2 passed its final checks. It is ready to go out.",
    ),
    ("docs", "v1.0 is live. The new home page links every guide."),
];

const MINUTE: u64 = 60_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---- what the leads said ---------------------------------------------------

/// Writes the shop's conversation with its lead into its data folder: what
/// the owner asked and what Nora answered, with a plan and an action as
/// cards. What waits for the owner and what is ready to ship comes from the
/// studio itself, as the newest of it all.
fn seed_shop_talk(paths: &StudioPaths, ids: &Ids) -> Result<()> {
    let nora = || TalkFrom::Lead {
        hire: "shop-lead".into(),
    };
    let plan = TalkCard::Plan {
        goal: ids.checkout_goal.clone(),
        title: "Checkout flow".into(),
        version: Some("0.5".into()),
        tasks: [
            ("Count the items in the cart", "dev"),
            ("Add a cart drawer", "dev"),
            ("Validate the payment form", "dev"),
            ("Design the order confirmation page", "designer"),
        ]
        .iter()
        .map(|(title, hire)| PlanTask {
            title: (*title).into(),
            hire: Some((*hire).into()),
        })
        .collect(),
    };
    let told = TalkCard::Acted {
        task: ids.asking.clone(),
        title: "Add a cart drawer".into(),
        action: "told".into(),
        note: "Close the drawer when the shopper clicks outside it.".into(),
    };
    // (seconds ago, who, words, card): the first exchange is hours old, the
    // rest comes after everything the studio itself shows in the thread.
    let turns = [
        (
            11_400,
            TalkFrom::Owner,
            "How is the store look coming along?",
            None,
        ),
        (
            11_280,
            nora(),
            "The logo is done. The product cards went back: nine colours is too many, so Designer is redoing them with three. The launch email waits for your go-ahead.",
            None,
        ),
        (4, TalkFrom::Owner, "Plan the checkout flow for 0.5.", None),
        (
            3,
            nora(),
            "Here is the plan for \"Checkout flow\": 4 tasks. Dev starts with the cart.",
            Some(plan),
        ),
        (
            2,
            TalkFrom::Owner,
            "Tell Dev the cart drawer should close when you click outside it.",
            None,
        ),
        (
            1,
            nora(),
            "Done. Dev has it and will add it before handing the drawer over.",
            Some(told),
        ),
    ];
    let now = now_ms();
    let mut lines = Vec::new();
    for (n, (seconds_ago, from, text, card)) in turns.into_iter().enumerate() {
        lines.push(serde_json::to_string(&TalkMessage {
            id: n as u64 + 1,
            at: now - seconds_ago * 1000,
            from,
            text: text.into(),
            card,
        })?);
    }
    write(
        &paths.data_dir.join("talk/shop.jsonl"),
        &format!("{}\n", lines.join("\n")),
    )
}

// ---- projects being set up --------------------------------------------------

/// How far a project's setup through its lead has got.
#[derive(Clone, Copy, PartialEq)]
enum Stage {
    /// The lead is reading the repository.
    Reading,
    /// The studio is trying the checks the lead proposed.
    Checking,
    /// The lead's proposal waits for the owner's choice.
    Ready,
    /// The lead ended without a proposal.
    Failed,
    /// The owner took the proposal.
    Accepted,
}

/// The starting studio's projects, each with the name of its new lead and how
/// far its setup has got.
const SETUPS: [(&str, &str, Stage); 5] = [
    ("wiki", "Kit", Stage::Reading),
    ("ledger", "Ada", Stage::Checking),
    ("garden", "Mira", Stage::Ready),
    ("tracker", "Leo", Stage::Failed),
    ("journal", "Iris", Stage::Accepted),
];

const SETUP_FILES: &[(&str, &str)] = &[
    ("README.md", "A small web app, with tests.\n"),
    (
        "scripts/test.sh",
        "#!/bin/sh\necho \"running 112 tests\"\necho \"test result: ok. 112 passed; 0 failed\"\n",
    ),
    (
        "scripts/lint.sh",
        "#!/bin/sh\necho \"checked 48 files\"\necho \"clean\"\n",
    ),
    (
        "scripts/capture.sh",
        "#!/bin/sh\nmkdir -p \"$1\"\nfor n in 1 2 3 4; do echo page > \"$1/page-$n.txt\"; done\n",
    ),
    ("scripts/package.sh", PACKAGE_SCRIPT),
    ("scripts/upload.sh", PUBLISH_SCRIPT),
];

/// Tests that wait for the gallery to let them finish, as a run the studio is
/// still trying does.
const SLOW_SETUP_FILES: &[(&str, &str)] = &[
    ("README.md", "A small ledger app.\n"),
    (
        "scripts/test.sh",
        "#!/bin/sh\nwhile [ -e \"$(git rev-parse --path-format=absolute --git-common-dir)/hold\" ]; do sleep 0.2; done\necho \"test result: ok. 31 passed; 0 failed\"\n",
    ),
    (
        "scripts/lint.sh",
        "#!/bin/sh\necho \"checked 20 files\"\necho \"clean\"\n",
    ),
];

/// What the lead of `project` proposes. A `slow` project has no pictures and
/// no release.
fn proposal(project: &str, slow: bool) -> SetupProposal {
    let command = |args: &[&str], why: &str| ProposedCommand {
        command: args.iter().map(|a| (*a).to_string()).collect(),
        why: why.into(),
        tried: None,
    };
    let release = |script: &str| vec!["sh".to_string(), script.to_string()];
    SetupProposal {
        summary: format!(
            "{project} is a small web app with tests, a lint and a release script in its CI."
        ),
        verify: vec![
            command(
                &["sh", "scripts/test.sh"],
                "The tests, as your CI runs them.",
            ),
            command(&["sh", "scripts/lint.sh"], "The lint, as your CI runs it."),
        ],
        capture: (!slow).then(|| {
            command(
                &["sh", "scripts/capture.sh", "{out}"],
                "Takes a picture of each page.",
            )
        }),
        release: if slow {
            Vec::new()
        } else {
            vec![release("scripts/package.sh"), release("scripts/upload.sh")]
        },
        release_source: if slow {
            String::new()
        } else {
            "your CI".into()
        },
        protected: vec![
            ProtectedPath {
                glob: "saves/**".into(),
                why: "Holds what people saved.".into(),
            },
            ProtectedPath {
                glob: ".env*".into(),
                why: "Holds secrets.".into(),
            },
        ],
        daily_usd: Some(5.0),
        first_goal: Some(FirstGoal {
            text: "Fix the three crashes in TODO.md".into(),
            version: Some("0.9".into()),
            why: "They block every page.".into(),
        }),
    }
}

/// A studio whose projects were just added through their leads, one in each
/// state a setup can be in.
async fn starting(dir: &Path) -> Result<Live> {
    let brain = Brain::new();
    let (paths, studio) = home(dir, "starting", &[], brain.clone(), false, &[])?;
    let repos = dir.join("starting-repos");
    let driver = Driver::new(Arc::clone(&studio), brain.clone(), &[]);
    for (project, lead, stage) in SETUPS {
        let slow = stage == Stage::Checking;
        let files = if slow { SLOW_SETUP_FILES } else { SETUP_FILES };
        let path = repo(&repos, project, files, "")?;
        if slow {
            write(&path.join(".git/hold"), "")?;
        }
        // What the lead says at the end, and when it ends.
        let words = format!(
            "{project} is a small web app with tests, a lint and a release script in its CI. \
             I'd run the tests and the lint on every change, and ask you before anything \
             touches saved plans or secrets. Then I'd start on the three crashes in TODO.md."
        );
        let (runtime, hold) = match stage {
            Stage::Failed => (FakeRuntime::new(vec![reply("")]), None),
            _ => {
                let (runtime, hold) = FakeRuntime::new(vec![reply(&words)]).held();
                (runtime, Some(hold))
            }
        };
        lock(&brain.0).setups.insert(project.into(), runtime);
        driver
            .call(Request::SetupProject {
                path: path.display().to_string(),
                name: Some(project.into()),
                lead: None,
                new_lead: Some(NewLead {
                    name: lead.into(),
                    model: "opus".into(),
                }),
            })
            .await?;
        if matches!(stage, Stage::Checking | Stage::Ready | Stage::Accepted) {
            driver
                .call(Request::ProposeSetup {
                    project: project.into(),
                    proposal: proposal(project, slow),
                })
                .await?;
        }
        // Held for good while the lead is still reading.
        if stage != Stage::Reading
            && let Some(hold) = hold
        {
            hold.notify_one();
        }
        let until = match stage {
            Stage::Reading => None,
            Stage::Checking => Some(SetupState::Checking),
            Stage::Ready | Stage::Accepted => Some(SetupState::Ready),
            Stage::Failed => Some(SetupState::Failed),
        };
        if let Some(until) = until {
            driver.setup_reaches(project, until).await?;
        }
        if stage == Stage::Accepted {
            let command = |script: &str| vec!["sh".to_string(), script.to_string()];
            driver
                .call(Request::AcceptSetup {
                    project: project.into(),
                    choice: SetupChoice {
                        verify: vec![command("scripts/test.sh"), command("scripts/lint.sh")],
                        capture: vec!["sh".into(), "scripts/capture.sh".into(), "{out}".into()],
                        release: vec![command("scripts/package.sh"), command("scripts/upload.sh")],
                        protected: vec!["saves/**".into(), ".env*".into()],
                        daily_usd: Some(5.0),
                        plan_first: false,
                    },
                })
                .await?;
        }
    }
    Ok(Live {
        studio,
        data_dir: paths.data_dir,
    })
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
    /// What a lead says when it sets a project up, by project.
    setups: HashMap<String, FakeRuntime>,
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
            setups: HashMap::new(),
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
        let setting_up = prompt
            .strip_prefix("You are setting up the project ")
            .and_then(|rest| rest.split(' ').next());
        let fake = (if let Some(project) = setting_up {
            tables.setups.get(project).unwrap_or(&tables.pass)
        } else if prompt.contains("Decide whether the agent may make this tool call.") {
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
    /// Passes its checks and review and waits for the owner.
    Ready,
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
    /// What it leaves there when the work is sent back, if it is.
    revised: Vec<(&'static str, &'static str)>,
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
        revised: Vec::new(),
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

    fn revised(mut self, files: &[(&'static str, &'static str)]) -> Job {
        self.revised = files.to_vec();
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

    /// Waits until the setup of `project` is in `state`.
    async fn setup_reaches(&self, project: &str, state: SetupState) -> Result<()> {
        let want = serde_json::to_value(state)?;
        self.until(
            &format!("the setup of {project} to be {want}"),
            async || {
                let view = self
                    .call(Request::SetupStatus {
                        project: project.into(),
                    })
                    .await?;
                Ok(view["state"] == want)
            },
        )
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
        version: Option<&str>,
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
                version: version.map(str::to_string),
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
            Fate::Ready => {
                self.plant(project, id, files).await?;
                self.finish_until(id, "ready").await
            }
            Fate::Landed | Fate::Revised(_) => {
                self.plant(project, id, files).await?;
                self.finish_until(id, "ready").await?;
                if let Fate::Revised(notes) = job.fate {
                    // The agent's second run leaves the page with the change.
                    self.plant(project, id, &job.revised).await?;
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

    /// Releases `project` as `version`: starts it, waits for it to pass its
    /// final checks and wait for the owner, and, with `publish`, gives the
    /// go-ahead and waits for it to publish.
    async fn release(&mut self, project: &str, version: &str, publish: bool) -> Result<()> {
        self.call(Request::StartRelease {
            project: project.into(),
            version: version.into(),
        })
        .await?;
        self.until(
            &format!("{project} {version} to wait for you"),
            async || Ok(self.release_state(project).await? == "waiting"),
        )
        .await?;
        if publish {
            self.call(Request::ApproveRelease {
                project: project.into(),
            })
            .await?;
            self.until(&format!("{project} {version} to publish"), async || {
                Ok(self.release_state(project).await? == "released")
            })
            .await?;
        }
        Ok(())
    }

    /// The state of the project's newest release.
    async fn release_state(&self, project: &str) -> Result<String> {
        let all = self
            .call(Request::Releases {
                project: Some(project.into()),
            })
            .await?;
        let state = &all[0]["state"];
        if state == "failed" {
            return Err(format!("the {project} release failed: {}", all[0]["error"]).into());
        }
        Ok(state.as_str().unwrap_or_default().to_string())
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
                "Starting the store opens port 8080 on this machine, so it is yours to decide.\nQUESTION: Start the store on port 8080?\nDECISION: OWNER: starting the store opens port 8080 on this machine, so it is yours to decide",
            ),
            (
                "cargo run --bin mailer",
                "Sending mail reaches outside the project.\nQUESTION: Send the launch email?\nDECISION: OWNER: sending the launch email goes to a real address, so it is yours to decide",
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

        // Every project has a lead, who plans its goals from here on.
        for project in ["shop", "editor", "admin", "docs"] {
            self.call(Request::SetProjectLead {
                project: project.into(),
                hire: Some(format!("{project}-lead")),
            })
            .await?;
        }

        // The docs are live: their goal landed and merged, and the release
        // published.
        let (_, guides) = self
            .plan(
                "docs",
                "Give the help guides a new home page",
                Some("1.0"),
                vec![
                    job("Write the home page", "writer", Fate::Landed)
                        .files(&[("guides/home.md", "# Help\n")]),
                    job(
                        "Link every guide from the home page",
                        "writer",
                        Fate::Landed,
                    )
                    .files(&[("guides/index.md", "- Start here\n")]),
                ],
            )
            .await?;
        self.call(Request::Merge {
            ticket: guides[0].clone(),
        })
        .await?;
        self.release("docs", "1.0", true).await?;

        // The admin tool is releasing: its goal merged, its final checks
        // passed, and its release waits for the owner's go-ahead.
        let (_, staff) = self
            .plan(
                "admin",
                "Staff accounts: invite people, give them roles, log sign-ins",
                Some("1.2"),
                vec![
                    job("Invite staff by email", "dev", Fate::Landed)
                        .files(&[("src/invite.rs", "pub fn invite() {}\n")]),
                    job("Give staff roles", "dev", Fate::Landed)
                        .files(&[("src/roles.rs", "pub fn roles() {}\n")]),
                    job("Show the sign-in log", "dev", Fate::Landed)
                        .files(&[("src/log.rs", "pub fn log() {}\n")]),
                ],
            )
            .await?;
        self.call(Request::Merge {
            ticket: staff[0].clone(),
        })
        .await?;
        self.release("admin", "1.2", false).await?;

        // Work that is over, goal by goal.
        let (_, nav) = self
            .plan(
                "shop",
                "Polish the navigation",
                Some("0.3"),
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

        self.plan(
                "shop",
                "Give the store its look: logo, product cards, launch email",
                Some("0.4"),
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

        let (ready_goal, _) = self
            .plan(
                "shop",
                "Save carts between sessions",
                Some("0.4"),
                vec![
                    job("Store carts in the database", "dev", Fate::Landed)
                        .files(&[("src/carts.rs", CARTS_STORED)])
                        .usd(0.2),
                    job(
                        "Bring the cart back when a shopper signs in",
                        "dev",
                        Fate::Landed,
                    )
                    .files(&[("src/session.rs", SESSION_RESTORED)]),
                ],
            )
            .await?;

        let (checkout_goal, checkout) = self
            .plan(
                "shop",
                "Checkout flow",
                Some("0.5"),
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

        // Single tickets.
        self.single(
            "shop",
            "",
            job("Fix the broken footer links", "dev", Fate::Landed).files(&[(
                "templates/footer.html",
                "<a href=\"/contact\">Contact</a>\n",
            )]),
        )
        .await?;
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
        self.single(
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
                .revised(&[("src/landing.rs", LANDING_V3)])
                .usd(0.25),
            )
            .await?;

        // The editor is testing: one ticket landed, one in review, two waiting
        // behind it, and another ready for the owner.
        let (_, editor) = self
            .plan(
                "editor",
                "Ship a product editor: add items, undo mistakes, save drafts",
                Some("0.2"),
                vec![
                    job("Add a form for new items", "dev", Fate::Landed)
                        .files(&[("src/form.rs", "pub fn form() {}\n")])
                        .usd(0.2),
                    job("Add undo and redo", "dev", Fate::Reviewing)
                        .files(&[("src/undo.rs", "pub fn undo() {}\n")]),
                    job("Save drafts as JSON files", "dev", Fate::Queued),
                    job("Show how many drafts are saved", "dev", Fate::Queued),
                ],
            )
            .await?;
        self.single(
            "editor",
            "",
            job("Add keyboard shortcuts", "dev", Fate::Ready)
                .files(&[("src/keys.rs", "pub fn keys() {}\n")]),
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
        self.single(
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
        self.single(
            "shop",
            "",
            job("Add an order history page", "dev", Fate::Reviewing)
                .beats(&[Write("src/orders.rs"), Run("git diff --stat main...HEAD")])
                .files(&[("src/orders.rs", "pub fn history() {}\n")]),
        )
        .await?;
        self.plan(
            "shop",
            "Add a settings page for email and notification preferences",
            Some("0.5"),
            Vec::new(),
        )
        .await?;
        // Every desk is taken now, so this one waits.
        self.single("shop", "", job("Add a contact page", "dev", Fate::Queued))
            .await?;

        // The goal that is ready to ship has had its checks run on its branch.
        self.until("the checks of the ready goal", async || {
            let office = self.call(Request::Office).await?;
            Ok(office["ship_checks"][&ready_goal]["state"] == "passed")
        })
        .await?;

        let landed_merged = nav[0].clone();
        let landed = editor[0].clone();
        Ok(Ids {
            working,
            asking: checkout[1].clone(),
            ready,
            failed,
            ready_goal,
            checkout_goal,
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

const LANDING_V3: &str = "pub fn page(html: &mut Html, user: Option<&User>) {
    html.heading(\"Corner Shop\");
    html.link(\"Shop now\", \"/products\");
    if let Some(user) = user {
        html.text(&format!(\"Signed in as {}\", user.name));
        html.confirm(\"Sign out\", \"/sign-out\");
    }
}
";

// What the finished goal adds to the shop: the cart kept in the database and
// put back when a shopper signs in. New files only: the checkout of a worktree
// can still put back a file that exists when `plant` writes it.
const CARTS_STORED: &str = r#"use crate::cart::Cart;

/// Keeps a shopper's cart in the database, so it is there on the next visit.
pub fn save(db: &Db, shopper: ShopperId, cart: &Cart) -> Result<(), DbError> {
    db.put("carts", shopper, cart)
}

/// The cart a shopper left, or an empty one.
pub fn load(db: &Db, shopper: ShopperId) -> Cart {
    db.get("carts", shopper).unwrap_or_default()
}

/// Empties the cart a shopper left, once they have checked out.
pub fn clear(db: &Db, shopper: ShopperId) -> Result<(), DbError> {
    db.delete("carts", shopper)
}
"#;

const SESSION_RESTORED: &str = r#"use crate::carts;

/// Puts back the cart a shopper left when they sign in.
pub fn on_sign_in(db: &Db, shopper: ShopperId) -> Cart {
    carts::load(db, shopper)
}
"#;

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
