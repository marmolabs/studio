use crate::api::{APPROVER_CALLS, AskView, JudgeView, RECENT_EVENTS, Request, Response};
use crate::budget::{self, DAY_MS, Reservation};
use crate::now_ms;
use crate::router::{self, Routed};
use crate::ruletest;
use crate::today;
use crate::worker;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use studio_config::config::{BudgetConfig, ConcurrencyConfig, ProjectEntry};
use studio_config::{StudioConfig, StudioPaths, load_hires, load_pack_hires};
use studio_core::event::Event;
use studio_core::ids::{HireId, ProjectId, TicketId};
use studio_core::policy;
use studio_core::rule::Hire;
use studio_core::ticket::{Ticket, TicketKind, TicketState};
use studio_runtime::Runtime;
use studio_store::Store;
use thiserror::Error;
use tokio::sync::Notify;

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
    /// Who is judging each ticket in review now, by ticket id: set when a
    /// reviewer or gate critic starts, cleared when it ends.
    judges: Mutex<HashMap<String, JudgeView>>,
    /// Whether a goal tip is in a base tip, as of the last `MergedTickets`.
    merged_tips: Mutex<HashMap<(String, String), bool>>,
    /// Held while `studio.toml` is rewritten, so two changes never mix.
    settings: Mutex<()>,
    wake: Notify,
    notifier: crate::notify::Notifier,
}

mod asks;
mod files;
mod history;
mod merging;
mod runs;
mod settings;
mod snapshot;
mod tickets;

pub(crate) use asks::Decision;
use asks::PendingAsk;
pub(crate) use merging::{Landing, MERGE_SOURCE, goal_of};
pub(crate) use runs::{Stop, StopReason, quota_retry_at};
use tickets::{NewTicket, shorten};

/// `studio.toml` as last read, and when to look at the file again.
struct LiveConfig {
    config: Arc<StudioConfig>,
    modified: Option<std::time::SystemTime>,
    checked: std::time::Instant,
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
            judges: Mutex::new(HashMap::new()),
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

    /// Every loaded hire.
    pub(crate) fn all_hires(&self) -> Vec<Hire> {
        lock(&self.hires).values().cloned().collect()
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
                let (tickets, spent) = {
                    let store = lock(&self.store);
                    let since = now_ms().saturating_sub(DAY_MS);
                    (store.tickets()?, store.spent_since(since)?.total)
                };
                Ok(Response::ok(self.status(&tickets, spent)))
            }
            Request::Office => self.office(),
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
            Request::AddProject {
                path,
                name,
                branch,
                daily_usd,
                pack,
                check,
            } => self.add_project(&path, name, branch, daily_usd, pack, check),
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

    /// The hire with this role whose id sorts first, so the choice is stable.
    pub(crate) fn first_with_role(&self, role: &str) -> Option<Hire> {
        lock(&self.hires)
            .values()
            .filter(|h| h.role == role)
            .min_by(|a, b| a.id.0.cmp(&b.id.0))
            .cloned()
    }
}
