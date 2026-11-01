//! One ticket's life while it has a worker: agent run, commit, checks, review.
//! The worker picks up from whatever state the ticket is in, so held or
//! restarted tickets continue where they stopped.

use crate::approver;
use crate::budget::Meter;
use crate::checks;
use crate::evidence;
use crate::runlog::RunLog;
use crate::studio::{
    Decision, Landing, MERGE_SOURCE, Stop, StopReason, Studio, StudioError, goal_of, other,
    quota_retry_at,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use studio_config::{Gate, ProjectConfig, StudioPaths, matches_any};
use studio_core::budget::{Allowance, run_spend};
use studio_core::event::Event;
use studio_core::ids::{RunId, TicketId};
use studio_core::policy::{self, Action, Context, Verdict};
use studio_core::rule::Hire;
use studio_core::ticket::{FailureClass, HeldReason, Ticket, TicketKind, TicketState};
use studio_git::{Repo, Worktree};
use studio_runtime::{Control, Cost, FailureKind, Outcome, Run, RunEvent, RunSpec, Runtime};
use studio_store::Spend;

/// How long a stopped run gets to report its final cost.
const DRAIN: Duration = Duration::from_secs(15);

pub(crate) async fn work(studio: Arc<Studio>, id: TicketId, stop: Arc<Stop>) {
    let log = RunLog::new(&studio.paths.data_dir, &id.to_string());
    if let Err(err) = work_on(&studio, &id, &stop, &log).await {
        log.note(&format!("worker stopped: {err}"));
        // Ignored when the ticket already moved on (e.g. it was cancelled).
        let _ = studio.append(
            &id,
            Event::Failed {
                class: FailureClass::Tooling,
                detail: err.to_string(),
            },
        );
    }
    studio.release(&id);
    studio.worker_done(&id);
}

/// How an agent run ended, from the worker's point of view.
enum AgentEnd {
    /// The agent finished its turn; its summary goes into the commit message.
    Done(Option<String>),
    /// The ticket was held, failed or cancelled; the worker stops.
    Stopped,
}

async fn work_on(
    studio: &Arc<Studio>,
    id: &TicketId,
    stop: &Stop,
    log: &RunLog,
) -> Result<(), StudioError> {
    let ticket = studio.ticket(id)?;
    let project = studio.project(&ticket.project)?;
    let repo = Repo::open(&project.path)?;
    let worktree = match find_worktree(&repo, id)? {
        Some(worktree) => worktree,
        None => {
            // A goal's tickets build on each other: start from what has landed.
            let goal_branch = Repo::goal_branch_name(&goal_of(&ticket));
            let worktree = if repo.branch_exists(&goal_branch)? {
                let worktree = repo.create_worktree(id, &goal_branch)?;
                log.note(&format!("started from {goal_branch}"));
                worktree
            } else {
                repo.create_worktree(id, &ticket.base_branch)?
            };
            // A ticket filed to merge a branch into its goal starts with that merge.
            if let Some(rev) = ticket
                .source
                .as_deref()
                .and_then(|s| s.strip_prefix(MERGE_SOURCE))
            {
                let files = worktree.merge_in(rev)?;
                log.note(&if files.is_empty() {
                    format!("merged {rev} cleanly")
                } else {
                    format!("merged {rev}; conflicts in: {}", files.join(", "))
                });
            }
            worktree
        }
    };
    let hire = studio.hire(&ticket.hire)?.clone();
    let runtime = studio.runtime(&hire.runtime)?;
    let mut session = log.last_session();
    let mut prompt = first_prompt(&ticket);

    loop {
        let ticket = studio.ticket(id)?;
        match ticket.state {
            TicketState::Claimed | TicketState::Working => {
                let run = AgentRun {
                    studio,
                    ticket: &ticket,
                    hire: &hire,
                    runtime: runtime.as_ref(),
                    log,
                    stop,
                    failed: ticket.attempts,
                    read_only: ticket.kind == TicketKind::Plan,
                };
                // A merge left conflicts: the agent resolves them first.
                let conflicted = worktree.conflicted_files()?;
                let asked = if conflicted.is_empty() {
                    prompt.clone()
                } else {
                    format!("{}\n\n{prompt}", conflict_note(&conflicted))
                };
                match run.go(&worktree, &asked, &mut session).await? {
                    AgentEnd::Stopped => return Ok(()),
                    AgentEnd::Done(summary) => {
                        // A run that never announced its session still counts as started.
                        if studio.ticket(id)?.state == TicketState::Claimed {
                            let run = session.clone().unwrap_or_else(|| "unknown".to_string());
                            studio.append(
                                id,
                                Event::RunStarted {
                                    run: RunId::new(run),
                                },
                            )?;
                        }
                        // A plan changes nothing: it lands once the producer is done.
                        if ticket.kind == TicketKind::Plan {
                            if let Some(summary) = summary.as_deref() {
                                log.note(&format!("plan: {summary}"));
                            }
                            studio.append(id, Event::Landed)?;
                            let _ = repo.remove_worktree(&worktree, true);
                            return Ok(());
                        }
                        // Nothing is committed while conflict markers remain.
                        let unresolved = unresolved_conflicts(&worktree)?;
                        if !unresolved.is_empty() {
                            let detail =
                                format!("conflict markers are still in: {}", unresolved.join(", "));
                            log.note(&detail);
                            prompt =
                                format!("Resolve every conflict and remove its markers: {detail}.");
                            studio.append(id, Event::ChecksStarted)?;
                            studio.append(id, Event::ChecksFailed { detail })?;
                            continue;
                        }
                        let message = match worktree.merge_subject()? {
                            Some(subject) => format!("{subject}\n\nStudio-Ticket: {}", ticket.id),
                            None => commit_message(&ticket),
                        };
                        match review_change(studio, &ticket, &hire, &worktree, &message, log)
                            .await?
                        {
                            ChangeVerdict::Commit => {
                                if let Some(sha) = worktree.commit(&message)? {
                                    log.note(&format!("committed {sha}"));
                                }
                                studio.append(id, Event::ChecksStarted)?;
                            }
                            ChangeVerdict::SendBack(detail) => {
                                prompt = format!(
                                    "Your changes break the owner's rules. Undo or fix them.\n\n{detail}"
                                );
                                studio.append(id, Event::ChecksStarted)?;
                                studio.append(id, Event::ChecksFailed { detail })?;
                            }
                            ChangeVerdict::Stop => return Ok(()),
                        }
                    }
                }
            }
            TicketState::Checking => {
                let config = ProjectConfig::load(&project.path)
                    .map_err(|e| other(e.to_string()))?
                    .unwrap_or_default();
                let mut commands = config.verify.clone();
                commands.extend(policy::checks(&studio.rules(), &context(&ticket, &hire)));
                let env = config.env_for(&project.path);
                // One note per command, so the web can say which check runs now.
                let mut result = Ok(());
                for command in &commands {
                    log.note(&format!("check: {}", command.join(" ")));
                    result = checks::run(&worktree.path, command, &env).await;
                    if result.is_err() {
                        break;
                    }
                }
                if result.is_ok() {
                    result = capture(&repo, &worktree, &config, ticket.attempts, log).await;
                }
                match result {
                    Ok(()) => {
                        log.note("checks passed");
                        studio.append(id, Event::ChecksPassed)?;
                    }
                    Err(detail) => {
                        log.note(&detail);
                        prompt = format!(
                            "The project's checks failed on your work. Fix the cause, \
                             don't weaken the checks.\n\n{detail}"
                        );
                        studio.append(id, Event::ChecksFailed { detail })?;
                    }
                }
            }
            TicketState::Reviewing => {
                let reviewer = studio.reviewer_for(&hire, ticket.attempts);
                let reviewed = reviewer.is_some();
                if let Some(reviewer) = reviewer {
                    let reviewer_runtime = studio.runtime(&reviewer.runtime)?;
                    let run = AgentRun {
                        studio,
                        ticket: &ticket,
                        hire: &reviewer,
                        runtime: reviewer_runtime.as_ref(),
                        log,
                        stop,
                        failed: 0,
                        read_only: true,
                    };
                    let images = evidence_images(&worktree, ticket.attempts);
                    // Reviewers run read-only, so they hear what already passed.
                    let checked: Vec<Vec<String>> = ProjectConfig::load(&project.path)
                        .ok()
                        .flatten()
                        .map(|config| config.verify)
                        .unwrap_or_default()
                        .into_iter()
                        .chain(policy::checks(&studio.rules(), &context(&ticket, &hire)))
                        .collect();
                    let request = review_prompt(&ticket, &worktree.diff()?, &images, &checked);
                    log.note(&format!("review by {} ({})", reviewer.id, reviewer.model));
                    studio.set_judge(id, &reviewer.id.0, &reviewer.model, None);
                    let ended = run.go(&worktree, &request, &mut None).await;
                    studio.clear_judge(id);
                    match ended? {
                        AgentEnd::Stopped => return Ok(()),
                        AgentEnd::Done(reply) => {
                            match verdict(reply.as_deref().unwrap_or_default()) {
                                Review::Pass => log.note("review passed"),
                                Review::Fail(notes) => {
                                    log.note(&format!("review failed: {notes}"));
                                    prompt = format!(
                                        "A reviewer sent your work back. Address every point.\n\n{notes}"
                                    );
                                    studio.append(id, Event::ReviewFailed { detail: notes })?;
                                    continue;
                                }
                            }
                        }
                    }
                } else {
                    log.note("no reviewer on a different model; the owner reviews");
                }
                let config = ProjectConfig::load(&project.path)
                    .map_err(|e| other(e.to_string()))?
                    .unwrap_or_default();
                match run_gates(studio, &ticket, &hire, &worktree, &config.gates, stop, log).await?
                {
                    GateEnd::Pass => {
                        studio.append(id, Event::ReviewPassed)?;
                        // Unreviewed work waits for the owner even with auto_land.
                        if config.auto_land && reviewed {
                            auto_land(studio, &ticket, log);
                        }
                    }
                    GateEnd::SendBack(notes) => {
                        prompt = format!(
                            "A reviewer sent your work back. Address every point.\n\n{notes}"
                        );
                        studio.append(id, Event::ReviewFailed { detail: notes })?;
                    }
                    GateEnd::Stop => return Ok(()),
                }
            }
            _ => return Ok(()),
        }
    }
}

/// Lands a ready ticket on its goal branch. A conflict sends it back to its
/// agent; an error leaves it ready for the owner, who is told as for any
/// ready ticket.
fn auto_land(studio: &Studio, ticket: &Ticket, log: &RunLog) {
    match studio.land(&ticket.id) {
        Ok(Landing::Landed { branch, .. }) => log.note(&format!("auto-landed on {branch}")),
        Ok(Landing::SentBack { branch, files }) => log.note(&format!(
            "auto-land: {branch} has moved on and conflicts in {}; back to resolve them",
            files.join(", ")
        )),
        Err(err) => log.note(&format!("auto-land failed, left ready: {err}")),
    }
}

/// The environment the project's `.studio.toml` gives its agents; none when
/// the project or that file can't be read.
fn project_env(studio: &Studio, ticket: &Ticket) -> Vec<(String, String)> {
    let Ok(project) = studio.project(&ticket.project) else {
        return Vec::new();
    };
    ProjectConfig::load(&project.path)
        .ok()
        .flatten()
        .map(|config| config.env_for(&project.path))
        .unwrap_or_default()
}

/// What the agent hears when a merge left conflicts in its worktree.
fn conflict_note(files: &[String]) -> String {
    format!(
        "Newer work was merged into this worktree and conflicts with it in: {}. \
         Resolve each conflict with the Edit tool so both sides' changes still work, \
         and remove the conflict markers. Don't commit, abort or redo the merge: \
         the studio finishes it when it commits your changes.",
        files.join(", ")
    )
}

/// Conflicted files that still hold a conflict marker at the start of a line.
fn unresolved_conflicts(worktree: &Worktree) -> Result<Vec<String>, StudioError> {
    let marker = |line: &str| {
        ["<<<<<<<", ">>>>>>>"].iter().any(|m| {
            line.strip_prefix(m)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        })
    };
    Ok(worktree
        .conflicted_files()?
        .into_iter()
        .filter(|file| {
            std::fs::read_to_string(worktree.path.join(file))
                .is_ok_and(|text| text.lines().any(marker))
        })
        .collect())
}

/// A wait in words: "15 min", "2 h 5 min", "3 d 4 h".
fn wait_words(ms: u64) -> String {
    let minutes = ms.div_ceil(60_000);
    match minutes {
        0..60 => format!("{minutes} min"),
        60..1440 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => format!("{} d {} h", minutes / 1440, minutes % 1440 / 60),
    }
}

struct AgentRun<'a> {
    studio: &'a Arc<Studio>,
    ticket: &'a Ticket,
    hire: &'a Hire,
    runtime: &'a dyn Runtime,
    log: &'a RunLog,
    stop: &'a Stop,
    /// Attempts at this ticket that didn't pass; picks the hire's model.
    failed: u32,
    /// Reviewers and producers read; every write or question they raise is
    /// refused. Producers may still use the studio's own tools.
    read_only: bool,
}

/// What a run has done so far, for the ledger.
struct Tally {
    model: String,
    meter: Meter,
    session: Option<String>,
    reported: Option<Cost>,
}

impl AgentRun<'_> {
    fn id(&self) -> &TicketId {
        &self.ticket.id
    }

    /// Reserves money, runs the agent within it, and records what it cost.
    async fn go(
        &self,
        worktree: &Worktree,
        prompt: &str,
        session: &mut Option<String>,
    ) -> Result<AgentEnd, StudioError> {
        let model = self.hire.model_for(self.failed).to_string();
        let price = self.studio.price(&model);
        if price.is_none() && !self.runtime.capabilities().reports_cost {
            self.log.note(&format!(
                "budget: no price for {model} and {} doesn't report cost; refusing to run",
                self.runtime.id()
            ));
            return self.hold(HeldReason::Budget);
        }
        if model != self.hire.model {
            self.log.note(&format!(
                "escalated to {model} after {} failed attempts",
                self.failed
            ));
        }
        // A session can't carry over to a different model.
        if let Some(old) = session.as_deref()
            && self.studio.session_model(old)?.is_some_and(|m| m != model)
        {
            *session = None;
        }
        self.quote();
        let allowance = match self.studio.reserve(self.ticket, self.hire)? {
            Allowance::Run { usd, bound_by } => {
                self.log.note(&format!(
                    "budget: this run may spend ${usd:.4} (bound by {bound_by:?})"
                ));
                usd
            }
            Allowance::Refused { scope, limit, used } => {
                self.log.note(&format!(
                    "budget: {scope:?} limit ${limit:.2} has ${used:.4} used or reserved; holding"
                ));
                return self.hold(HeldReason::Budget);
            }
        };
        let mut tally = Tally {
            model,
            meter: Meter::new(price),
            session: session.clone(),
            reported: None,
        };
        let result = self
            .run(worktree, prompt, allowance, session, &mut tally)
            .await;
        self.record(&tally)?;
        self.studio.release(self.id());
        result
    }

    async fn run(
        &self,
        worktree: &Worktree,
        prompt: &str,
        allowance: f64,
        session: &mut Option<String>,
        tally: &mut Tally,
    ) -> Result<AgentEnd, StudioError> {
        let rules = self.studio.rules();
        let ctx = context(self.ticket, self.hire);
        let system = [
            self.hire.prompt.trim(),
            policy::guidance(&rules, &ctx).trim(),
        ]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n\n");
        let mcp_config = self.mcp_config();
        let spec = RunSpec {
            cwd: worktree.path.clone(),
            prompt: prompt.to_string(),
            model: Some(tally.model.clone()).filter(|m| !m.is_empty()),
            effort: self.hire.effort.clone(),
            resume: session.clone(),
            append_system_prompt: Some(system).filter(|p| !p.is_empty()),
            max_budget_usd: Some(allowance),
            mcp_config,
            strict_mcp: !self.hire.user_mcp,
            env: project_env(self.studio, self.ticket),
            ..RunSpec::default()
        };
        let mut run = self.runtime.start(spec)?;
        let mut stopping: Option<StopReason> = None;
        // Calls the approver allowed in this run, by tool and input.
        let mut approved: HashMap<String, String> = HashMap::new();
        // What the agent said, for a reviewer whose run ends without a summary.
        let mut said = String::new();
        loop {
            let event = if stopping.is_some() {
                // Draining a stopped run: wait only so long for its final cost.
                tokio::time::timeout(DRAIN, run.next()).await.ok().flatten()
            } else {
                tokio::select! {
                    event = run.next() => event,
                    _ = self.stop.notify.notified() => {
                        let reason = self.stop.reason().unwrap_or(StopReason::Cancel);
                        self.log.note(match reason {
                            StopReason::Cancel => "cancelled by the owner",
                            StopReason::Budget => "budget: a 24-hour limit was reached; stopping",
                            StopReason::Pause => "paused by the owner",
                        });
                        stopping = Some(reason);
                        let _ = run.interrupt().await;
                        continue;
                    }
                }
            };
            let Some(event) = event else {
                return match stopping {
                    Some(reason) => self.stopped(reason),
                    None => self.fail(FailureClass::Crash, "the run ended without a result"),
                };
            };
            self.log.event(&event);
            if let RunEvent::Started { session: s, .. } = &event {
                tally.session = Some(s.clone());
                *session = Some(s.clone());
            }
            // Any sign of life means the run has started, even if the runtime
            // never announced a session.
            self.mark_started(session.as_deref())?;
            match event {
                RunEvent::Started { .. } => {}
                RunEvent::Usage {
                    message_id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    ..
                } => {
                    tally.meter.update(
                        &message_id,
                        (
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_write_tokens,
                        ),
                    );
                    self.studio.meter(self.id(), tally.meter.usd());
                    if stopping.is_none()
                        && tally.meter.has_price()
                        && tally.meter.usd() >= allowance
                    {
                        self.log.note(&format!(
                            "budget: estimated ${:.4} reached this run's ${allowance:.4}; stopping",
                            tally.meter.usd()
                        ));
                        stopping = Some(StopReason::Budget);
                        let _ = run.interrupt().await;
                    }
                }
                // The studio's own tools do their checks in studiod.
                RunEvent::Permission {
                    request_id, tool, ..
                } if stopping.is_none() && tool.starts_with("mcp__studio__") => {
                    self.log.note(&format!("allowed {tool}: studio tool"));
                    run.allow(&request_id).await?;
                }
                RunEvent::Permission { request_id, .. } | RunEvent::Question { request_id, .. }
                    if self.read_only =>
                {
                    let _ = run
                        .deny(
                            &request_id,
                            "This run is read-only: it can't change files or ask questions \
                             except through the studio tools.",
                        )
                        .await;
                }
                RunEvent::Permission { request_id, .. } | RunEvent::Question { request_id, .. }
                    if stopping.is_some() =>
                {
                    let _ = run
                        .deny(&request_id, "The studio is stopping this run.")
                        .await;
                }
                RunEvent::Permission {
                    request_id,
                    tool,
                    input,
                } => {
                    let verdict = tool_verdict(&rules, &ctx, &worktree.path, &tool, &input);
                    match verdict.decision {
                        policy::Decision::Allow => {
                            let why = reasons(&verdict, "inside the worktree").join("; ");
                            self.log.note(&format!("allowed {tool}: {why}"));
                            run.allow(&request_id).await?;
                        }
                        policy::Decision::Block => {
                            let why = reasons(&verdict, "").join("; ");
                            self.log.note(&format!("blocked {tool}: {why}"));
                            let message = format!("Blocked by the owner's rules: {why}");
                            run.deny(&request_id, &message).await?;
                        }
                        policy::Decision::Ask => {
                            let mut why = reasons(&verdict, "");
                            // Owner rules always reach the owner; the rest go to the
                            // approver, when the studio has one.
                            let owner_rule = verdict
                                .findings
                                .iter()
                                .any(|f| f.level == studio_core::rule::Level::Ask);
                            let approver = self.studio.first_with_role("approver");
                            if let (false, Some(approver)) = (owner_rule, approver) {
                                let key = format!("{tool}{input}");
                                let call = match approved.get(&key) {
                                    Some(reason) => approver::Call::Allow(reason.clone()),
                                    None => {
                                        approver::judge(
                                            self.studio,
                                            &approver,
                                            self.ticket,
                                            &worktree.path,
                                            &tool,
                                            &input,
                                            &why,
                                        )
                                        .await
                                    }
                                };
                                match call {
                                    approver::Call::Allow(reason) => {
                                        self.log
                                            .note(&format!("approver allowed {tool}: {reason}"));
                                        approved.insert(key, reason);
                                        run.allow(&request_id).await?;
                                        continue;
                                    }
                                    approver::Call::Deny(reason) => {
                                        self.log.note(&format!("approver denied {tool}: {reason}"));
                                        let message = format!(
                                            "The studio's approver declined this: {reason} \
                                             Find another way, or explain in your summary \
                                             why it's needed."
                                        );
                                        run.deny(&request_id, &message).await?;
                                        continue;
                                    }
                                    approver::Call::Owner(reason) => {
                                        self.log.note(&format!(
                                            "approver left {tool} to the owner: {reason}"
                                        ));
                                        why.push(format!("Approver: {reason}"));
                                    }
                                }
                            }
                            let decision = self
                                .studio
                                .ask(self.id(), &self.hire.id.0, "permission", &tool, input, why)
                                .await?;
                            if !self.decide(&run, request_id, decision).await? {
                                stopping = Some(StopReason::Cancel);
                            }
                        }
                    }
                }
                RunEvent::Question {
                    request_id,
                    questions,
                } => {
                    let input = serde_json::to_value(&questions).unwrap_or_default();
                    let decision = self
                        .studio
                        .ask(
                            self.id(),
                            &self.hire.id.0,
                            "question",
                            "AskUserQuestion",
                            input,
                            Vec::new(),
                        )
                        .await?;
                    if !self.decide(&run, request_id, decision).await? {
                        stopping = Some(StopReason::Cancel);
                    }
                }
                RunEvent::Finished {
                    outcome,
                    session: s,
                    summary,
                    cost,
                } => {
                    if let Some(s) = s {
                        tally.session = Some(s.clone());
                        *session = Some(s);
                    }
                    tally.reported = Some(cost);
                    // A run cut short by its cap reports no summary, but a
                    // reviewer may already have said its verdict.
                    let summary = match summary {
                        None if self.read_only && !said.is_empty() => Some(said),
                        summary => summary,
                    };
                    return match stopping {
                        Some(reason) => self.stopped(reason),
                        None => self.finished(outcome, summary),
                    };
                }
                RunEvent::Text { text } => {
                    said.push_str(&text);
                    said.push('\n');
                }
                _ => {}
            }
        }
    }

    /// Logs what this hire's past tickets suggest the work will cost and take,
    /// and warns when that is more than the ticket may still spend. An estimate
    /// only: the hard caps are what stop a run.
    fn quote(&self) {
        if self.hire.id != self.ticket.hire {
            return;
        }
        let Ok(Some(quote)) = self.studio.quote(&self.hire.id.0) else {
            return;
        };
        self.log.note(&format!(
            "quote: ~${:.2}, ~{:.0} min from {} tickets",
            quote.usd, quote.minutes, quote.samples
        ));
        if let Ok(left) = self.studio.ticket_left(self.ticket, self.hire)
            && quote.usd > left
        {
            self.log.note(&format!(
                "warning: the quote is above the ${left:.2} this ticket may still spend"
            ));
        }
    }

    /// Writes the config that gives a run the studio-tools MCP server. Reviewers
    /// get none; producers do.
    fn mcp_config(&self) -> Option<PathBuf> {
        let wanted = self.runtime.capabilities().mcp
            && (!self.read_only || self.ticket.kind == TicketKind::Plan);
        if !wanted {
            return None;
        }
        let path = self.log.mcp_config_path();
        let config = mcp_config(&self.studio.paths, self.id());
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&path, config.to_string()) {
            Ok(()) => Some(path),
            Err(err) => {
                self.log.note(&format!("can't write the MCP config: {err}"));
                None
            }
        }
    }

    fn mark_started(&self, session: Option<&str>) -> Result<(), StudioError> {
        if self.studio.ticket(self.id())?.state == TicketState::Claimed {
            self.studio.append(
                self.id(),
                Event::RunStarted {
                    run: RunId::new(session.unwrap_or("unknown")),
                },
            )?;
        }
        Ok(())
    }

    /// Writes the run's cost to the ledger: the runtime's own figure when it
    /// reported one, otherwise the live estimate.
    fn record(&self, tally: &Tally) -> Result<(), StudioError> {
        let (input_tokens, output_tokens) = tally.meter.tokens();
        let (usd, reported) = match &tally.reported {
            Some(cost) if cost.reported => {
                let before = match &tally.session {
                    Some(s) if cost.cumulative => self.studio.session_spent(s)?,
                    _ => 0.0,
                };
                (run_spend(cost.usd, before, cost.cumulative), true)
            }
            _ => (tally.meter.usd(), false),
        };
        self.log.note(&format!(
            "spent ${usd:.4} ({})",
            if reported { "reported" } else { "estimated" }
        ));
        self.studio.record_spend(&Spend {
            at_ms: crate::now_ms(),
            ticket_id: self.id().clone(),
            project: self.ticket.project.0.clone(),
            hire: self.hire.id.0.clone(),
            runtime: self.runtime.id().to_string(),
            model: tally.model.clone(),
            session: tally.session.clone(),
            usd,
            reported,
            input_tokens,
            output_tokens,
        })
    }

    /// Passes the owner's decision to the run. Returns false on a denial that
    /// stops the ticket. A denied permission with a note from the owner only
    /// refuses that call; the run carries on (the ask settles that).
    async fn decide(
        &self,
        run: &Run,
        request_id: String,
        decision: Decision,
    ) -> Result<bool, StudioError> {
        match decision {
            Decision::Allow => run.send(Control::Allow { request_id }).await?,
            Decision::Answer(answers) => {
                run.send(Control::Answer {
                    request_id,
                    answers,
                })
                .await?
            }
            Decision::Deny {
                message,
                stop: false,
            } => {
                let reply = format!("The owner declined this: {message}");
                run.deny(&request_id, &reply).await?;
                self.log.note(&format!("denied, run continues: {message}"));
            }
            Decision::Deny { message, .. } => {
                let _ = run.deny(&request_id, &message).await;
                let _ = run.interrupt().await;
                self.log.note(&format!("denied: {message}"));
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The run was stopped on purpose. A cancel or a denial already moved the
    /// ticket; a budget stop or a pause holds it.
    fn stopped(&self, reason: StopReason) -> Result<AgentEnd, StudioError> {
        match reason {
            StopReason::Cancel => Ok(AgentEnd::Stopped),
            StopReason::Budget => self.hold(HeldReason::Budget),
            StopReason::Pause => self.hold(HeldReason::Paused),
        }
    }

    fn finished(&self, outcome: Outcome, summary: Option<String>) -> Result<AgentEnd, StudioError> {
        // A reviewer or gate that gave its verdict before the cap stopped it
        // has done its job: the money is spent, so the verdict counts.
        let judged = self.read_only
            && summary.as_deref().is_some_and(|s| {
                s.lines()
                    .any(|l| l.trim().trim_matches('`').starts_with("VERDICT:"))
            });
        if judged && matches!(outcome, Outcome::Budget | Outcome::MaxTurns) {
            self.log
                .note("the verdict came in before the run was stopped; it counts");
            return Ok(AgentEnd::Done(summary));
        }
        match outcome {
            Outcome::Success => Ok(AgentEnd::Done(summary)),
            Outcome::MaxTurns => self.fail(FailureClass::Timeout, "ran out of turns"),
            Outcome::Budget => self.hold(HeldReason::Budget),
            Outcome::Failed { kind, message } => match kind {
                FailureKind::Quota => {
                    let now = crate::now_ms();
                    let wait = quota_retry_at(now, self.log.quota_resets_at()).saturating_sub(now);
                    self.log
                        .note(&format!("{message}; trying again in {}", wait_words(wait)));
                    self.hold(HeldReason::Quota)
                }
                FailureKind::Auth => self.hold(HeldReason::Auth),
                FailureKind::Timeout => self.fail(FailureClass::Timeout, &message),
                FailureKind::Crash => self.fail(FailureClass::Crash, &message),
                FailureKind::Tooling => self.fail(FailureClass::Tooling, &message),
            },
        }
    }

    /// Holds the ticket unless it already left the running states.
    fn hold(&self, reason: HeldReason) -> Result<AgentEnd, StudioError> {
        let state = self.studio.ticket(self.id())?.state;
        if matches!(
            state,
            TicketState::Claimed
                | TicketState::Working
                | TicketState::Checking
                | TicketState::Reviewing
        ) {
            self.studio.append(self.id(), Event::Held { reason })?;
        }
        Ok(AgentEnd::Stopped)
    }

    fn fail(&self, class: FailureClass, detail: &str) -> Result<AgentEnd, StudioError> {
        self.studio.append(
            self.id(),
            Event::Failed {
                class,
                detail: detail.to_string(),
            },
        )?;
        Ok(AgentEnd::Stopped)
    }
}

/// The MCP config for a ticket's run. `studio` sits next to `studiod`; if it
/// doesn't, we hope it is on PATH.
fn mcp_config(paths: &StudioPaths, ticket: &TicketId) -> Value {
    let studio = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("studio")))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "studio".to_string());
    let socket = crate::api::socket_path(&paths.data_dir);
    let args = [
        "mcp",
        "--ticket",
        &ticket.to_string(),
        "--socket",
        &socket.to_string_lossy(),
    ];
    json!({ "mcpServers": { "studio": { "command": studio, "args": args } } })
}

pub(crate) fn context<'a>(ticket: &'a Ticket, hire: &'a Hire) -> Context<'a> {
    Context {
        project: &ticket.project.0,
        hire: &hire.id.0,
        role: &hire.role,
    }
}

fn reasons(verdict: &Verdict, fallback: &str) -> Vec<String> {
    let found: Vec<String> = verdict
        .findings
        .iter()
        .map(|f| format!("{} ({})", f.why, f.rule))
        .collect();
    if found.is_empty() && !fallback.is_empty() {
        vec![fallback.to_string()]
    } else {
        found
    }
}

/// Applies the rules to a tool call, with its file paths made relative to the
/// worktree.
pub(crate) fn tool_verdict(
    rules: &[studio_core::rule::Rule],
    ctx: &Context,
    root: &Path,
    tool: &str,
    input: &Value,
) -> Verdict {
    let (paths, outside) = tool_paths(input, root);
    let command = input["command"].as_str();
    let text = input.to_string();
    policy::evaluate(
        rules,
        ctx,
        &Action::Tool {
            tool,
            paths,
            outside,
            command,
            text: &text,
        },
    )
}

fn tool_paths(input: &Value, root: &Path) -> (Vec<String>, bool) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut paths = Vec::new();
    let mut outside = false;
    for key in ["file_path", "path", "notebook_path"] {
        let Some(raw) = input[key].as_str() else {
            continue;
        };
        let given = Path::new(raw);
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            root.join(given)
        };
        let path = real_path(&normalize(&joined));
        match path.strip_prefix(&root) {
            Ok(rel) => paths.push(rel.to_string_lossy().into_owned()),
            Err(_) => {
                outside = true;
                paths.push(path.to_string_lossy().into_owned());
            }
        }
    }
    (paths, outside)
}

/// Resolves `..` and `.` without touching the disk, so `a/../../x` can't hide
/// a path outside the worktree.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The canonical form of a path that may not exist yet: symlinks such as
/// `/tmp` → `/private/tmp` are resolved through its nearest existing ancestor.
fn real_path(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
    let mut real = existing.canonicalize().unwrap_or(existing);
    for name in rest.iter().rev() {
        real.push(name);
    }
    real
}

enum ChangeVerdict {
    Commit,
    /// Rules block the change; the agent gets this to fix.
    SendBack(String),
    /// The owner refused the change; the ticket has failed.
    Stop,
}

/// Checks the uncommitted work and its message against the rules before the
/// studio commits it.
async fn review_change(
    studio: &Arc<Studio>,
    ticket: &Ticket,
    hire: &Hire,
    worktree: &Worktree,
    message: &str,
    log: &RunLog,
) -> Result<ChangeVerdict, StudioError> {
    let files: Vec<String> = worktree
        .changed_files()?
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    if files.is_empty() {
        return Ok(ChangeVerdict::Commit);
    }
    let added = worktree
        .diff()?
        .lines()
        .filter_map(|l| l.strip_prefix('+').filter(|_| !l.starts_with("+++")))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!("{added}\n{message}");
    let rules = studio.rules();
    let verdict = policy::evaluate(
        &rules,
        &context(ticket, hire),
        &Action::Change {
            files: &files,
            text: &text,
        },
    );
    match verdict.decision {
        policy::Decision::Allow => Ok(ChangeVerdict::Commit),
        policy::Decision::Block => {
            let why = reasons(&verdict, "").join("\n");
            log.note(&format!("change blocked: {why}"));
            Ok(ChangeVerdict::SendBack(why))
        }
        policy::Decision::Ask => {
            let decision = studio
                .ask(
                    &ticket.id,
                    &hire.id.0,
                    "change",
                    "commit",
                    json!({ "files": files, "message": message }),
                    reasons(&verdict, ""),
                )
                .await?;
            Ok(match decision {
                Decision::Deny { message: why, .. } => {
                    log.note(&format!("change refused: {why}"));
                    ChangeVerdict::Stop
                }
                _ => ChangeVerdict::Commit,
            })
        }
    }
}

pub(crate) fn find_worktree(repo: &Repo, id: &TicketId) -> Result<Option<Worktree>, StudioError> {
    let probe = format!("studio/{}", branch_slug(&id.to_string()));
    Ok(repo
        .studio_worktrees()?
        .into_iter()
        .find(|w| w.branch == probe))
}

// Mirrors studio-git's branch naming for ticket worktrees.
pub(crate) fn branch_slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

const DIFF_LIMIT: usize = 60_000;

/// The diff cut to what a reviewer is shown, with a note if it was cut.
fn shown_diff(diff: &str) -> String {
    let cut = (0..=DIFF_LIMIT.min(diff.len()))
        .rev()
        .find(|i| diff.is_char_boundary(*i))
        .unwrap_or(0);
    let more = if cut < diff.len() {
        "\n… (diff cut short)"
    } else {
        ""
    };
    format!("{}{more}", &diff[..cut])
}

fn review_prompt(
    ticket: &Ticket,
    diff: &str,
    images: &[String],
    checked: &[Vec<String>],
) -> String {
    let checks = if checked.is_empty() {
        String::new()
    } else {
        format!(
            "The studio already ran these on the change and they passed:\n{}\n\n",
            checked
                .iter()
                .map(|c| format!("- `{}`", c.join(" ")))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "Review this change for {}\
         Check that it does what the ticket asks, follows the owner's rules, and \
         doesn't break anything. You may read files, but you can't change them or \
         run commands.\n\n{checks}{}\
         Reply with your findings, then end with exactly one line: \
         `VERDICT: PASS` or `VERDICT: FAIL`.\n\n```diff\n{}\n```",
        brief(ticket),
        screenshots(images),
        shown_diff(diff)
    )
}

/// A changed image beside its shipped version, as paths relative to the worktree.
#[derive(Debug, PartialEq, Eq)]
struct ImagePair {
    path: String,
    before: Option<String>,
    after: String,
}

/// Most changed images a gate gets to compare.
const MAX_PAIRS: usize = 12;

/// Writes each changed png, jpg or gif matching the gate's paths, and its base
/// version if it had one, into the attempt's evidence folder. A deleted image
/// has nothing to compare and doesn't count. A file that can't be written is
/// skipped with a note.
fn image_pairs(
    worktree: &Worktree,
    attempt: u32,
    files: &[String],
    gate: &Gate,
    log: &RunLog,
) -> Vec<ImagePair> {
    let mut pairs = Vec::new();
    let mut keys = HashSet::new();
    let images = files
        .iter()
        .filter(|f| matches_any(&gate.paths, std::slice::from_ref(f)))
        .filter(|f| {
            let ext = Path::new(f).extension().and_then(|e| e.to_str());
            ext.is_some_and(|e| ["png", "jpg", "gif"].contains(&e.to_ascii_lowercase().as_str()))
        })
        // `symlink_metadata` so a dangling link still gets its note.
        .filter(|f| std::fs::symlink_metadata(worktree.path.join(f)).is_ok())
        .take(MAX_PAIRS);
    for file in images {
        // Different paths can share a key; lowercase for case-blind disks.
        let stem = evidence::image_key(file);
        let mut key = stem.clone();
        let mut n = 1;
        while !keys.insert(key.to_ascii_lowercase()) {
            n += 1;
            key = format!("{stem}-{n}");
        }
        match write_pair(worktree, attempt, file, &key) {
            Ok(pair) => pairs.push(pair),
            Err(why) => log.note(&format!("no before/after for {file}: {why}")),
        }
    }
    pairs
}

/// Authors control the worktree, so a link anywhere on the way in or out is
/// refused: it could leak or overwrite a file outside it.
fn write_pair(
    worktree: &Worktree,
    attempt: u32,
    file: &str,
    key: &str,
) -> Result<ImagePair, String> {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let folder = evidence::folder(attempt);
    let after = format!("{folder}/after-{key}.{ext}");
    let before = format!("{folder}/before-{key}.{ext}");
    if let Some(link) = [file, &after, &before]
        .into_iter()
        .find(|p| evidence::through_link(&worktree.path, p))
    {
        return Err(format!("{link} goes through a link"));
    }
    let new = std::fs::read(worktree.path.join(file)).map_err(|e| e.to_string())?;
    let old = worktree.file_at_base(file).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(worktree.path.join(&folder)).map_err(|e| e.to_string())?;
    std::fs::write(worktree.path.join(&after), new).map_err(|e| e.to_string())?;
    let before = match old {
        Some(bytes) => {
            std::fs::write(worktree.path.join(&before), bytes).map_err(|e| e.to_string())?;
            Some(before)
        }
        None => None,
    };
    Ok(ImagePair {
        path: file.to_string(),
        before,
        after,
    })
}

/// Most changed files a gate's measure command runs on.
const MAX_MEASURED: usize = 12;

/// Runs the gate's measure command on each changed file matching its paths, or
/// once when no argument has `{file}`. A deleted file has nothing to measure and
/// doesn't count. A file reached through a link is skipped with a note: authors
/// control the worktree, and the output goes to the critic.
async fn measurements(
    worktree: &Worktree,
    files: &[String],
    gate: &Gate,
    log: &RunLog,
) -> Vec<String> {
    if gate.measure.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    if gate.measure.iter().any(|a| a.contains("{file}")) {
        let matching = files
            .iter()
            .filter(|f| matches_any(&gate.paths, std::slice::from_ref(f)))
            .filter(|f| {
                let link = evidence::through_link(&worktree.path, f);
                if link {
                    log.note(&format!("no measurement for {f}: it goes through a link"));
                }
                !link && worktree.path.join(f).is_file()
            })
            .take(MAX_MEASURED);
        for file in matching {
            results.push(checks::measure(&worktree.path, &gate.measure, Some(file)).await);
        }
    } else {
        results.push(checks::measure(&worktree.path, &gate.measure, None).await);
    }
    log.note(&format!(
        "gate {}: measured {} file(s)",
        gate.name,
        results.len()
    ));
    results
}

fn gate_prompt(
    ticket: &Ticket,
    gate: &Gate,
    diff: &str,
    images: &[String],
    pairs: &[ImagePair],
    measured: &[String],
) -> String {
    let mut prompt = format!("You are the {} gate for {}", gate.name, brief(ticket));
    if let Some(extra) = gate.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
        prompt.push_str(&format!("{}\n\n", extra.trim()));
    }
    prompt.push_str(
        "Judge whether this change is good enough to show the owner. You may read \
         files, but you can't change them.\n\n",
    );
    if !pairs.is_empty() {
        let lines: Vec<String> = pairs
            .iter()
            .map(|p| {
                let before = p.before.as_deref().unwrap_or("new file");
                format!("- {}: before {before}, after {}", p.path, p.after)
            })
            .collect();
        prompt.push_str(&format!(
            "Changed images, the shipped version beside the new one (read them to see them):\n{}\n\n",
            lines.join("\n")
        ));
    }
    if !measured.is_empty() {
        let blocks: Vec<String> = measured.iter().map(|m| format!("```\n{m}\n```")).collect();
        prompt.push_str(&format!("Measurements:\n{}\n\n", blocks.join("\n")));
    }
    prompt.push_str(&screenshots(images));
    prompt.push_str(&format!(
        "Reply with your findings, then end with exactly one line: \
         `VERDICT: PASS` or `VERDICT: FAIL`.\n\n```diff\n{}\n```",
        shown_diff(diff)
    ));
    prompt
}

/// "ticket X: title", then the goal and detail when there are any.
fn brief(ticket: &Ticket) -> String {
    let mut text = format!("ticket {}: {}\n\n", ticket.id, ticket.title.trim());
    if !ticket.goal.trim().is_empty() {
        text.push_str(&format!("Goal: {}\n\n", ticket.goal.trim()));
    }
    if !ticket.detail.trim().is_empty() {
        text.push_str(&format!("{}\n\n", ticket.detail.trim()));
    }
    text
}

fn screenshots(images: &[String]) -> String {
    if images.is_empty() {
        return "The capture produced no screenshots for this change.\n\n".to_string();
    }
    format!(
        "Screenshots from running the project with this change (read them to see them):\n{}\n\n",
        images
            .iter()
            .map(|i| format!("- {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// This attempt's evidence files, relative to the worktree.
fn evidence_images(worktree: &Worktree, attempt: u32) -> Vec<String> {
    evidence::list(&worktree.path)
        .into_iter()
        .filter(|f| f.attempt == attempt)
        .map(|f| format!("{}/{}", evidence::folder(f.attempt), f.name))
        .collect()
}

enum Review {
    Pass,
    Fail(String),
}

/// Reads the reviewer's last `VERDICT:` line. No verdict counts as a fail, so
/// unreviewed work never slips through.
fn verdict(reply: &str) -> Review {
    let last = reply.lines().rev().find_map(|l| {
        l.trim()
            .trim_matches('`')
            .strip_prefix("VERDICT:")
            .map(str::trim)
    });
    match last {
        Some(v) if v.eq_ignore_ascii_case("pass") => Review::Pass,
        Some(_) => Review::Fail(reply.trim().to_string()),
        None => Review::Fail(format!(
            "The reviewer gave no verdict. Their reply:\n{}",
            reply.trim()
        )),
    }
}

enum GateEnd {
    /// Every gate that applies passed or was skipped.
    Pass,
    /// A gate failed; the author gets its notes.
    SendBack(String),
    /// The owner refused to pass a gate, or the run stopped; the ticket has moved on.
    Stop,
}

/// Runs each gate whose paths match the ticket's changed files, in order, and
/// stops at the first that fails.
async fn run_gates(
    studio: &Arc<Studio>,
    ticket: &Ticket,
    author: &Hire,
    worktree: &Worktree,
    gates: &[Gate],
    stop: &Stop,
    log: &RunLog,
) -> Result<GateEnd, StudioError> {
    let files: Vec<String> = worktree
        .changed_files()?
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    let diff = worktree.diff()?;
    let images = evidence_images(worktree, ticket.attempts);
    for gate in gates.iter().filter(|g| matches_any(&g.paths, &files)) {
        let critic = match studio.gate_hire(&gate.hire, author, ticket.attempts) {
            Ok(critic) => critic,
            Err(why) => {
                log.note(&format!("gate {} skipped: {why}", gate.name));
                continue;
            }
        };
        let runtime = studio.runtime(&critic.runtime)?;
        let run = AgentRun {
            studio,
            ticket,
            hire: &critic,
            runtime: runtime.as_ref(),
            log,
            stop,
            failed: 0,
            read_only: true,
        };
        log.note(&format!(
            "gate {} by {} ({})",
            gate.name, critic.id, critic.model
        ));
        studio.set_judge(&ticket.id, &critic.id.0, &critic.model, Some(&gate.name));
        let pairs = image_pairs(worktree, ticket.attempts, &files, gate, log);
        let measured = measurements(worktree, &files, gate, log).await;
        let request = gate_prompt(ticket, gate, &diff, &images, &pairs, &measured);
        let ended = run.go(worktree, &request, &mut None).await;
        studio.clear_judge(&ticket.id);
        let reply = match ended? {
            AgentEnd::Stopped => return Ok(GateEnd::Stop),
            AgentEnd::Done(reply) => reply,
        };
        let notes = match verdict(reply.as_deref().unwrap_or_default()) {
            Review::Pass => {
                log.note(&format!("gate {} passed", gate.name));
                continue;
            }
            Review::Fail(notes) => format!("{} gate: {notes}", gate.name),
        };
        log.note(&format!("gate {} failed: {notes}", gate.name));
        // This failure counts too: at `rounds` the owner decides.
        if studio.gate_failures(&ticket.id, &gate.name)? + 1 < gate.rounds {
            return Ok(GateEnd::SendBack(notes));
        }
        let decision = studio
            .ask(
                &ticket.id,
                &critic.id.0,
                "gate",
                &gate.name,
                json!({ "gate": gate.name, "rounds": gate.rounds }),
                vec![notes],
            )
            .await?;
        if let Decision::Deny { message: why, .. } = decision {
            log.note(&format!("gate {} refused: {why}", gate.name));
            return Ok(GateEnd::Stop);
        }
        log.note(&format!("gate {} passed by the owner", gate.name));
    }
    Ok(GateEnd::Pass)
}

/// Runs the project's capture command when the change calls for it. A
/// capture that crashes or hangs fails the checks.
async fn capture(
    repo: &Repo,
    worktree: &Worktree,
    config: &ProjectConfig,
    attempt: u32,
    log: &RunLog,
) -> Result<(), String> {
    if config.capture.is_empty() {
        return Ok(());
    }
    let files: Vec<String> = worktree
        .changed_files()
        .map_err(|e| e.to_string())?
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    if !config.capture_paths.is_empty() && !matches_any(&config.capture_paths, &files) {
        log.note("capture skipped: no changed file matches capture_paths");
        return Ok(());
    }
    // Worktrees made before evidence existed need the exclude too.
    repo.exclude(&format!("{}/", evidence::DIR))
        .map_err(|e| e.to_string())?;
    let out = worktree.path.join(evidence::folder(attempt));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).map_err(|e| format!("can't create {}: {e}", out.display()))?;
    checks::capture(
        &worktree.path,
        &config.capture,
        &out,
        &config.env_for(repo.root()),
    )
    .await
    .map_err(|detail| format!("The capture crashed or hung:\n{detail}"))?;
    let made: Vec<String> = evidence::list(&worktree.path)
        .into_iter()
        .filter(|f| f.attempt == attempt)
        .map(|f| format!("{}/{}", evidence::folder(attempt), f.name))
        .collect();
    log.note(&format!("evidence: {}", made.join(", ")));
    Ok(())
}

fn first_prompt(ticket: &Ticket) -> String {
    let mut prompt = format!("Ticket {}: {}", ticket.id, ticket.title);
    if !ticket.goal.trim().is_empty() {
        prompt.push_str(&format!("\n\nGoal: {}", ticket.goal));
    }
    if !ticket.detail.trim().is_empty() {
        prompt.push_str(&format!("\n\n{}", ticket.detail.trim()));
    }
    if let Some(feedback) = &ticket.feedback {
        prompt.push_str(&format!("\n\nThe owner sent this back with: {feedback}"));
    }
    if ticket.kind == TicketKind::Plan {
        let mut plan = format!(
            "You are planning the goal \"{}\" for project {}. Read the code to see what \
             it takes. File one ticket per independent, reviewable change with file_ticket, \
             each small enough for one agent run, with acceptance criteria in its detail. \
             Use ask_owner only if the goal is ambiguous. Don't change files. End with a \
             short summary of the plan.",
            ticket.goal, ticket.project
        );
        if !ticket.detail.trim().is_empty() {
            plan.push_str(&format!(
                "\n\nThe owner's brief:\n\n{}",
                ticket.detail.trim()
            ));
        }
        return plan;
    }
    prompt.push_str(
        "\n\nWork only in the current directory. Change files with the Edit and \
         Write tools: shell commands that write files wait for the owner's approval. \
         Leave changes uncommitted; the studio commits them, then runs the project's \
         checks and, if the project has one, its screenshot capture. \
         Builds and tests can take many minutes: give such commands a long timeout \
         and wait for them, since nothing keeps running after you finish. \
         Finish with a short summary of what you changed.",
    );
    prompt
}

/// The ticket title as the subject, cut to fit, and a trailer naming the
/// ticket. The agent's own summary stays in the run log and `studio export`:
/// it describes the session, not the commit.
fn commit_message(ticket: &Ticket) -> String {
    const SUBJECT: usize = 72;
    let title = ticket.title.trim();
    let subject = if title.chars().count() <= SUBJECT {
        title.to_string()
    } else {
        let cut: String = title.chars().take(SUBJECT - 3).collect();
        let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
        format!("{}...", cut.trim_end_matches([',', '.', ';', ':']))
    };
    let mut message = subject.clone();
    if subject != title {
        message.push_str(&format!("\n\n{title}"));
    }
    message.push_str(&format!("\n\nStudio-Ticket: {}", ticket.id));
    message
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use studio_core::ids::{HireId, ProjectId};

    #[test]
    fn waits_read_in_minutes_hours_or_days() {
        assert_eq!(wait_words(15 * 60_000), "15 min");
        assert_eq!(wait_words(15 * 60_000 - 1), "15 min");
        assert_eq!(wait_words(61 * 60_000), "1 h 1 min");
        assert_eq!(wait_words((3 * 24 + 4) * 60 * 60_000), "3 d 4 h");
    }

    fn ticket() -> Ticket {
        Ticket {
            id: TicketId::new("GAME-1"),
            project: ProjectId::new("game"),
            title: "Redraw the hero".into(),
            goal: String::new(),
            hire: HireId::new("dev"),
            state: TicketState::Reviewing,
            attempts: 1,
            base_branch: "master".into(),
            created_at: 0,
            held_from: None,
            feedback: None,
            kind: TicketKind::Work,
            parent: None,
            detail: String::new(),
            source: None,
        }
    }

    fn gate() -> Gate {
        Gate {
            name: "art".into(),
            paths: vec!["assets/**".into()],
            hire: "critic".into(),
            prompt: None,
            rounds: 3,
            measure: Vec::new(),
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }

    /// A repo at `dir` with `files` committed, as a worktree at that commit.
    fn committed(dir: &Path, files: &[&str]) -> Worktree {
        git(dir, &["init", "-q"]);
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("README"), "old").unwrap();
        for file in files {
            fs::write(dir.join(file), "old").unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        let base = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        Worktree {
            path: dir.to_path_buf(),
            branch: "studio/game-1".into(),
            base_commit: String::from_utf8_lossy(&base.stdout).trim().to_string(),
        }
    }

    fn changed(worktree: &Worktree) -> Vec<String> {
        worktree
            .changed_files()
            .unwrap()
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn writes_before_and_after_for_changed_images() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let worktree = committed(dir, &["assets/hero.PNG", "assets/notes.txt"]);
        fs::write(dir.join("assets/hero.PNG"), "new").unwrap();
        fs::write(dir.join("assets/notes.txt"), "new").unwrap();
        fs::write(dir.join("assets/fresh.gif"), "gif").unwrap();
        fs::write(dir.join("outside.png"), "x").unwrap();
        let files = changed(&worktree);
        let log = RunLog::new(dir, "GAME-1");

        let pairs = image_pairs(&worktree, 1, &files, &gate(), &log);

        let folder = evidence::folder(1);
        assert_eq!(
            pairs,
            vec![
                ImagePair {
                    path: "assets/fresh.gif".into(),
                    before: None,
                    after: format!("{folder}/after-assets-fresh-gif.gif"),
                },
                ImagePair {
                    path: "assets/hero.PNG".into(),
                    before: Some(format!("{folder}/before-assets-hero-PNG.png")),
                    after: format!("{folder}/after-assets-hero-PNG.png"),
                },
            ]
        );
        let read = |p: &str| fs::read_to_string(dir.join(p)).unwrap();
        assert_eq!(read(&pairs[1].after), "new");
        assert_eq!(read(pairs[1].before.as_deref().unwrap()), "old");
        assert_eq!(read(&pairs[0].after), "gif");
        assert!(
            !dir.join(&folder)
                .join("before-assets-fresh-gif.gif")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_never_carry_a_pair_out_of_the_worktree() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        fs::create_dir_all(&dir).unwrap();
        let secret = tmp.path().join("secret.png");
        fs::write(&secret, "secret").unwrap();
        let worktree = committed(&dir, &["assets/hero.png"]);
        let log = RunLog::new(&dir, "GAME-1");
        let folder = dir.join(evidence::folder(1));
        let target = folder.join("after-assets-hero-png.png");

        // A planted target is left alone, and so is what it points at.
        fs::create_dir_all(&folder).unwrap();
        fs::write(dir.join("assets/hero.png"), "new").unwrap();
        symlink(&secret, &target).unwrap();
        let pairs = image_pairs(&worktree, 1, &changed(&worktree), &gate(), &log);
        assert_eq!(pairs, vec![]);
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret");
        fs::remove_file(&target).unwrap();

        // A changed image that links out isn't copied in.
        fs::remove_file(dir.join("assets/hero.png")).unwrap();
        symlink(&secret, dir.join("assets/hero.png")).unwrap();
        let pairs = image_pairs(&worktree, 1, &changed(&worktree), &gate(), &log);
        assert_eq!(pairs, vec![]);
        assert!(!target.exists());
        fs::remove_file(dir.join("assets/hero.png")).unwrap();
        fs::write(dir.join("assets/hero.png"), "new").unwrap();

        // A link in place of the evidence folder isn't written through.
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::remove_dir_all(dir.join(evidence::DIR)).unwrap();
        symlink(&elsewhere, dir.join(evidence::DIR)).unwrap();
        let pairs = image_pairs(&worktree, 1, &changed(&worktree), &gate(), &log);
        assert_eq!(pairs, vec![]);
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn paths_with_the_same_key_keep_their_own_images() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let worktree = committed(dir, &[]);
        fs::write(dir.join("assets/a-b.png"), "dash").unwrap();
        fs::create_dir_all(dir.join("assets/a")).unwrap();
        fs::write(dir.join("assets/a/b.png"), "slash").unwrap();
        let log = RunLog::new(dir, "GAME-1");

        let pairs = image_pairs(&worktree, 1, &changed(&worktree), &gate(), &log);

        assert_eq!(pairs.len(), 2);
        assert_ne!(pairs[0].after, pairs[1].after);
        for pair in &pairs {
            let want = fs::read(dir.join(&pair.path)).unwrap();
            assert_eq!(fs::read(dir.join(&pair.after)).unwrap(), want);
        }
    }

    #[test]
    fn deleted_images_do_not_use_up_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let gone: Vec<String> = (0..MAX_PAIRS).map(|n| format!("assets/a{n}.png")).collect();
        let names: Vec<&str> = gone.iter().map(String::as_str).collect();
        let worktree = committed(dir, &names);
        for file in &gone {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::write(dir.join("assets/z.png"), "new").unwrap();
        let log = RunLog::new(dir, "GAME-1");

        let pairs = image_pairs(&worktree, 1, &changed(&worktree), &gate(), &log);

        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].path, "assets/z.png");
    }

    #[test]
    fn gate_prompt_lists_pairs_apart_from_screenshots() {
        let shots = vec![".studio-evidence/1/title.png".to_string()];
        let pairs = vec![
            ImagePair {
                path: "a.png".into(),
                before: Some(".studio-evidence/1/before-a-png.png".into()),
                after: ".studio-evidence/1/after-a-png.png".into(),
            },
            ImagePair {
                path: "b.png".into(),
                before: None,
                after: ".studio-evidence/1/after-b-png.png".into(),
            },
        ];
        let prompt = gate_prompt(&ticket(), &gate(), "diff", &shots, &pairs, &[]);
        let (changed, rest) = prompt
            .split_once(
                "Changed images, the shipped version beside the new one (read them to see them):\n",
            )
            .unwrap();
        assert!(changed.contains("Judge whether"));
        assert!(rest.starts_with(
            "- a.png: before .studio-evidence/1/before-a-png.png, after .studio-evidence/1/after-a-png.png\n\
             - b.png: before new file, after .studio-evidence/1/after-b-png.png\n\n"
        ));
        let (_, screenshots) = rest.split_once("Screenshots from running").unwrap();
        assert!(screenshots.contains("- .studio-evidence/1/title.png"));

        let plain = gate_prompt(&ticket(), &gate(), "diff", &shots, &[], &[]);
        assert!(!plain.contains("Changed images"));
        assert!(plain.contains("- .studio-evidence/1/title.png"));
    }

    #[test]
    fn gate_prompt_shows_measurements_only_when_there_are_any() {
        let measured = vec![
            "$ lint a.png\nok".to_string(),
            "$ lint b.png\n(exit 1)".to_string(),
        ];
        let prompt = gate_prompt(&ticket(), &gate(), "diff", &[], &[], &measured);
        assert!(prompt.contains(
            "Measurements:\n```\n$ lint a.png\nok\n```\n```\n$ lint b.png\n(exit 1)\n```\n\n"
        ));
        let plain = gate_prompt(&ticket(), &gate(), "diff", &[], &[], &[]);
        assert!(!plain.contains("Measurements"));
    }

    fn measuring(command: &[&str]) -> Gate {
        Gate {
            measure: command.iter().map(ToString::to_string).collect(),
            ..gate()
        }
    }

    #[tokio::test]
    async fn measures_matching_files_up_to_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let worktree = committed(dir, &[]);
        fs::write(dir.join("README"), "new").unwrap();
        for n in 1..=MAX_MEASURED + 1 {
            fs::write(dir.join(format!("assets/{n:02}.png")), "new").unwrap();
        }
        let files = changed(&worktree);
        let log = RunLog::new(dir, "GAME-1");

        let out = measurements(&worktree, &files, &measuring(&["echo", "{file}"]), &log).await;

        assert_eq!(out.len(), MAX_MEASURED);
        assert!(out.iter().all(|m| m.starts_with("$ echo assets/")));
        assert!(out[MAX_MEASURED - 1].starts_with("$ echo assets/12.png"));
    }

    #[tokio::test]
    async fn deleted_files_are_not_measured_and_leave_their_slots() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let names: Vec<String> = (1..=MAX_MEASURED)
            .map(|n| format!("assets/{n:02}.png"))
            .collect();
        let mut base: Vec<&str> = names.iter().map(String::as_str).collect();
        base.push("assets/z.png");
        let worktree = committed(dir, &base);
        for name in &names {
            fs::remove_file(dir.join(name)).unwrap();
        }
        fs::write(dir.join("assets/z.png"), "new").unwrap();
        let files = changed(&worktree);
        assert!(files.contains(&names[0]));
        let log = RunLog::new(dir, "GAME-1");

        let out = measurements(&worktree, &files, &measuring(&["echo", "{file}"]), &log).await;

        assert_eq!(out, vec!["$ echo assets/z.png\nassets/z.png\n"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn files_through_a_link_are_not_measured() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        fs::create_dir(&dir).unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let secret = outside.join("secret.png");
        fs::write(&secret, "secret").unwrap();
        let worktree = committed(&dir, &[]);
        symlink(&secret, dir.join("assets/hero.png")).unwrap();
        symlink(&outside, dir.join("assets/linked")).unwrap();
        let files = vec![
            "assets/hero.png".to_string(),
            "assets/linked/secret.png".to_string(),
        ];
        let log = RunLog::new(&dir, "GAME-1");

        let out = measurements(&worktree, &files, &measuring(&["cat", "{file}"]), &log).await;

        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn a_measure_without_a_file_runs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let worktree = committed(dir, &["assets/a.png", "assets/b.png"]);
        fs::write(dir.join("assets/a.png"), "new").unwrap();
        fs::write(dir.join("assets/b.png"), "new").unwrap();
        let files = changed(&worktree);
        let log = RunLog::new(dir, "GAME-1");

        let out = measurements(&worktree, &files, &measuring(&["echo", "all"]), &log).await;

        assert_eq!(out, vec!["$ echo all\nall\n"]);
    }

    #[tokio::test]
    async fn no_measure_command_runs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = committed(tmp.path(), &[]);
        let log = RunLog::new(tmp.path(), "GAME-1");

        let files = ["assets/a.png".to_string()];
        assert!(
            measurements(&worktree, &files, &gate(), &log)
                .await
                .is_empty()
        );
    }
}
