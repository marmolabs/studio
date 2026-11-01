use crate::api::{
    APPROVER_CALLS, AskView, CheckResult, Estimate, FileKind, FileView, HireDay, MAX_RUNS_LIMIT,
    ProjectCheck, RECENT_EVENTS, Request, Response, RuleFile,
};
use crate::budget::{self, DAY_MS, Reservation};
use crate::gc;
use crate::now_ms;
use crate::router::{self, Routed};
use crate::ruletest;
use crate::today;
use crate::trophies::{self, FirstMerge};
use crate::watch::CiWatcher;
use crate::worker;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use studio_config::config::ModelPrice;
use studio_config::config::{BudgetConfig, ConcurrencyConfig, ProjectEntry};
use studio_config::{
    ProjectConfig, Setting, StudioConfig, StudioPaths, check_hire, check_rule, load_hires,
    load_pack_hires, set_values,
};
use studio_core::budget::Allowance;
use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, TicketId};
use studio_core::policy;
use studio_core::quote::{QUOTE_SAMPLES, Quote, quote};
use studio_core::rule::Hire;
use studio_core::ticket::{HeldReason, Ticket, TicketKind, TicketState};
use studio_git::{LandOutcome, MergeOutcome, Repo};
use studio_runtime::Runtime;
use studio_store::Spend;
use studio_store::Store;
use thiserror::Error;
use tokio::sync::{Notify, oneshot};

#[derive(Debug, Error)]
pub enum StudioError {
    #[error(transparent)]
    Store(#[from] studio_store::StoreError),
    #[error(transparent)]
    Config(#[from] studio_config::ConfigError),
    #[error(transparent)]
    Git(#[from] studio_git::GitError),
    #[error(transparent)]
    Runtime(#[from] studio_runtime::RuntimeError),
    #[error("{0}")]
    Other(String),
}

pub(crate) fn other(message: impl Into<String>) -> StudioError {
    StudioError::Other(message.into())
}

/// `path` with a leading `~` turned into the home folder.
fn expand_home(path: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match (path.strip_prefix("~/"), path, home) {
        (Some(rest), _, Some(home)) => home.join(rest),
        (None, "~", Some(home)) => home,
        _ => std::path::PathBuf::from(path),
    }
}

struct NewTicket {
    project: String,
    title: String,
    goal: String,
    hire: String,
    base: Option<String>,
    kind: TicketKind,
    parent: Option<TicketId>,
    detail: String,
    source: Option<String>,
}

/// At most `max` characters of the first line, with an ellipsis if cut.
fn shorten(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// The first line of a skill's `description`, or of its text if it has none.
fn skill_summary(text: &str) -> String {
    let first = |s: &str| {
        s.lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    };
    let Ok(Some((yaml, body))) = studio_config::frontmatter::split(text) else {
        return first(text).unwrap_or_default();
    };
    let mut lines = yaml.lines();
    while let Some(line) = lines.next() {
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() && !value.starts_with(['>', '|']) {
                return value.to_string();
            }
            // A folded or block value starts on the next line.
            return lines
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .to_string();
        }
    }
    first(body).unwrap_or_default()
}

/// The owner's answer to an ask.
pub(crate) enum Decision {
    Allow,
    /// `stop` is false when the owner denied a permission with a note: the
    /// agent is told and carries on.
    Deny {
        message: String,
        stop: bool,
    },
    Answer(BTreeMap<String, String>),
}

/// Why the studio wants a live run to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    Cancel,
    Budget,
    /// The owner paused the ticket; it waits, held, until resumed.
    Pause,
}

/// Tells a ticket's worker to stop its run. Stored reasons survive until read,
/// and a cancel wins over a budget stop.
pub(crate) struct Stop {
    pub(crate) notify: Notify,
    reason: Mutex<Option<StopReason>>,
}

impl Stop {
    fn new() -> Stop {
        Stop {
            notify: Notify::new(),
            reason: Mutex::new(None),
        }
    }

    pub(crate) fn request(&self, reason: StopReason) {
        let mut current = lock(&self.reason);
        if *current != Some(StopReason::Cancel) {
            *current = Some(reason);
        }
        self.notify.notify_one();
    }

    pub(crate) fn reason(&self) -> Option<StopReason> {
        *lock(&self.reason)
    }
}

struct PendingAsk {
    view: AskView,
    reply: oneshot::Sender<Decision>,
}

pub struct Studio {
    pub paths: StudioPaths,
    config: Mutex<LiveConfig>,
    store: Mutex<Store>,
    hires: Mutex<HashMap<String, Hire>>,
    runtimes: HashMap<String, Arc<dyn Runtime>>,
    asks: Mutex<BTreeMap<String, PendingAsk>>,
    next_ask: Mutex<u64>,
    /// When budget-held tickets were last looked at for a resume.
    last_resume: Mutex<Option<std::time::Instant>>,
    /// Tickets with a live worker, with the signal that stops it.
    active: Mutex<HashMap<String, Arc<Stop>>>,
    /// Money held back for live runs, by ticket.
    reservations: Mutex<HashMap<String, Reservation>>,
    /// What each live run has spent so far by its meter, by ticket id, until
    /// the run's spend is written to the ledger.
    metered: Mutex<HashMap<String, f64>>,
    /// Whether a goal tip is in a base tip, as of the last `MergedTickets`.
    merged_tips: Mutex<HashMap<(String, String), bool>>,
    /// Held while `studio.toml` is rewritten, so two changes never mix.
    settings: Mutex<()>,
    wake: Notify,
    notifier: crate::notify::Notifier,
}

const MAX_FILE_BYTES: usize = 64 * 1024;

/// `studio.toml` as last read, and when to look at the file again.
struct LiveConfig {
    config: Arc<StudioConfig>,
    modified: Option<std::time::SystemTime>,
    checked: std::time::Instant,
}

/// How often held tickets are checked for a resume.
const RESUME_EVERY: Duration = Duration::from_secs(60);

/// How long a ticket held for the provider's quota waits to try again when
/// the provider didn't say when its limit resets.
const QUOTA_RETRY_MS: u64 = 15 * 60 * 1000;

/// When a ticket held for quota at `held_at` tries again: a minute after the
/// limit resets, if the provider said when, else `QUOTA_RETRY_MS` later.
pub(crate) fn quota_retry_at(held_at: u64, resets_at: Option<u64>) -> u64 {
    match resets_at {
        Some(reset) if reset > held_at => reset + 60_000,
        _ => held_at + QUOTA_RETRY_MS,
    }
}

/// How often the config file's time stamp is looked at.
const CONFIG_CHECK: Duration = Duration::from_secs(1);

/// Pack hires first, then the studio's own, so a studio hire overrides a pack's
/// hire with the same id.
fn load_all_hires(config: &StudioConfig, paths: &StudioPaths) -> HashMap<String, Hire> {
    let mut hires = HashMap::new();
    for (path, hire) in load_pack_hires(config).into_iter().chain(load_hires(paths)) {
        match hire {
            Ok(hire) => {
                hires.insert(hire.id.to_string(), hire);
            }
            Err(problem) => eprintln!("skipping hire {}: {problem}", path.display()),
        }
    }
    hires
}

/// File names are `[a-z0-9][a-z0-9_-]{0,63}`, which also rules out path tricks.
fn valid_name(name: &str) -> Result<(), StudioError> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(other(
            "name must be lowercase letters, digits, - or _, start with a letter or digit, and be at most 64 characters",
        ))
    }
}

fn check(kind: FileKind, name: &str, text: &str) -> Result<(), String> {
    match kind {
        FileKind::Hires => check_hire(name, text),
        FileKind::Rules => check_rule(name, text),
    }
}

/// The `*.md` files in a rules folder, sorted; none if it can't be read.
fn rule_paths(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    paths
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The config a studio without `studio.toml` runs with.
fn default_config() -> StudioConfig {
    StudioConfig {
        budget: BudgetConfig {
            daily_usd: 25.0,
            ..BudgetConfig::default()
        },
        concurrency: ConcurrencyConfig { max_runs: 2 },
        projects: Vec::new(),
        providers: HashMap::new(),
        rules: Default::default(),
        packs: Vec::new(),
        prices: HashMap::new(),
        goals: Default::default(),
        watchers: Default::default(),
        notify: Default::default(),
    }
}

/// The newest change to `studio.toml` or a hire file, so editing either on
/// disk reloads both. The hires folder's own time covers added and removed
/// files.
fn config_modified(paths: &StudioPaths) -> Option<std::time::SystemTime> {
    let time = |path: &std::path::Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let hires = paths.config_dir.join("hires");
    let files = std::fs::read_dir(&hires)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| time(&e.path()));
    [time(&paths.config_dir.join("studio.toml")), time(&hires)]
        .into_iter()
        .flatten()
        .chain(files)
        .max()
}

impl Studio {
    /// Loads `studio.toml` (defaults if it's missing), the hires and the store.
    /// `runtimes` maps a hire's `runtime` field to the runtime that runs it.
    pub fn open(
        paths: StudioPaths,
        runtimes: HashMap<String, Arc<dyn Runtime>>,
    ) -> Result<Arc<Studio>, StudioError> {
        let config = if paths.config_dir.join("studio.toml").exists() {
            StudioConfig::load(&paths)?
        } else {
            default_config()
        };
        let notifier = crate::notify::Notifier::new(config.notify.desktop);
        let hires = load_all_hires(&config, &paths);
        let store = Store::open(paths.data_dir.join("studio.db"))?;
        let modified = config_modified(&paths);
        let last_ask = store.max_ask_number()?;
        Ok(Arc::new(Studio {
            paths,
            config: Mutex::new(LiveConfig {
                config: Arc::new(config),
                modified,
                checked: std::time::Instant::now(),
            }),
            store: Mutex::new(store),
            hires: Mutex::new(hires),
            runtimes,
            asks: Mutex::new(BTreeMap::new()),
            next_ask: Mutex::new(last_ask),
            last_resume: Mutex::new(None),
            active: Mutex::new(HashMap::new()),
            reservations: Mutex::new(HashMap::new()),
            metered: Mutex::new(HashMap::new()),
            merged_tips: Mutex::new(HashMap::new()),
            settings: Mutex::new(()),
            wake: Notify::new(),
            notifier,
        }))
    }

    /// `studio.toml` as it is now. An edit takes effect within a second; a
    /// file that no longer loads keeps the last good config. Runtimes and
    /// notifications are set at start and need a restart.
    pub(crate) fn config(&self) -> Arc<StudioConfig> {
        self.refresh_config(false)
    }

    /// Reads `studio.toml` now rather than at the next check, after the
    /// studio itself changed it.
    fn reload_config(&self) -> Arc<StudioConfig> {
        self.refresh_config(true)
    }

    fn refresh_config(&self, force: bool) -> Arc<StudioConfig> {
        let mut live = lock(&self.config);
        if !force && live.checked.elapsed() < CONFIG_CHECK {
            return Arc::clone(&live.config);
        }
        live.checked = std::time::Instant::now();
        let modified = config_modified(&self.paths);
        if !force && modified == live.modified {
            return Arc::clone(&live.config);
        }
        live.modified = modified;
        match StudioConfig::load(&self.paths) {
            Ok(config) => {
                live.config = Arc::new(config);
                let config = Arc::clone(&live.config);
                drop(live);
                *lock(&self.hires) = load_all_hires(&config, &self.paths);
                // More room or money may let queued work start.
                self.wake.notify_one();
                eprintln!("studio.toml reloaded");
                config
            }
            Err(err) => {
                eprintln!("studio.toml: {err}; keeping the last good config");
                Arc::clone(&live.config)
            }
        }
    }

    pub fn socket_path(&self) -> std::path::PathBuf {
        crate::api::socket_path(&self.paths.data_dir)
    }

    /// After a restart nothing is running: hold every ticket that thought it
    /// was, so the board never shows work that isn't happening.
    pub fn recover(&self) -> Result<usize, StudioError> {
        let mut store = lock(&self.store);
        let mut stuck = store.in_flight()?;
        stuck.extend(store.tickets_in("asking")?);
        for ticket in &stuck {
            store.append(
                &ticket.id,
                now_ms(),
                &Event::Held {
                    reason: HeldReason::Paused,
                },
            )?;
        }
        Ok(stuck.len())
    }

    /// Starts workers whenever there's capacity, until the process ends.
    pub fn spawn_scheduler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let studio = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                if let Err(err) = studio.schedule() {
                    eprintln!("scheduler: {err}");
                }
                tokio::select! {
                    _ = studio.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        })
    }

    /// One scheduling pass: resume orphaned work first, then claim queued
    /// tickets. `max_runs = 0` or a closed office pauses the studio: nothing
    /// new starts.
    pub fn schedule(self: &Arc<Self>) -> Result<(), StudioError> {
        let capacity = if self.is_closed() {
            0
        } else {
            self.desks() as usize
        };
        let orphans: Vec<Ticket> = {
            let store = lock(&self.store);
            let active = lock(&self.active);
            store
                .in_flight()?
                .into_iter()
                .filter(|t| !active.contains_key(&t.id.to_string()))
                .collect()
        };
        for ticket in orphans {
            if lock(&self.active).len() >= capacity {
                return Ok(());
            }
            self.start_worker(ticket.id);
        }
        {
            let mut last = lock(&self.last_resume);
            if last.is_none_or(|at| at.elapsed() >= RESUME_EVERY) {
                *last = Some(std::time::Instant::now());
                drop(last);
                self.resume_budget_held()?;
                self.resume_quota_held(now_ms())?;
            }
        }
        while lock(&self.active).len() < capacity {
            if self.studio_budget_exhausted()? {
                break;
            }
            let Some(next) = self.next_queued()? else {
                break;
            };
            // Another claimer may have taken it; the next pass looks again.
            if let Some(ticket) = lock(&self.store).claim(&next, now_ms())? {
                self.start_worker(ticket.id);
            }
        }
        Ok(())
    }

    /// The oldest queued ticket whose goal has no other ticket in progress.
    /// A ticket with no goal is its own goal. A ticket its own worker sent
    /// back (a landing that conflicted) waits until that worker has ended.
    fn next_queued(&self) -> Result<Option<TicketId>, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let working: std::collections::HashSet<String> =
            lock(&self.active).keys().cloned().collect();
        let busy: std::collections::HashSet<(ProjectId, String)> = tickets
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    TicketState::Claimed
                        | TicketState::Working
                        | TicketState::Asking { .. }
                        | TicketState::Checking
                        | TicketState::Reviewing
                        | TicketState::Held { .. }
                        | TicketState::Ready
                )
            })
            .map(|t| (t.project.clone(), goal_of(t)))
            .collect();
        Ok(tickets
            .into_iter()
            .find(|t| {
                t.state == TicketState::Queued
                    && !busy.contains(&(t.project.clone(), goal_of(t)))
                    && !working.contains(&t.id.to_string())
            })
            .map(|t| t.id))
    }

    /// Resumes each ticket held for budget that a run would now be funded for.
    /// A ticket whose own cap is used up stays held. Returns how many resumed.
    pub fn resume_budget_held(&self) -> Result<usize, StudioError> {
        let held = lock(&self.store).tickets_in("held")?;
        let mut resumed = 0;
        for ticket in held {
            if ticket.state
                != (TicketState::Held {
                    reason: HeldReason::Budget,
                })
            {
                continue;
            }
            let Ok(hire) = self.hire(&ticket.hire) else {
                continue;
            };
            // A run would be refused for want of a price, whatever the money.
            let unpriced = self.price(hire.model_for(ticket.attempts)).is_none()
                && self
                    .runtime(&hire.runtime)
                    .is_ok_and(|r| !r.capabilities().reports_cost);
            if unpriced {
                continue;
            }
            let allowance = self.allowance(&ticket, &hire)?;
            if !matches!(allowance, Allowance::Run { .. }) {
                continue;
            }
            self.append(&ticket.id, Event::Resumed)?;
            crate::runlog::RunLog::new(&self.paths.data_dir, &ticket.id.to_string())
                .note("resumed: budget is available again");
            resumed += 1;
        }
        if resumed > 0 {
            self.wake.notify_one();
        }
        Ok(resumed)
    }

    /// Resumes each ticket held for the provider's quota whose wait is over
    /// at `now`; see `quota_retry_at`. Returns how many resumed.
    pub fn resume_quota_held(&self, now: u64) -> Result<usize, StudioError> {
        let (held, since) = {
            let store = lock(&self.store);
            (store.tickets_in("held")?, store.state_since()?)
        };
        let mut resumed = 0;
        for ticket in held {
            if ticket.state
                != (TicketState::Held {
                    reason: HeldReason::Quota,
                })
            {
                continue;
            }
            let Some(&held_at) = since.get(&ticket.id.to_string()) else {
                continue;
            };
            let log = crate::runlog::RunLog::new(&self.paths.data_dir, &ticket.id.to_string());
            if now < quota_retry_at(held_at, log.quota_resets_at()) {
                continue;
            }
            self.append(&ticket.id, Event::Resumed)?;
            log.note("resumed: trying the provider again");
            resumed += 1;
        }
        if resumed > 0 {
            self.wake.notify_one();
        }
        Ok(resumed)
    }

    fn start_worker(self: &Arc<Self>, id: TicketId) {
        let stop = Arc::new(Stop::new());
        lock(&self.active).insert(id.to_string(), Arc::clone(&stop));
        tokio::spawn(worker::work(Arc::clone(self), id, stop));
    }

    /// True when the studio-wide 24-hour budget can't fund another run, so
    /// queued tickets stay queued instead of being claimed and held.
    fn studio_budget_exhausted(&self) -> Result<bool, StudioError> {
        let budget = &self.config().budget;
        let spent = lock(&self.store)
            .spent_since(now_ms().saturating_sub(DAY_MS))?
            .total;
        let reserved: f64 = lock(&self.reservations).values().map(|r| r.usd).sum();
        let left = budget.daily_usd - spent - reserved;
        Ok(!studio_core::budget::funded(left, budget.min_run_usd))
    }

    /// What the ticket's next run with `hire` could spend, without reserving it.
    fn allowance(&self, ticket: &Ticket, hire: &Hire) -> Result<Allowance, StudioError> {
        let reservations = lock(&self.reservations);
        self.decide_allowance(ticket, hire, &reservations)
    }

    fn decide_allowance(
        &self,
        ticket: &Ticket,
        hire: &Hire,
        reservations: &HashMap<String, Reservation>,
    ) -> Result<Allowance, StudioError> {
        let project_day = self.project(&ticket.project)?.daily_usd;
        let inputs = budget::Inputs {
            budget: &self.config().budget,
            project_day,
            ticket,
            hire,
        };
        Ok(budget::decide(
            &lock(&self.store),
            &inputs,
            reservations,
            now_ms(),
        )?)
    }

    /// Reserves money for the ticket's next run, or says which limit refuses it.
    pub(crate) fn reserve(&self, ticket: &Ticket, hire: &Hire) -> Result<Allowance, StudioError> {
        let mut reservations = lock(&self.reservations);
        let decision = self.decide_allowance(ticket, hire, &reservations)?;
        if let Allowance::Run { usd, .. } = decision {
            reservations.insert(
                ticket.id.to_string(),
                Reservation {
                    project: ticket.project.0.clone(),
                    hire: hire.id.0.clone(),
                    usd,
                },
            );
        }
        Ok(decision)
    }

    /// Whether a small extra call costing `usd` fits under the studio, project
    /// and hire daily limits, counting live reservations.
    /// Whether `usd` more fits under the daily limits. `for_ticket` names the
    /// running ticket the spend is for, whose own reservation already covers it.
    pub(crate) fn has_headroom(
        &self,
        project: &str,
        hire: &Hire,
        usd: f64,
        for_ticket: &str,
    ) -> bool {
        let config = self.config();
        let project_day = config
            .projects
            .iter()
            .find(|p| p.name == project)
            .and_then(|p| p.daily_usd);
        // A call made for a running ticket is part of that run, so the run's
        // own reservation doesn't count against it.
        let mut reservations = lock(&self.reservations).clone();
        reservations.remove(for_ticket);
        budget::has_headroom(
            &lock(&self.store),
            &config.budget,
            (project, project_day),
            hire,
            &reservations,
            now_ms(),
            usd,
        )
        .unwrap_or(false)
    }

    /// Every loaded hire.
    pub(crate) fn all_hires(&self) -> Vec<Hire> {
        lock(&self.hires).values().cloned().collect()
    }

    /// What the ticket's live run has spent so far by its meter, which
    /// `TicketSpend` and `TicketSpends` count until the run's spend is recorded.
    pub(crate) fn meter(&self, ticket: &TicketId, usd: f64) {
        lock(&self.metered).insert(ticket.to_string(), usd);
    }

    /// Each loaded hire's id, role and model, by id.
    fn team(&self) -> Vec<serde_json::Value> {
        let mut hires: Vec<Hire> = lock(&self.hires).values().cloned().collect();
        hires.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        hires
            .iter()
            .map(|h| json!({ "id": h.id.0, "role": h.role, "model": h.model }))
            .collect()
    }

    pub(crate) fn release(&self, ticket: &TicketId) {
        lock(&self.reservations).remove(&ticket.to_string());
        lock(&self.metered).remove(&ticket.to_string());
        self.wake.notify_one();
    }

    /// The owner's rules as they are on disk right now, so edits apply to the
    /// next run without a restart.
    pub(crate) fn rules(&self) -> Vec<studio_core::rule::Rule> {
        let loaded = studio_config::load_rules(&self.config(), &self.paths);
        for problem in &loaded.problems {
            eprintln!("rule {}: {}", problem.path.display(), problem.message);
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let mut rules: Vec<_> = loaded
            .rules
            .into_iter()
            .map(|(_, mut rule)| {
                // `~/` in a path glob means the owner's home folder.
                if let Some(home) = &home {
                    for path in &mut rule.when.paths {
                        if let Some(rest) = path.strip_prefix("~/") {
                            *path = home.join(rest).to_string_lossy().into_owned();
                        }
                    }
                }
                rule
            })
            .collect();
        rules.extend(self.config().projects.iter().flat_map(project_rules));
        rules
    }

    pub(crate) fn price(&self, model: &str) -> Option<ModelPrice> {
        self.config().prices.get(model).cloned()
    }

    /// The model a session last ran on.
    pub(crate) fn session_model(&self, session: &str) -> Result<Option<String>, StudioError> {
        Ok(lock(&self.store).session_model(session)?)
    }

    /// What the hire's recent landed tickets cost and took, if there are any.
    pub(crate) fn quote(&self, hire: &str) -> Result<Option<Quote>, StudioError> {
        let samples = lock(&self.store).landed_samples(hire, QUOTE_SAMPLES)?;
        Ok(quote(&samples))
    }

    /// What the hire may still spend on the ticket: under the ticket's cap,
    /// and under the hire's own cap for its runs on it.
    pub(crate) fn ticket_left(&self, ticket: &Ticket, hire: &Hire) -> Result<f64, StudioError> {
        let store = lock(&self.store);
        let left = self.config().budget.ticket_usd - store.ticket_spent(&ticket.id)?;
        Ok(match hire.budget_per_ticket_usd {
            Some(cap) => left.min(cap - store.ticket_hire_spent(&ticket.id, &hire.id.0)?),
            None => left,
        })
    }

    /// A quote and expected finish for every open ticket. A running ticket
    /// finishes a median duration after it started, and not sooner than a
    /// minute from now; a waiting one a median duration from now.
    fn estimates(&self) -> Result<Vec<Estimate>, StudioError> {
        let now = now_ms();
        let mut quotes: HashMap<String, Option<Quote>> = HashMap::new();
        let mut out = Vec::new();
        let tickets = lock(&self.store).tickets()?;
        for ticket in tickets {
            use TicketState::*;
            if matches!(ticket.state, Landed | Cancelled | Failed { .. } | Ready) {
                continue;
            }
            let hire = ticket.hire.0.clone();
            if !quotes.contains_key(&hire) {
                quotes.insert(hire.clone(), self.quote(&hire)?);
            }
            let eta_ms = quotes[&hire].as_ref().map(|q| {
                let median = (q.minutes * 60_000.0) as u64;
                let running = matches!(ticket.state, Claimed | Working | Checking | Reviewing);
                let started = if running {
                    lock(&self.store).started_at(&ticket.id).ok().flatten()
                } else {
                    None
                };
                match started {
                    Some(at) => (at + median).max(now + 60_000),
                    None => now + median,
                }
            });
            out.push(Estimate {
                ticket: ticket.id.to_string(),
                quote_usd: quotes[&hire].as_ref().map(|q| q.usd),
                eta_ms,
            });
        }
        Ok(out)
    }

    pub(crate) fn session_spent(&self, session: &str) -> Result<f64, StudioError> {
        Ok(lock(&self.store).session_spent(session)?)
    }

    /// Records a run's cost, then stops every live run under a 24-hour limit
    /// that is now used up.
    pub(crate) fn record_spend(&self, spend: &Spend) -> Result<(), StudioError> {
        lock(&self.store).record_spend(spend)?;
        lock(&self.metered).remove(&spend.ticket_id.to_string());
        let spent = lock(&self.store).spent_since(now_ms().saturating_sub(DAY_MS))?;
        let budget = &self.config().budget;
        let over = |limit: Option<f64>, used: f64| limit.is_some_and(|l| used >= l);
        let reservations = lock(&self.reservations).clone();
        let active = lock(&self.active);
        for (ticket, r) in &reservations {
            let project_limit = self
                .config()
                .projects
                .iter()
                .find(|p| p.name == r.project)
                .and_then(|p| p.daily_usd);
            let hire_limit = lock(&self.hires).get(&r.hire).and_then(|h| h.daily_usd);
            let breached = spent.total >= budget.daily_usd
                || over(
                    project_limit,
                    spent.by_project.get(&r.project).copied().unwrap_or(0.0),
                )
                || over(
                    hire_limit,
                    spent.by_hire.get(&r.hire).copied().unwrap_or(0.0),
                );
            if breached && let Some(stop) = active.get(ticket) {
                stop.request(StopReason::Budget);
            }
        }
        Ok(())
    }

    pub(crate) fn worker_done(&self, id: &TicketId) {
        lock(&self.active).remove(&id.to_string());
        self.wake.notify_one();
    }

    pub(crate) fn append(&self, id: &TicketId, event: Event) -> Result<Ticket, StudioError> {
        let ticket = lock(&self.store).append(id, now_ms(), &event)?;
        self.notifier.observe(&ticket);
        Ok(ticket)
    }

    pub(crate) fn ticket(&self, id: &TicketId) -> Result<Ticket, StudioError> {
        lock(&self.store)
            .ticket(id)?
            .ok_or_else(|| other(format!("no ticket {id}")))
    }

    pub(crate) fn project(&self, name: &ProjectId) -> Result<ProjectEntry, StudioError> {
        self.config()
            .projects
            .iter()
            .find(|p| p.name == name.0)
            .cloned()
            .ok_or_else(|| other(format!("no project named {name} in studio.toml")))
    }

    pub(crate) fn hire(&self, id: &HireId) -> Result<Hire, StudioError> {
        lock(&self.hires)
            .get(&id.0)
            .cloned()
            .ok_or_else(|| other(format!("no hire named {id}")))
    }

    /// A hire with the `reviewer` role on none of the models `author` used over
    /// `failed` attempts, so no model grades its own work.
    /// A reviewer runs on a model other than the one that wrote this attempt:
    /// its own model, or else the first of its `escalate` models. Earlier
    /// attempts were reviewed when they were made, so only the latest counts.
    pub(crate) fn reviewer_for(&self, author: &Hire, failed: u32) -> Option<Hire> {
        let used = [author.model_for(failed)];
        let hires = lock(&self.hires);
        let mut reviewers: Vec<&Hire> = hires
            .values()
            .filter(|h| h.role == "reviewer" && h.id != author.id)
            .collect();
        reviewers.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        reviewers.into_iter().find_map(|h| {
            let model = std::iter::once(&h.model)
                .chain(&h.escalate)
                .find(|m| !used.contains(&m.as_str()))?;
            Some(Hire {
                model: model.clone(),
                escalate: Vec::new(),
                ..h.clone()
            })
        })
    }

    /// The critic a gate names, unless it would grade its own side: the ticket's
    /// own hire, or a hire with no model other than the one that wrote the latest attempt.
    pub(crate) fn gate_hire(&self, id: &str, author: &Hire, failed: u32) -> Result<Hire, String> {
        let critic = lock(&self.hires)
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no hire named {id}"))?;
        if critic.id == author.id {
            return Err(format!("{id} is the ticket's own hire"));
        }
        // Like the reviewer, fall back to the critic's escalate models when the
        // author used its own.
        // As for the reviewer, only the model that wrote the latest attempt counts.
        let used = [author.model_for(failed)];
        let Some(model) = std::iter::once(&critic.model)
            .chain(&critic.escalate)
            .find(|m| !used.contains(&m.as_str()))
        else {
            return Err(format!(
                "{id} runs on {}, which the author used",
                critic.model
            ));
        };
        Ok(Hire {
            model: model.clone(),
            escalate: Vec::new(),
            ..critic
        })
    }

    /// How many times the gate has sent this ticket back since the owner last
    /// restarted its work.
    pub(crate) fn gate_failures(&self, id: &TicketId, gate: &str) -> Result<u32, StudioError> {
        let prefix = format!("{gate} gate:");
        let mut failures = 0;
        for stored in lock(&self.store).events(id)? {
            match stored.event {
                Event::ReviewFailed { detail } if detail.starts_with(&prefix) => failures += 1,
                Event::Retried | Event::ChangesRequested { .. } => failures = 0,
                _ => {}
            }
        }
        Ok(failures)
    }

    pub(crate) fn runtime(&self, name: &str) -> Result<Arc<dyn Runtime>, StudioError> {
        self.runtimes
            .get(name)
            .cloned()
            .ok_or_else(|| other(format!("no runtime called {name}")))
    }

    /// Opens an ask and waits for the owner. The ticket shows as asking until then.
    pub(crate) async fn ask(
        &self,
        ticket: &TicketId,
        hire: &str,
        kind: &str,
        tool: &str,
        input: serde_json::Value,
        reasons: Vec<String>,
    ) -> Result<Decision, StudioError> {
        let id = {
            let mut next = lock(&self.next_ask);
            *next += 1;
            format!("a{}", *next)
        };
        let (reply, answer) = oneshot::channel();
        self.append(
            ticket,
            Event::AskOpened {
                ask: studio_core::ids::AskId::new(&id),
            },
        )?;
        let view = AskView {
            id: id.clone(),
            ticket: ticket.to_string(),
            hire: hire.to_string(),
            kind: kind.to_string(),
            tool: tool.to_string(),
            input,
            reasons,
            at_ms: now_ms(),
        };
        lock(&self.asks).insert(id.clone(), PendingAsk { view, reply });
        let decision = answer.await.unwrap_or_else(|_| Decision::Deny {
            message: "the ask was withdrawn".to_string(),
            stop: true,
        });
        self.append(
            ticket,
            Event::AskResolved {
                ask: studio_core::ids::AskId::new(&id),
                approved: !matches!(decision, Decision::Deny { stop: true, .. }),
            },
        )?;
        Ok(decision)
    }

    pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let (request, routed) = match self.route(request).await {
            Ok(done) => done,
            Err(err) => return Response::err(err),
        };
        let response = self.handle_routed(request).await;
        if let (Some(routed), true) = (routed, response.ok)
            && let Some(id) = response.data["id"].as_str()
        {
            let id = TicketId::new(id);
            crate::runlog::RunLog::new(&self.paths.data_dir, &id.0)
                .note(&format!("routed to {}: {}", routed.hire, routed.reason));
            if let Some(reply) = &routed.reply {
                // The ticket is made; a ledger error must not undo it.
                let _ = self.record_spend(&reply.charge(&id));
            }
        }
        response
    }

    /// Fills in the hire of a ticket request that names none, by asking the
    /// router (or taking the first engineer).
    async fn route(
        self: &Arc<Self>,
        request: Request,
    ) -> Result<(Request, Option<Routed>), StudioError> {
        match request {
            Request::CreateTicket {
                project,
                title,
                goal,
                hire,
                base,
                detail,
            } if hire.trim().is_empty() => {
                let entry = self.project(&ProjectId::new(&project))?;
                let routed = router::pick(self, &project, &entry.path, &title, &detail).await?;
                let request = Request::CreateTicket {
                    project,
                    title,
                    goal,
                    hire: routed.hire.clone(),
                    base,
                    detail,
                };
                Ok((request, Some(routed)))
            }
            Request::FileTicket {
                ticket,
                title,
                detail,
                hire,
            } if hire.as_deref().is_none_or(|h| h.trim().is_empty()) => {
                // A parent that can't file anyway gets its error from dispatch.
                let parent = self
                    .ticket(&TicketId::new(&ticket))
                    .ok()
                    .filter(|t| t.state == TicketState::Working);
                let mut routed = None;
                let mut chosen = None;
                if let Some(parent) = parent
                    && let Ok(entry) = self.project(&parent.project)
                {
                    let found =
                        router::pick(self, &parent.project.0, &entry.path, &title, &detail).await?;
                    chosen = Some(found.hire.clone());
                    routed = Some(found);
                }
                Ok((
                    Request::FileTicket {
                        ticket,
                        title,
                        detail,
                        hire: chosen,
                    },
                    routed,
                ))
            }
            other => Ok((other, None)),
        }
    }

    async fn handle_routed(self: &Arc<Self>, request: Request) -> Response {
        if let Request::AskOwner {
            ticket,
            question,
            options,
        } = request
        {
            return match self.ask_owner(&ticket, &question, options).await {
                Ok(text) => Response::ok(text),
                Err(err) => Response::err(err),
            };
        }
        match self.dispatch(request) {
            Ok(response) => response,
            Err(err) => Response::err(err),
        }
    }

    fn dispatch(self: &Arc<Self>, request: Request) -> Result<Response, StudioError> {
        match request {
            Request::Status => {
                let tickets = lock(&self.store).tickets()?;
                let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
                for ticket in &tickets {
                    *by_state.entry(ticket.state.kind()).or_default() += 1;
                }
                let goals = tickets
                    .iter()
                    .filter(|t| t.kind == TicketKind::Plan)
                    .count();
                Ok(Response::ok(json!({
                    "tickets": by_state,
                    "goals": goals,
                    "rules": self.rule_files(),
                    "running": lock(&self.active).len(),
                    "spent_24h": lock(&self.store).spent_since(now_ms().saturating_sub(DAY_MS))?.total,
                    "daily_usd": self.config().budget.daily_usd,
                    "out_of_budget": self.studio_budget_exhausted()?,
                    "asks": lock(&self.asks).len(),
                    "projects": self.config().projects.iter().map(|p| &p.name).collect::<Vec<_>>(),
                    "hires": lock(&self.hires).keys().collect::<Vec<_>>(),
                    "max_runs": self.desks(),
                    "closed": self.is_closed(),
                    "team": self.team(),
                })))
            }
            Request::CreateTicket {
                project,
                title,
                goal,
                hire,
                base,
                detail,
            } => {
                let ticket = self.create_ticket(NewTicket {
                    project,
                    title,
                    goal,
                    hire,
                    base,
                    kind: TicketKind::Work,
                    parent: None,
                    detail,
                    source: None,
                })?;
                Ok(Response::ok(ticket))
            }
            Request::CreateGoal {
                project,
                text,
                hire,
                detail,
            } => {
                let hire = match hire {
                    Some(hire) => hire,
                    None => {
                        self.first_with_role("producer")
                            .ok_or_else(|| other("no hire has the producer role; pass --hire"))?
                            .id
                            .0
                    }
                };
                let ticket = self.create_ticket(NewTicket {
                    project,
                    title: format!("Plan: {}", shorten(&text, 60)),
                    goal: text,
                    hire,
                    base: None,
                    kind: TicketKind::Plan,
                    parent: None,
                    detail,
                    source: None,
                })?;
                Ok(Response::ok(ticket))
            }
            Request::ReadRules { ticket } => {
                let ticket = self.ticket(&TicketId::new(&ticket))?;
                let hire = self.hire(&ticket.hire)?;
                let rules = self.rules();
                Ok(Response::ok(policy::guidance(
                    &rules,
                    &worker::context(&ticket, &hire),
                )))
            }
            Request::LoadSkill { name, .. } => Ok(Response::ok(self.load_skill(name.as_deref())?)),
            Request::FileTicket {
                ticket,
                title,
                detail,
                hire,
            } => {
                let filed = self.file_ticket(&TicketId::new(&ticket), title, detail, hire)?;
                Ok(Response::ok(filed))
            }
            Request::AskOwner { .. } => Err(other("ask_owner is handled before dispatch")),
            Request::Tickets => Ok(Response::ok(lock(&self.store).tickets()?)),
            Request::Events {
                since,
                limit,
                ticket,
            } => {
                let store = lock(&self.store);
                let events = match ticket {
                    Some(ticket) => store
                        .events(&TicketId::new(&ticket))?
                        .into_iter()
                        .filter(|e| e.seq > since)
                        .take(limit)
                        .collect(),
                    None => store.events_since(since, limit)?,
                };
                Ok(Response::ok(events))
            }
            Request::RecentEvents {
                since_ms,
                limit,
                kinds,
            } => {
                let limit = limit.map_or(RECENT_EVENTS, |l| l.min(RECENT_EVENTS));
                let keep = |e: &Event| kinds.is_empty() || kinds.iter().any(|k| k == e.kind());
                Ok(Response::ok(
                    lock(&self.store).events_from(since_ms, limit, keep)?,
                ))
            }
            Request::Activity { since_ms } => {
                Ok(Response::ok(lock(&self.store).activity_since(since_ms)?))
            }
            Request::StateSince => Ok(Response::ok(lock(&self.store).state_since()?)),
            Request::Diffstat { ticket } => Ok(Response::ok(self.stat(&TicketId::new(&ticket))?)),
            Request::Diffstats { tickets } => {
                let stats: BTreeMap<String, Option<String>> = tickets
                    .into_iter()
                    .map(|id| {
                        let stat = self.stat(&TicketId::new(&id)).ok().flatten();
                        (id, stat)
                    })
                    .collect();
                Ok(Response::ok(stats))
            }
            Request::Diff { ticket } => self.diff(&TicketId::new(&ticket)),
            Request::Evidence { ticket } => self.evidence(&TicketId::new(&ticket)),
            Request::Gc => self.gc(),
            Request::Asks => {
                let asks: Vec<AskView> =
                    lock(&self.asks).values().map(|a| a.view.clone()).collect();
                Ok(Response::ok(asks))
            }
            Request::Decide {
                ask,
                allow,
                message,
                answers,
            } => {
                let pending = lock(&self.asks)
                    .remove(&ask)
                    .ok_or_else(|| other(format!("no open ask {ask}")))?;
                let decision = match (allow, answers) {
                    (false, _) => {
                        let note = message.filter(|m| !m.trim().is_empty());
                        Decision::Deny {
                            // Only a permission ask can go on after a denial.
                            stop: note.is_none() || pending.view.kind != "permission",
                            message: note.unwrap_or_else(|| "Denied by the owner".into()),
                        }
                    }
                    (true, Some(answers)) => Decision::Answer(answers),
                    (true, None) => Decision::Allow,
                };
                let _ = pending.reply.send(decision);
                Ok(Response::ok(json!({ "ask": ask })))
            }
            Request::Land { ticket } => {
                let id = TicketId::new(&ticket);
                match self.land(&id)? {
                    Landing::Landed {
                        ticket,
                        branch,
                        sha,
                    } => Ok(Response::ok(
                        json!({ "ticket": ticket, "branch": branch, "sha": sha }),
                    )),
                    Landing::SentBack { branch, files } => Err(other(format!(
                        "{id} conflicts with {branch} in: {}; it went back to its agent to resolve them",
                        files.join(", ")
                    ))),
                }
            }
            Request::Merge { ticket } => self.merge(&TicketId::new(&ticket)),
            Request::MergedTickets => Ok(Response::ok(self.merged_tickets()?)),
            Request::Cancel { ticket } => {
                let id = TicketId::new(&ticket);
                let updated = self.append(&id, Event::Cancelled)?;
                lock(&self.asks).retain(|_, a| a.view.ticket != ticket);
                if let Some(stop) = lock(&self.active).get(&ticket) {
                    stop.request(StopReason::Cancel);
                }
                Ok(Response::ok(updated))
            }
            Request::Retry { ticket } => self.simple(&ticket, Event::Retried),
            Request::RequestChanges { ticket, feedback } => {
                self.simple(&ticket, Event::ChangesRequested { detail: feedback })
            }
            Request::Resume { ticket } => self.simple(&ticket, Event::Resumed),
            Request::Pause { ticket } => self.pause(&ticket),
            Request::TicketSpend { ticket } => {
                let spent = lock(&self.store).ticket_spent(&TicketId::new(&ticket))?;
                let live = lock(&self.metered).get(&ticket).copied().unwrap_or(0.0);
                Ok(Response::ok(spent + live))
            }
            Request::TicketSpends => {
                let mut spent = lock(&self.store).spent_by_ticket()?;
                for (ticket, usd) in lock(&self.metered).iter() {
                    *spent.entry(ticket.clone()).or_default() += usd;
                }
                Ok(Response::ok(spent))
            }
            Request::UpdatedAt => Ok(Response::ok(lock(&self.store).updated_at()?)),
            Request::Files { kind } => self.files(kind),
            Request::RuleFiles => Ok(Response::ok(self.rule_file_list())),
            Request::HiresToday => self.hires_today(),
            Request::ApproverCalls { limit } => {
                let since = now_ms().saturating_sub(DAY_MS);
                let runs = self.paths.data_dir.join("runs");
                let limit = limit.min(APPROVER_CALLS);
                Ok(Response::ok(today::approver_calls(&runs, since, limit)))
            }
            Request::SaveFile { kind, name, text } => self.save_file(kind, &name, &text),
            Request::DeleteFile { kind, name } => self.delete_file(kind, &name),
            Request::TestRule { text, since } => {
                let tickets = lock(&self.store).tickets()?;
                Ok(Response::ok(ruletest::run(self, &tickets, &text, since)?))
            }
            Request::Estimates => Ok(Response::ok(self.estimates()?)),
            Request::SetOpen { open } => self.set_open(open),
            Request::SetMaxRuns { max_runs } => self.set_max_runs(max_runs),
            Request::SetLimits {
                daily_usd,
                ticket_usd,
            } => self.set_limits(daily_usd, ticket_usd),
            Request::Trophies => self.trophies(),
            Request::ProjectChecks => self.project_checks(),
            Request::AddProject { path, name, branch } => self.add_project(&path, name, branch),
            Request::SpendByDay { days } => {
                let store = lock(&self.store);
                Ok(Response::ok(budget::by_day(&store, now_ms(), days)?))
            }
            Request::Budget => {
                let since = now_ms().saturating_sub(DAY_MS);
                let store = lock(&self.store);
                let spent = store.spent_since(since)?;
                let runs = store.ledger_since(since)?.len();
                let reservations: BTreeMap<String, f64> = lock(&self.reservations)
                    .iter()
                    .map(|(t, r)| (t.clone(), r.usd))
                    .collect();
                let budget = &self.config().budget;
                Ok(Response::ok(json!({
                    "limits": {
                        "daily_usd": budget.daily_usd,
                        "ticket_usd": budget.ticket_usd,
                        "run_usd": budget.run_usd,
                        "min_run_usd": budget.min_run_usd,
                        "projects": self.config().projects.iter()
                            .filter_map(|p| p.daily_usd.map(|d| (p.name.clone(), d)))
                            .collect::<BTreeMap<_, _>>(),
                        "hires": lock(&self.hires).values()
                            .filter_map(|h| h.daily_usd.map(|d| (h.id.0.clone(), d)))
                            .collect::<BTreeMap<_, _>>(),
                        "hires_ticket": lock(&self.hires).values()
                            .filter_map(|h| h.budget_per_ticket_usd.map(|d| (h.id.0.clone(), d)))
                            .collect::<BTreeMap<_, _>>(),
                    },
                    "spent_24h": spent,
                    "runs_24h": runs,
                    "reserved": reservations,
                })))
            }
        }
    }

    /// The file whose presence means the owner closed the office.
    fn closed_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("closed")
    }

    /// Whether the owner closed the office: nothing new starts.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed_path().exists()
    }

    fn set_open(&self, open: bool) -> Result<Response, StudioError> {
        let path = self.closed_path();
        if open {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(other(format!("could not reopen the office: {e}"))),
            }
            // Queued work may start now.
            self.wake.notify_one();
        } else {
            std::fs::create_dir_all(&self.paths.data_dir)
                .and_then(|()| std::fs::write(&path, "closed by the owner\n"))
                .map_err(|e| other(format!("could not close the office: {e}")))?;
        }
        Ok(Response::ok(json!({ "closed": self.is_closed() })))
    }

    /// Writes studio.toml values, then reads the file back so they apply now.
    fn change_settings(
        &self,
        changes: &[(&str, &str, Setting)],
    ) -> Result<Arc<StudioConfig>, StudioError> {
        let _writing = lock(&self.settings);
        set_values(&self.paths, changes).map_err(|e| other(e.to_string()))?;
        Ok(self.reload_config())
    }

    /// Adds a repository as a project, as `studio add` does, and applies it
    /// at once.
    fn add_project(
        &self,
        path: &str,
        name: Option<String>,
        branch: Option<String>,
    ) -> Result<Response, StudioError> {
        let path = expand_home(path.trim());
        if !path.is_dir() {
            return Err(other(format!("{} is not a folder", path.display())));
        }
        let repo = studio_git::Repo::open(&path)?;
        let branch = match branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
        {
            Some(branch) => branch,
            None => repo
                .current_branch()?
                .ok_or_else(|| other("that repository has no branch checked out; name one"))?,
        };
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let added = {
            let _writing = lock(&self.settings);
            let added = studio_config::add_project(
                &self.paths,
                studio_config::NewProject {
                    path: repo.root(),
                    name: name.as_deref(),
                    integration_branch: Some(&branch),
                    daily_usd: None,
                    pack: None,
                },
            )
            .map_err(|e| other(e.to_string()))?;
            self.reload_config();
            added
        };
        self.wake.notify_one();
        Ok(Response::ok(json!({
            "name": added.name,
            "path": added.path,
            "branch": branch,
            "checks": added.created_checks,
        })))
    }

    fn set_max_runs(&self, max_runs: u64) -> Result<Response, StudioError> {
        if !(1..=MAX_RUNS_LIMIT).contains(&max_runs) {
            return Err(other(format!(
                "max_runs must be from 1 to {MAX_RUNS_LIMIT}, not {max_runs}"
            )));
        }
        let config =
            self.change_settings(&[("concurrency", "max_runs", Setting::Whole(max_runs))])?;
        // More desks may let queued work start.
        self.wake.notify_one();
        Ok(Response::ok(
            json!({ "max_runs": config.concurrency.max_runs }),
        ))
    }

    fn set_limits(
        &self,
        daily_usd: Option<f64>,
        ticket_usd: Option<f64>,
    ) -> Result<Response, StudioError> {
        let mut changes = Vec::new();
        for (key, value) in [("daily_usd", daily_usd), ("ticket_usd", ticket_usd)] {
            let Some(usd) = value else {
                continue;
            };
            if !usd.is_finite() || usd <= 0.0 {
                return Err(other(format!("{key} must be more than zero, not {usd}")));
            }
            changes.push(("budget", key, Setting::Decimal(usd)));
        }
        if changes.is_empty() {
            return Err(other("give a daily_usd or a ticket_usd to change"));
        }
        let config = self.change_settings(&changes)?;
        self.wake.notify_one();
        Ok(Response::ok(json!({
            "daily_usd": config.budget.daily_usd,
            "ticket_usd": config.budget.ticket_usd,
        })))
    }

    /// How many agents work at once: `max_runs`, up to `MAX_RUNS_LIMIT`.
    fn desks(&self) -> u64 {
        u64::from(self.config().concurrency.max_runs).min(MAX_RUNS_LIMIT)
    }

    fn first_merge_path(&self) -> std::path::PathBuf {
        self.paths.data_dir.join("first_merge.json")
    }

    /// Notes the first goal merged through the studio, once. The history
    /// doesn't say when a merge happened, so this is where the date comes from;
    /// a studio that merged goals before it kept this note never gets one.
    fn remember_first_merge(&self, goal: &str, into: &str) {
        let path = self.first_merge_path();
        if path.exists() {
            return;
        }
        let first = FirstMerge {
            at_ms: now_ms(),
            goal: goal.to_string(),
            into: into.to_string(),
        };
        let written = serde_json::to_string(&first)
            .map_err(|e| e.to_string())
            .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
        if let Err(err) = written {
            eprintln!("could not note the first merge: {err}");
        }
    }

    fn first_merge(&self) -> Option<FirstMerge> {
        let text = std::fs::read_to_string(self.first_merge_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn trophies(&self) -> Result<Response, StudioError> {
        let (landings, today) = {
            let store = lock(&self.store);
            (store.landings()?, store.local_date(now_ms())?)
        };
        Ok(Response::ok(trophies::build(
            &landings,
            &today,
            self.first_merge().as_ref(),
        )))
    }

    fn project_checks(&self) -> Result<Response, StudioError> {
        let (tickets, latest) = {
            let store = lock(&self.store);
            (store.tickets()?, store.latest_checks()?)
        };
        let checks: Vec<ProjectCheck> =
            self.config()
                .projects
                .iter()
                .map(|project| ProjectCheck {
                    project: project.name.clone(),
                    verify: ProjectConfig::load(&project.path)
                        .ok()
                        .flatten()
                        .map(|config| config.verify)
                        .unwrap_or_default(),
                    running: tickets
                        .iter()
                        .filter(|t| t.project.0 == project.name && t.state == TicketState::Checking)
                        .map(|t| t.id.to_string())
                        .collect(),
                    latest: latest.iter().find(|c| c.project == project.name).map(|c| {
                        CheckResult {
                            ticket: c.ticket_id.to_string(),
                            passed: c.passed,
                            at_ms: c.at_ms,
                        }
                    }),
                })
                .collect();
        Ok(Response::ok(checks))
    }

    /// `git diff --stat` of a ticket's worktree, or `None` when it is gone.
    fn stat(&self, id: &TicketId) -> Result<Option<String>, StudioError> {
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        Ok(match worker::find_worktree(&repo, id)? {
            Some(worktree) => Some(worktree.diff_stat()?),
            None => None,
        })
    }

    fn diff(&self, id: &TicketId) -> Result<Response, StudioError> {
        const MAX_DIFF: usize = 1024 * 1024;
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let diff = match worker::find_worktree(&repo, id)? {
            Some(worktree) => {
                let mut diff = worktree.diff()?;
                if diff.len() > MAX_DIFF {
                    let cut = diff.floor_char_boundary(MAX_DIFF);
                    diff.truncate(cut);
                    diff.push_str("\n… (diff cut at 1 MB)\n");
                }
                Some(diff)
            }
            None => None,
        };
        Ok(Response::ok(diff))
    }

    fn evidence(&self, id: &TicketId) -> Result<Response, StudioError> {
        let ticket = self.ticket(id)?;
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let files = match worker::find_worktree(&repo, id)? {
            Some(worktree) => crate::evidence::all(&worktree.path),
            None => crate::evidence::list_kept(&self.paths.data_dir, &id.to_string()),
        };
        Ok(Response::ok(files))
    }

    fn gc(&self) -> Result<Response, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let mut removed = Vec::new();
        for project in &self.config().projects {
            let mine: Vec<Ticket> = tickets
                .iter()
                .filter(|t| t.project.0 == project.name)
                .cloned()
                .collect();
            let repo = match Repo::open(&project.path) {
                Ok(repo) => repo,
                Err(err) => {
                    removed.push(format!("{}: skipped, {err}", project.name));
                    continue;
                }
            };
            removed.extend(gc::collect(&repo, &mine)?);
        }
        Ok(Response::ok(removed))
    }

    /// Files a ticket for the first engineer, remembering where it came from.
    pub(crate) fn file_ci_ticket(
        &self,
        project: &str,
        title: String,
        detail: String,
        source: String,
    ) -> Result<Ticket, StudioError> {
        let hire = self
            .first_with_role("engineer")
            .ok_or_else(|| other("no hire has the engineer role"))?;
        self.create_ticket(NewTicket {
            project: project.to_string(),
            title,
            goal: String::new(),
            hire: hire.id.0,
            base: None,
            kind: TicketKind::Work,
            parent: None,
            detail,
            source: Some(source),
        })
    }

    /// Tickets of the project that the CI watcher filed.
    pub(crate) fn ci_tickets(&self, project: &str) -> Result<Vec<Ticket>, StudioError> {
        Ok(lock(&self.store)
            .tickets()?
            .into_iter()
            .filter(|t| t.project.0 == project)
            .filter(|t| t.source.as_deref().is_some_and(|s| s.starts_with("ci:")))
            .collect())
    }

    /// Checks CI for the watched projects now and then, until the process ends.
    /// Returns None when no project asks for it.
    pub fn spawn_ci_watcher(
        self: &Arc<Self>,
        watcher: CiWatcher,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if !self.config().projects.iter().any(|p| p.watch_ci) {
            return None;
        }
        let studio = Arc::clone(self);
        let every = self.config().watchers.ci_interval();
        Some(tokio::spawn(async move {
            loop {
                watcher.tick(&studio).await;
                tokio::time::sleep(every).await;
            }
        }))
    }

    pub(crate) fn watched_projects(&self) -> Vec<ProjectEntry> {
        self.config()
            .projects
            .iter()
            .filter(|p| p.watch_ci)
            .cloned()
            .collect()
    }

    /// How many rule files the studio loads: the ones `RuleFiles` lists. It
    /// only lists the folders, since `Status` asks for it every second.
    fn rule_files(&self) -> usize {
        self.rule_folders()
            .iter()
            .map(|(_, _, dir)| rule_paths(dir).len())
            .sum()
    }

    /// Where rules live: the studio's folder, each pack's, each project's,
    /// as (origin, owner, folder).
    fn rule_folders(&self) -> Vec<(&'static str, String, std::path::PathBuf)> {
        let config = self.config();
        let mut folders = vec![(
            "studio",
            String::new(),
            self.paths.config_dir.join(FileKind::Rules.dir()),
        )];
        folders.extend(
            config
                .packs
                .iter()
                .map(|p| ("pack", p.name.clone(), p.path.join("rules"))),
        );
        folders.extend(config.projects.iter().map(|p| {
            let dir = p.path.join(".studio").join("rules");
            ("project", p.name.clone(), dir)
        }));
        folders
    }

    /// Every rule file, in the order `load_rules` reads them.
    fn rule_file_list(&self) -> Vec<RuleFile> {
        let config = self.config();
        let loaded = studio_config::load_rules(&config, &self.paths);
        let mut files = Vec::new();
        for (origin, owner, dir) in self.rule_folders() {
            for path in rule_paths(&dir) {
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let (text, problem) = match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let problem = loaded.problems.iter().find(|p| p.path == path);
                        (text, problem.map(|p| p.message.clone()))
                    }
                    Err(e) => (String::new(), Some(format!("read error: {e}"))),
                };
                let id = format!("pack/{owner}/{name}");
                files.push(RuleFile {
                    origin: origin.to_string(),
                    disabled: origin == "pack" && config.rules.disable.contains(&id),
                    owner: owner.clone(),
                    name: name.to_string(),
                    path: path.to_string_lossy().into_owned(),
                    text,
                    problem,
                });
            }
        }
        files
    }

    /// What each hire did in the last 24 hours.
    fn hires_today(&self) -> Result<Response, StudioError> {
        let since = now_ms().saturating_sub(DAY_MS);
        let (tickets, events) = {
            let store = lock(&self.store);
            (
                store.tickets()?,
                store.events_from(since, usize::MAX, |_| true)?,
            )
        };
        let mut days: BTreeMap<String, HireDay> = BTreeMap::new();
        today::from_events(&tickets, &events, &mut days);
        let notes = today::notes_since(&self.paths.data_dir.join("runs"), since);
        let approver = self.first_with_role("approver").map(|h| h.id.0);
        let router = self.first_with_role("router").map(|h| h.id.0);
        today::from_notes(&notes, approver.as_deref(), router.as_deref(), &mut days);
        Ok(Response::ok(days))
    }

    fn files(&self, kind: FileKind) -> Result<Response, StudioError> {
        let dir = self.paths.config_dir.join(kind.dir());
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let (text, problem) = match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let problem = check(kind, name, &text).err();
                        (text, problem)
                    }
                    Err(e) => (String::new(), Some(format!("read error: {e}"))),
                };
                files.push(FileView {
                    name: name.to_string(),
                    text,
                    problem,
                });
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Response::ok(files))
    }

    fn save_file(&self, kind: FileKind, name: &str, text: &str) -> Result<Response, StudioError> {
        valid_name(name)?;
        if text.len() > MAX_FILE_BYTES {
            return Err(other("file is over 64 KiB"));
        }
        check(kind, name, text).map_err(other)?;
        let dir = self.paths.config_dir.join(kind.dir());
        std::fs::create_dir_all(&dir).map_err(|e| other(format!("cannot create folder: {e}")))?;
        let tmp = dir.join(format!(".{name}.tmp"));
        let written = std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, dir.join(format!("{name}.md"))));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(other(format!("cannot write: {e}")));
        }
        if kind == FileKind::Hires {
            *lock(&self.hires) = load_all_hires(&self.config(), &self.paths);
        }
        Ok(Response::ok(json!({ "name": name })))
    }

    fn delete_file(&self, kind: FileKind, name: &str) -> Result<Response, StudioError> {
        valid_name(name)?;
        let path = self
            .paths
            .config_dir
            .join(kind.dir())
            .join(format!("{name}.md"));
        std::fs::remove_file(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => other(format!("no {} file named {name}", kind.dir())),
            _ => other(format!("cannot delete: {e}")),
        })?;
        if kind == FileKind::Hires {
            *lock(&self.hires) = load_all_hires(&self.config(), &self.paths);
        }
        Ok(Response::ok(json!({ "name": name })))
    }

    fn create_ticket(&self, new: NewTicket) -> Result<Ticket, StudioError> {
        let entry = self.project(&ProjectId::new(&new.project))?;
        self.hire(&HireId::new(&new.hire))?;
        let base = new
            .base
            .or_else(|| entry.integration_branch.clone())
            .unwrap_or_else(|| "HEAD".to_string());
        let id = self.next_ticket_id(&new.project)?;
        let ticket = self.append(
            &id,
            Event::Created {
                id: id.clone(),
                project: ProjectId::new(&new.project),
                title: new.title,
                goal: new.goal,
                hire: HireId::new(&new.hire),
                base_branch: base,
                created_at: now_ms(),
                kind: new.kind,
                parent: new.parent,
                detail: new.detail,
                source: new.source,
            },
        )?;
        self.wake.notify_one();
        Ok(ticket)
    }

    /// The hire with this role whose id sorts first, so the choice is stable.
    pub(crate) fn first_with_role(&self, role: &str) -> Option<Hire> {
        lock(&self.hires)
            .values()
            .filter(|h| h.role == role)
            .min_by(|a, b| a.id.0.cmp(&b.id.0))
            .cloned()
    }

    /// A ticket filed by another ticket's agent. It lands on the parent's goal
    /// branch, and each parent may file only so many.
    fn file_ticket(
        &self,
        parent: &TicketId,
        title: String,
        detail: String,
        hire: Option<String>,
    ) -> Result<Ticket, StudioError> {
        let parent = self.ticket(parent)?;
        if !self
            .hire(&parent.hire)?
            .tools
            .iter()
            .any(|t| t == "file_ticket")
        {
            return Err(other(format!(
                "{} may not file tickets; its hire doesn't list file_ticket in tools",
                parent.hire
            )));
        }
        if parent.state != TicketState::Working {
            return Err(other(format!(
                "{} is {}; only a working ticket may file tickets",
                parent.id,
                parent.state.kind()
            )));
        }
        let max = self.config().goals.max_filed as usize;
        let filed = lock(&self.store)
            .tickets()?
            .iter()
            .filter(|t| t.parent.as_ref() == Some(&parent.id))
            .count();
        if filed >= max {
            return Err(other(format!(
                "{} has already filed {filed} tickets, the most it may file ({max}); \
                 finish with a summary instead",
                parent.id
            )));
        }
        let hire = match hire {
            Some(hire) => hire,
            None => {
                self.first_with_role("engineer")
                    .ok_or_else(|| other("no hire has the engineer role; pass hire"))?
                    .id
                    .0
            }
        };
        self.create_ticket(NewTicket {
            project: parent.project.0.clone(),
            title,
            goal: parent.goal.clone(),
            hire,
            base: Some(parent.base_branch.clone()),
            kind: TicketKind::Work,
            parent: Some(parent.id.clone()),
            detail,
            source: None,
        })
    }

    async fn ask_owner(
        &self,
        ticket: &str,
        question: &str,
        options: Vec<String>,
    ) -> Result<String, StudioError> {
        let ticket = self.ticket(&TicketId::new(ticket))?;
        let input = json!([{
            "question": question,
            "header": "Question",
            "options": options,
            "multi_select": false,
        }]);
        let decision = self
            .ask(
                &ticket.id,
                &ticket.hire.0,
                "question",
                "ask_owner",
                input,
                Vec::new(),
            )
            .await?;
        Ok(match decision {
            Decision::Allow => "The owner approved without an answer.".to_string(),
            Decision::Answer(answers) => answers.into_values().collect::<Vec<_>>().join("; "),
            Decision::Deny { message, .. } => format!("The owner declined to answer: {message}"),
        })
    }

    /// Skill folders: the studio's own, then each pack's. Earlier ones win.
    fn skill_dirs(&self) -> Vec<std::path::PathBuf> {
        let mut dirs = vec![self.paths.config_dir.join("skills")];
        dirs.extend(self.config().packs.iter().map(|p| p.path.join("skills")));
        dirs
    }

    fn load_skill(&self, name: Option<&str>) -> Result<String, StudioError> {
        if let Some(name) = name {
            valid_name(name)?;
            return self
                .skill_dirs()
                .iter()
                .find_map(|dir| std::fs::read_to_string(dir.join(name).join("SKILL.md")).ok())
                .ok_or_else(|| other(format!("no skill named {name}")));
        }
        let mut skills = BTreeMap::new();
        for dir in self.skill_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(|e| e.ok()) {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if let Ok(text) = std::fs::read_to_string(entry.path().join("SKILL.md")) {
                    skills.entry(name).or_insert_with(|| skill_summary(&text));
                }
            }
        }
        if skills.is_empty() {
            return Ok("No skills are installed.".to_string());
        }
        Ok(skills
            .iter()
            .map(|(name, summary)| format!("{name}: {summary}"))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Stops the agent of a running ticket; its worker then holds it.
    fn pause(&self, ticket: &str) -> Result<Response, StudioError> {
        let state = self.ticket(&TicketId::new(ticket))?.state;
        let running = matches!(
            state,
            TicketState::Claimed | TicketState::Working | TicketState::Reviewing
        );
        match lock(&self.active).get(ticket) {
            Some(stop) if running => {
                stop.request(StopReason::Pause);
                Ok(Response::ok(json!({ "ticket": ticket })))
            }
            _ => Err(other(format!("{ticket} has no agent running to pause"))),
        }
    }

    fn simple(&self, ticket: &str, event: Event) -> Result<Response, StudioError> {
        let updated = self.append(&TicketId::new(ticket), event)?;
        self.wake.notify_one();
        Ok(Response::ok(updated))
    }

    fn next_ticket_id(&self, project: &str) -> Result<TicketId, StudioError> {
        let prefix: String = project
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(4)
            .collect::<String>()
            .to_ascii_uppercase();
        let prefix = if prefix.is_empty() {
            "T".to_string()
        } else {
            prefix
        };
        let count = lock(&self.store)
            .tickets()?
            .iter()
            .filter(|t| t.project.0 == project)
            .count();
        Ok(TicketId::new(format!("{prefix}-{}", count + 1)))
    }

    /// Merges a ready ticket into its goal branch and removes its worktree.
    /// If the goal branch has moved on and conflicts with it, the branch is
    /// merged into the ticket's worktree instead, and the ticket goes back to
    /// its agent to resolve the conflicts.
    pub(crate) fn land(&self, id: &TicketId) -> Result<Landing, StudioError> {
        let ticket = self.ticket(id)?;
        if ticket.state != TicketState::Ready {
            return Err(other(format!("{id} is {}, not ready", ticket.state.kind())));
        }
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let worktree = worker::find_worktree(&repo, id)?
            .ok_or_else(|| other(format!("{id} has no worktree")))?;
        let branch = repo.ensure_goal_branch(&goal_of(&ticket), &ticket.base_branch)?;
        match repo.land(&worktree, &branch)? {
            LandOutcome::Merged { sha } => {
                let ticket = self.append(id, Event::Landed)?;
                // The worktree goes, so its evidence moves to the data folder;
                // if that fails, the worktree stays for `studio gc`. Anything
                // else left in it came from checks after the last commit.
                let log = crate::runlog::RunLog::new(&self.paths.data_dir, &id.to_string());
                match crate::evidence::keep(&worktree.path, &self.paths.data_dir, &id.to_string()) {
                    Ok(_) => {
                        let _ = repo.remove_worktree(&worktree, true);
                    }
                    Err(err) => log.note(&format!(
                        "kept the worktree: its evidence couldn't be copied: {err}"
                    )),
                }
                Ok(Landing::Landed {
                    ticket: Box::new(ticket),
                    branch,
                    sha,
                })
            }
            LandOutcome::Conflict { files } => {
                let mut worktree = worktree;
                worktree.merge_in(&branch)?;
                // Review then reads only what the ticket adds to the branch.
                worktree.set_base(&branch)?;
                self.append(
                    id,
                    Event::Conflicted {
                        branch: branch.clone(),
                        files: files.clone(),
                    },
                )?;
                Ok(Landing::SentBack { branch, files })
            }
        }
    }

    /// Landed work tickets whose goal branch is already in their base branch.
    /// Open pages ask every second, so git reads the tips once per project and
    /// is only asked about a pair of tips it hasn't answered for yet.
    fn merged_tickets(&self) -> Result<Vec<String>, StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let known = std::mem::take(&mut *lock(&self.merged_tips));
        let mut seen: HashMap<(String, String), bool> = HashMap::new();
        let mut projects = HashMap::new();
        let mut merged = Vec::new();
        for ticket in tickets {
            if ticket.state != TicketState::Landed || ticket.kind == TicketKind::Plan {
                continue;
            }
            let project = projects.entry(ticket.project.clone()).or_insert_with(|| {
                let repo = Repo::open(&self.project(&ticket.project).ok()?.path).ok()?;
                let tips = repo.branch_tips().ok()?;
                Some((repo, tips))
            });
            let Some((repo, tips)) = project else {
                continue;
            };
            let branch = Repo::goal_branch_name(&goal_of(&ticket));
            let Some(tip) = tips.get(&branch) else {
                continue;
            };
            let is_merged = match tips.get(&ticket.base_branch) {
                Some(base) => {
                    let pair = (tip.clone(), base.clone());
                    let held = known.get(&pair).copied();
                    *seen
                        .entry(pair)
                        .or_insert_with(|| held.unwrap_or_else(|| repo.is_merged(tip, base)))
                }
                // A tag or a sha has no tip to remember it by.
                None => repo.is_merged(tip, &ticket.base_branch),
            };
            if is_merged {
                merged.push(ticket.id.to_string());
            }
        }
        *lock(&self.merged_tips) = seen;
        Ok(merged)
    }

    /// A ticket on the goal of `landed` that merges `into` into the goal's
    /// branch and resolves the conflicts, so the goal then merges cleanly.
    /// One that is already open is reused.
    fn merge_ticket(
        &self,
        landed: &Ticket,
        into: &str,
        files: &[String],
    ) -> Result<Ticket, StudioError> {
        let goal = goal_of(landed);
        let source = format!("{MERGE_SOURCE}{into}");
        let open = lock(&self.store).tickets()?.into_iter().find(|t| {
            t.project == landed.project
                && goal_of(t) == goal
                && t.source.as_deref() == Some(source.as_str())
                && !matches!(
                    t.state,
                    TicketState::Landed | TicketState::Cancelled | TicketState::Failed { .. }
                )
        });
        if let Some(open) = open {
            return Ok(open);
        }
        // Whoever did the goal's work knows it best.
        let hire = match self.hire(&landed.hire) {
            Ok(hire) if landed.kind == TicketKind::Work => hire,
            _ => self
                .first_with_role("engineer")
                .ok_or_else(|| other("no hire has the engineer role"))?,
        };
        self.create_ticket(NewTicket {
            project: landed.project.0.clone(),
            title: format!("Merge {into} into the goal and resolve the conflicts"),
            goal,
            hire: hire.id.0,
            base: Some(landed.base_branch.clone()),
            kind: TicketKind::Work,
            parent: None,
            detail: format!(
                "Merging the goal into {into} conflicts in: {}. The studio merges {into} \
                 into this ticket's worktree; once this lands, the goal merges cleanly.",
                files.join(", ")
            ),
            source: Some(source),
        })
    }

    /// Merges the goal branch a landed ticket is on into the branch its work
    /// started from. Tickets of the goal still open come along later.
    fn merge(&self, id: &TicketId) -> Result<Response, StudioError> {
        let ticket = self.ticket(id)?;
        if ticket.state != TicketState::Landed {
            return Err(other(format!(
                "{id} is {}; land it first",
                ticket.state.kind()
            )));
        }
        let repo = Repo::open(&self.project(&ticket.project)?.path)?;
        let goal = goal_of(&ticket);
        let branch = Repo::goal_branch_name(&goal);
        let into = ticket.base_branch.clone();
        let open: Vec<String> = lock(&self.store)
            .tickets()?
            .into_iter()
            .filter(|t| t.project == ticket.project && goal_of(t) == goal)
            .filter(|t| !matches!(t.state, TicketState::Landed | TicketState::Cancelled))
            .filter(|t| t.kind == TicketKind::Work)
            .map(|t| t.id.to_string())
            .collect();
        // Only a studio with no merged goal yet can tell when its first was.
        let first = !self.first_merge_path().exists()
            && self.merged_tickets().is_ok_and(|merged| merged.is_empty());
        match repo.merge_goal(&branch, &into)? {
            MergeOutcome::Merged { sha } => {
                if first {
                    self.remember_first_merge(&goal, &into);
                }
                Ok(Response::ok(
                    json!({ "branch": branch, "into": into, "sha": sha, "open": open }),
                ))
            }
            MergeOutcome::Conflict { files } => {
                let fix = self.merge_ticket(&ticket, &into, &files)?;
                Err(other(format!(
                    "{branch} conflicts with {into} in: {}; {} merges {into} into the goal \
                     and resolves them, then merge again",
                    files.join(", "),
                    fix.id
                )))
            }
            MergeOutcome::Dirty { path } => Err(other(format!(
                "{into} is checked out at {} with uncommitted changes; commit or stash them first",
                path.display()
            ))),
        }
    }
}

/// What landing a ready ticket did.
pub(crate) enum Landing {
    Landed {
        ticket: Box<Ticket>,
        branch: String,
        sha: String,
    },
    /// The goal branch conflicts with the ticket in `files`; the ticket went
    /// back to its agent with the branch merged into its worktree.
    SentBack { branch: String, files: Vec<String> },
}

/// Where a ticket filed to merge a branch into its goal comes from, followed
/// by the branch: `merge:master`.
pub(crate) const MERGE_SOURCE: &str = "merge:";

/// The goal a ticket lands under: its goal, or its own id when it has none.
pub(crate) fn goal_of(ticket: &Ticket) -> String {
    if ticket.goal.trim().is_empty() {
        ticket.id.to_string()
    } else {
        ticket.goal.clone()
    }
}

/// Rules a project's `.studio.toml` implies: its `protected` paths ask, and
/// its `verify` commands (program and subcommand) run without asking.
fn project_rules(project: &ProjectEntry) -> Vec<studio_core::rule::Rule> {
    use studio_core::ids::RuleId;
    use studio_core::rule::{Level, Rule, RuleKind, Scope};
    let Ok(Some(config)) = studio_config::ProjectConfig::load(&project.path) else {
        return Vec::new();
    };
    let scope = |paths: Vec<String>| Scope {
        projects: vec![project.name.clone()],
        paths,
        ..Scope::default()
    };
    let mut rules = Vec::new();
    if !config.protected.is_empty() {
        rules.push(Rule {
            id: RuleId::new(format!("project/{}/protected", project.name)),
            kind: RuleKind::Protect,
            level: Level::Ask,
            when: scope(config.protected),
            pattern: None,
            command: None,
            action: None,
            why: "Listed under protected in .studio.toml.".to_string(),
        });
    }
    for command in &config.verify {
        let prefix: Vec<&str> = command
            .iter()
            .take(2)
            .take_while(|word| !word.starts_with('-'))
            .map(String::as_str)
            .collect();
        if prefix.is_empty() {
            continue;
        }
        let prefix = prefix.join(" ");
        rules.push(Rule {
            id: RuleId::new(format!("project/{}/verify/{prefix}", project.name)),
            kind: RuleKind::Gate,
            level: Level::Allow,
            when: scope(Vec::new()),
            pattern: None,
            command: None,
            action: Some(format!("bash:{prefix}")),
            why: "A verify command in .studio.toml.".to_string(),
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_waits_for_the_reset_or_a_while() {
        assert_eq!(quota_retry_at(1_000, Some(5_000)), 65_000);
        // No reset time, or one already past: a while after the hold.
        assert_eq!(quota_retry_at(1_000, None), 1_000 + QUOTA_RETRY_MS);
        assert_eq!(quota_retry_at(10_000, Some(5_000)), 10_000 + QUOTA_RETRY_MS);
    }
}
