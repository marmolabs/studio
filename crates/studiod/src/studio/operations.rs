//! Project leads review their work, propose actions, and act with the owner's
//! approval or within the company's autonomy level. The review, proposal and
//! outcome are kept in `<data>/operations.json`.

use super::{Studio, StudioError, lock, other};
use crate::api::{
    Autonomy, ManagerAct, ManagerAction, ManagerReview, OperationState, OperationsSettings,
    OperationsView, Request, Response, RuntimeMode, TalkFrom,
};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studio_config::config::ProjectEntry;
use studio_core::ids::{HireId, ProjectId};
use studio_core::ticket::Ticket;

const REVIEW_USD: f64 = 0.75;
const REVIEW_TURNS: u32 = 12;
const MAX_ACTIONS: usize = 12;
const MAX_REVIEWS: usize = 50;
const MAX_AUDIT: usize = 100;
const LOOK_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct State {
    saved: Mutex<Option<Saved>>,
    problem: Mutex<Option<String>>,
    reviewing: Mutex<BTreeSet<String>>,
    looked: Mutex<Option<Instant>>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Saved {
    settings: OperationsSettings,
    reviews: Vec<ManagerReview>,
    actions: Vec<ManagerAction>,
    next_id: u64,
}

#[derive(Debug, Deserialize)]
struct ModelReview {
    summary: String,
    #[serde(default)]
    actions: Vec<ModelAction>,
}

#[derive(Debug, Deserialize)]
struct ModelAction {
    #[serde(default)]
    title: String,
    #[serde(default)]
    reason: String,
    act: ManagerAct,
}

impl Studio {
    pub(crate) fn runtime_mode(&self) -> Result<RuntimeMode, StudioError> {
        self.read_operations();
        if let Some(problem) = lock(&self.operations_state.problem).clone() {
            return Err(other(format!("new runs are stopped: {problem}")));
        }
        Ok(self.read_operations().settings.mode)
    }

    pub(super) fn operations(&self) -> OperationsView {
        let saved = self.read_operations();
        let mut reviews = saved.reviews;
        let mut actions = saved.actions;
        reviews.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
        actions.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
        OperationsView {
            settings: saved.settings,
            reviewing: lock(&self.operations_state.reviewing)
                .iter()
                .cloned()
                .collect(),
            reviews,
            actions,
            problem: lock(&self.operations_state.problem).clone(),
        }
    }

    fn operations_path(&self) -> PathBuf {
        self.paths.data_dir.join("operations.json")
    }

    fn load_operations(&self) -> Saved {
        let path = self.operations_path();
        let result = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Saved::default();
            }
            Err(error) => Err(error.to_string()),
        };
        match result {
            Ok(saved) => saved,
            Err(error) => {
                let problem = format!(
                    "{} is unreadable: {error}; it was not overwritten",
                    path.display()
                );
                eprintln!("operations: {problem}");
                *lock(&self.operations_state.problem) = Some(problem);
                Saved::default()
            }
        }
    }

    fn read_operations(&self) -> Saved {
        let mut kept = lock(&self.operations_state.saved);
        kept.get_or_insert_with(|| self.load_operations()).clone()
    }

    fn update_operations(&self, change: impl FnOnce(&mut Saved)) -> Result<Saved, StudioError> {
        let mut kept = lock(&self.operations_state.saved);
        let mut saved = kept.get_or_insert_with(|| self.load_operations()).clone();
        if let Some(problem) = lock(&self.operations_state.problem).clone() {
            return Err(other(problem));
        }
        change(&mut saved);
        saved.reviews.truncate(MAX_REVIEWS);
        retain_actions(&mut saved.actions);
        let path = self.operations_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| other(format!("can't keep manager operations: {error}")))?;
        }
        let text = serde_json::to_string_pretty(&saved)
            .map_err(|error| other(format!("can't keep manager operations: {error}")))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|error| other(format!("can't keep manager operations: {error}")))?;
        *kept = Some(saved.clone());
        Ok(saved)
    }

    pub(super) fn set_operations(
        &self,
        autopilot: Option<bool>,
        review_minutes: Option<u64>,
        autonomy: Option<Autonomy>,
        mode: Option<RuntimeMode>,
    ) -> Result<Response, StudioError> {
        if review_minutes.is_some_and(|minutes| !(5..=1440).contains(&minutes)) {
            return Err(other(
                "manager reviews must be between 5 minutes and 24 hours apart",
            ));
        }
        let saved = self.update_operations(|saved| {
            if let Some(autopilot) = autopilot {
                saved.settings.autopilot = autopilot;
            }
            if let Some(minutes) = review_minutes {
                saved.settings.review_minutes = minutes;
            }
            if let Some(autonomy) = autonomy {
                saved.settings.autonomy = autonomy;
            }
            if let Some(mode) = mode {
                saved.settings.mode = mode;
            }
        })?;
        Ok(Response::ok(saved.settings))
    }

    /// A process that stops after claiming an action leaves it marked as
    /// interrupted, never pending for an automatic second execution.
    pub(crate) fn recover_operations(&self) -> Result<(), StudioError> {
        let interrupted = self
            .read_operations()
            .actions
            .iter()
            .any(|action| action.state == OperationState::Executing);
        if interrupted {
            self.update_operations(|saved| {
                for action in saved
                    .actions
                    .iter_mut()
                    .filter(|action| action.state == OperationState::Executing)
                {
                    action.state = OperationState::Failed;
                    action.result =
                        "Interrupted by a restart; inspect the work before proposing it again"
                            .to_string();
                    action.resolved_at = Some(now_ms());
                }
            })?;
        }
        Ok(())
    }

    pub(super) fn review_operations(
        self: &Arc<Self>,
        project: &str,
    ) -> Result<Response, StudioError> {
        if let Some(problem) = self.operations().problem {
            return Err(other(problem));
        }
        let entry = self.project(&ProjectId::new(project))?;
        if self.lead_of(project).is_none() {
            return Err(other(format!("{project} has no lead to review it")));
        }
        if !lock(&self.operations_state.reviewing).insert(project.to_string()) {
            return Ok(Response::ok(json!({"project": project, "started": false})));
        }
        let studio = Arc::clone(self);
        let entry_clone = entry.clone();
        tokio::spawn(async move {
            studio.run_review(&entry_clone).await;
            lock(&studio.operations_state.reviewing).remove(&entry_clone.name);
        });
        Ok(Response::ok(json!({"project": project, "started": true})))
    }

    pub(super) fn spawn_operations(self: &Arc<Self>) {
        {
            let mut looked = lock(&self.operations_state.looked);
            match *looked {
                None => {
                    *looked = Some(Instant::now());
                    return;
                }
                Some(at) if at.elapsed() < LOOK_EVERY => return,
                Some(_) => *looked = Some(Instant::now()),
            }
        }
        let settings = self.read_operations().settings;
        if lock(&self.operations_state.problem).is_some() || !settings.autopilot || self.is_closed()
        {
            return;
        }
        let now = now_ms();
        let due = now.saturating_sub(settings.review_minutes.saturating_mul(60_000));
        let latest = self.operations();
        for entry in self.config().projects.clone() {
            let reviewed = latest
                .reviews
                .iter()
                .find(|review| review.project == entry.name)
                .is_some_and(|review| review.at > due);
            if !reviewed
                && self.lead_of(&entry.name).is_some()
                && let Err(error) = self.review_operations(&entry.name)
            {
                eprintln!("operations: {}: {error}", entry.name);
            }
        }
    }

    async fn run_review(self: &Arc<Self>, entry: &ProjectEntry) {
        let Some(lead) = self.lead_of(&entry.name) else {
            return;
        };
        let tickets = lock(&self.store).tickets().unwrap_or_default();
        let prompt = review_prompt(self, entry, &lead.id.0, &tickets);
        let charge = format!("{}/operations", entry.name);
        let run = LeadRun {
            project: &entry.name,
            hire: &lead,
            cwd: &entry.path,
            prompt,
            tools: Tools::ReadOnly,
            charge_task: Some(&charge),
            cap_usd: REVIEW_USD,
            max_turns: REVIEW_TURNS,
        };
        let at = now_ms();
        let outcome = leadrun::run(self, run).await;
        let (summary, proposed, usd) = match outcome {
            Ok(reply) => match parse_review(&reply.text) {
                Some(model) => (model.summary, model.actions, reply.usd),
                None => (reply.text, Vec::new(), reply.usd),
            },
            Err(error) => (format!("Review failed: {error}"), Vec::new(), 0.0),
        };
        if let Err(error) = self.save_review(entry, &lead.id.0, at, summary, proposed, usd) {
            eprintln!("operations: {}: {error}", entry.name);
        }
    }

    fn save_review(
        self: &Arc<Self>,
        entry: &ProjectEntry,
        lead: &str,
        at: u64,
        summary: String,
        proposed: Vec<ModelAction>,
        usd: f64,
    ) -> Result<(), StudioError> {
        let summary = summary.trim().to_string();
        let mut proposed = proposed.into_iter().take(MAX_ACTIONS).collect::<Vec<_>>();
        proposed.retain(|action| self.valid_action(entry, &action.act));
        let mut review = ManagerReview {
            id: String::new(),
            project: entry.name.clone(),
            lead: lead.to_string(),
            at,
            summary: summary.clone(),
            usd,
        };
        let autonomy = self.read_operations().settings.autonomy;
        let mut actions = Vec::new();
        self.update_operations(|saved| {
            saved.next_id = saved.next_id.saturating_add(1);
            review.id = format!("r{}", saved.next_id);
            for action in proposed {
                saved.next_id = saved.next_id.saturating_add(1);
                let title = if action.title.trim().is_empty() {
                    action.act.label().to_string()
                } else {
                    action.title.trim().to_string()
                };
                actions.push(ManagerAction {
                    id: format!("a{}", saved.next_id),
                    review: review.id.clone(),
                    project: entry.name.clone(),
                    lead: lead.to_string(),
                    title,
                    reason: action.reason.trim().to_string(),
                    act: action.act,
                    state: OperationState::Pending,
                    at,
                    resolved_at: None,
                    result: String::new(),
                });
            }
            saved.reviews.insert(0, review);
            saved.actions.extend(actions.clone());
        })?;
        if !summary.is_empty() {
            self.post(
                &entry.name,
                TalkFrom::Lead {
                    hire: lead.to_string(),
                },
                &summary,
                None,
            )?;
            self.post_room(
                &entry.name,
                TalkFrom::Lead {
                    hire: lead.to_string(),
                },
                &format!("Manager review: {summary}"),
            )?;
        }
        for action in actions {
            if automatic(autonomy, &action.act)
                && let Err(error) = self.execute_operation(&action.id, None)
            {
                eprintln!("operations: {}: {error}", action.id);
            }
        }
        Ok(())
    }

    fn valid_action(&self, entry: &ProjectEntry, act: &ManagerAct) -> bool {
        let task_ok = act.task().is_some_and(|task| {
            self.ticket(&studio_core::ids::TicketId::new(task))
                .is_ok_and(|ticket| ticket.project.0 == entry.name)
        });
        match act {
            ManagerAct::StartGoal { goal, .. } => !goal.trim().is_empty(),
            ManagerAct::CreateTask { title, hire, .. } => {
                !title.trim().is_empty()
                    && hire.as_deref().is_none_or(|hire| {
                        !hire.trim().is_empty() && self.hire(&HireId::new(hire)).is_ok()
                    })
            }
            ManagerAct::SendBack { note, .. } => task_ok && !note.trim().is_empty(),
            ManagerAct::Tell { text, .. } => task_ok && !text.trim().is_empty(),
            ManagerAct::Pause { .. } | ManagerAct::Resume { .. } | ManagerAct::Cancel { .. } => {
                task_ok
            }
        }
    }

    pub(super) fn decide_operation(
        self: &Arc<Self>,
        action: &str,
        approve: bool,
        note: Option<String>,
    ) -> Result<Response, StudioError> {
        if approve {
            self.execute_operation(action, note)
        } else {
            let action = self.change_operation(action, |action| {
                action.state = OperationState::Rejected;
                action.result = note.unwrap_or_else(|| "Rejected by the owner".to_string());
            })?;
            Ok(Response::ok(action))
        }
    }

    fn change_operation(
        &self,
        id: &str,
        change: impl FnOnce(&mut ManagerAction),
    ) -> Result<ManagerAction, StudioError> {
        let mut found = None;
        self.update_operations(|saved| {
            if let Some(action) = saved
                .actions
                .iter_mut()
                .find(|action| action.id == id && action.state == OperationState::Pending)
            {
                change(action);
                action.resolved_at = Some(now_ms());
                found = Some(action.clone());
            }
        })?;
        found.ok_or_else(|| other(format!("no pending manager action {id}")))
    }

    fn claim_operation(&self, id: &str) -> Result<ManagerAction, StudioError> {
        let mut found = None;
        self.update_operations(|saved| {
            if let Some(action) = saved
                .actions
                .iter_mut()
                .find(|action| action.id == id && action.state == OperationState::Pending)
            {
                action.state = OperationState::Executing;
                found = Some(action.clone());
            }
        })?;
        found.ok_or_else(|| other(format!("no pending manager action {id}")))
    }

    fn finish_operation(
        &self,
        id: &str,
        change: impl FnOnce(&mut ManagerAction),
    ) -> Result<ManagerAction, StudioError> {
        let mut found = None;
        self.update_operations(|saved| {
            if let Some(action) = saved
                .actions
                .iter_mut()
                .find(|action| action.id == id && action.state == OperationState::Executing)
            {
                change(action);
                action.resolved_at = Some(now_ms());
                found = Some(action.clone());
            }
        })?;
        found.ok_or_else(|| other(format!("manager action {id} is not executing")))
    }

    fn execute_operation(
        self: &Arc<Self>,
        id: &str,
        note: Option<String>,
    ) -> Result<Response, StudioError> {
        let action = self.claim_operation(id)?;
        let project = action.project.clone();
        let response = match &action.act {
            ManagerAct::StartGoal { goal, detail } => self.dispatch(Request::CreateGoal {
                project,
                text: goal.clone(),
                hire: Some(action.lead.clone()),
                detail: detail.clone(),
                version: None,
            }),
            ManagerAct::CreateTask {
                title,
                detail,
                hire,
            } => self.dispatch(Request::CreateTicket {
                project,
                title: title.clone(),
                goal: String::new(),
                hire: hire.clone().unwrap_or_else(|| action.lead.clone()),
                base: None,
                detail: detail.clone(),
            }),
            ManagerAct::SendBack { task, note } => self.lead_acts(
                &project,
                task,
                crate::api::LeadAct::SendBack { note: note.clone() },
            ),
            ManagerAct::Pause { task } => {
                self.lead_acts(&project, task, crate::api::LeadAct::Pause)
            }
            ManagerAct::Resume { task } => {
                self.lead_acts(&project, task, crate::api::LeadAct::Resume)
            }
            ManagerAct::Tell { task, text } => self.lead_acts(
                &project,
                task,
                crate::api::LeadAct::Tell { text: text.clone() },
            ),
            ManagerAct::Cancel { task } => {
                self.lead_acts(&project, task, crate::api::LeadAct::Cancel)
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                let message = error.to_string();
                self.finish_operation(id, |action| {
                    action.state = OperationState::Failed;
                    action.result = message;
                })?;
                return Err(error);
            }
        };
        if !response.ok {
            let error = response
                .error
                .unwrap_or_else(|| "action failed".to_string());
            self.finish_operation(id, |action| {
                action.state = OperationState::Failed;
                action.result = error.clone();
            })?;
            return Err(other(error));
        }
        let mut result = response.data["id"]
            .as_str()
            .map(|id| format!("Created {id}"))
            .unwrap_or_else(|| "Completed".to_string());
        if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
            result.push_str(&format!(" · {note}"));
        }
        let action = self.finish_operation(id, |action| {
            action.state = OperationState::Executed;
            action.result = result.clone();
        })?;
        self.post(
            &action.project,
            TalkFrom::Studio,
            &format!("Manager action: {}. {result}", action.title),
            None,
        )?;
        self.post_room(
            &action.project,
            TalkFrom::Studio,
            &format!("Manager action: {}. {result}", action.title),
        )?;
        Ok(Response::ok(action))
    }
}

fn retain_actions(actions: &mut Vec<ManagerAction>) {
    let (mut active, mut resolved): (Vec<_>, Vec<_>) =
        std::mem::take(actions).into_iter().partition(|action| {
            matches!(
                action.state,
                OperationState::Pending | OperationState::Executing
            )
        });
    let excess = resolved.len().saturating_sub(MAX_AUDIT);
    resolved.drain(..excess);
    active.extend(resolved);
    *actions = active;
}

fn automatic(autonomy: Autonomy, act: &ManagerAct) -> bool {
    match autonomy {
        Autonomy::Supervised => false,
        Autonomy::Balanced => matches!(act, ManagerAct::SendBack { .. } | ManagerAct::Tell { .. }),
        Autonomy::Autonomous => !matches!(act, ManagerAct::Cancel { .. }),
    }
}

fn review_prompt(studio: &Studio, entry: &ProjectEntry, lead: &str, tickets: &[Ticket]) -> String {
    let results = studio.results(tickets);
    let mut work = tickets
        .iter()
        .filter(|ticket| ticket.project.0 == entry.name)
        .rev()
        .take(40)
        .map(|ticket| {
            let mut line = format!(
                "- {}: \"{}\" — {}, assigned to {}. Goal: {}.",
                ticket.id,
                ticket.title.replace('\n', " "),
                ticket.state.kind(),
                ticket.hire,
                ticket.goal.replace('\n', " ")
            );
            if let Some(result) = results.get(&ticket.id.to_string()) {
                line.push_str(&format!(" Result: {result}"));
            }
            if let Some(feedback) = &ticket.feedback {
                line.push_str(&format!(" Feedback: {feedback}"));
            }
            line
        })
        .collect::<Vec<_>>();
    if work.is_empty() {
        work.push("- No tasks yet.".to_string());
    }
    let budget = studio.config().budget.daily_usd;
    format!(
        "You are the manager of {project}. Review the project and decide what should happen next. \
         Read files when useful. Return only a JSON object with this shape: \
         {{\"summary\":\"two short sentences for the owner\",\"actions\":[{{\"title\":\"short action\",\
         \"reason\":\"why it matters\",\"act\":{{\"do\":\"tell\",\"task\":\"TASK-ID\",\"text\":\"message\"}}}}]}}. \
         Allowed act forms are start_goal {{goal,detail}}, create_task {{title,detail,hire}}, \
         send_back {{task,note}}, pause {{task}}, resume {{task}}, tell {{task,text}}, and cancel {{task}}. \
         Propose no more than four useful actions; an empty actions array is correct when no action is needed. \
         Never invent task or hire ids. Do not propose shipping, releasing, changing files, or spending money. \
         Prefer unblocking existing work over creating more. Use plain text inside the JSON, not markdown.\n\n\
         Project: {project}\nManager hire id: {lead}\nCompany daily budget: ${budget:.2}\n\n## Work\n{work}",
        project = entry.name,
        work = work.join("\n")
    )
}

fn parse_review(text: &str) -> Option<ModelReview> {
    let text = text.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|text| text.strip_suffix("```"))
        .unwrap_or(text)
        .trim();
    serde_json::from_str(text).ok().or_else(|| {
        let start = text.find('{')?;
        let end = text.rfind('}')?;
        serde_json::from_str(&text[start..=end]).ok()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use std::collections::HashMap;
    use studio_config::StudioPaths;
    use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};

    struct Company {
        _home: tempfile::TempDir,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
    }

    fn company(reply: &str) -> Company {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("shop");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 10.0\n\n[concurrency]\nmax_runs = 2\n\n\
                 [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\nlead = \"nora\"\n",
                repo.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        for (id, role) in [("nora", "lead"), ("dev", "engineer")] {
            std::fs::write(
                home.path().join(format!("hires/{id}.md")),
                format!("---\nname: {id}\nrole: {role}\nruntime: fake\nmodel: fake-1\n---\nYou work here.\n"),
            )
            .unwrap();
        }
        let runtime = FakeRuntime::new(vec![RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost {
                usd: 0.04,
                reported: true,
                ..Cost::default()
            },
            summary: Some(reply.into()),
        }]);
        let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        runtimes.insert("fake".into(), Arc::new(runtime.clone()));
        let studio = Studio::open(
            StudioPaths::resolve(Some(home.path().to_path_buf())),
            runtimes,
        )
        .unwrap();
        Company {
            _home: home,
            studio,
            runtime,
        }
    }

    #[tokio::test]
    async fn a_supervised_review_proposes_work_and_approval_executes_it() {
        let company = company(
            r#"{"summary":"Checkout needs a next step.","actions":[{"title":"Build the drawer","reason":"It is the next milestone.","act":{"do":"create_task","title":"Cart drawer","detail":"Keep it keyboard accessible.","hire":"dev"}}]}"#,
        );
        let started = company
            .studio
            .handle(Request::ReviewOperations {
                project: "shop".into(),
            })
            .await;
        assert!(started.ok, "{:?}", started.error);
        for _ in 0..100 {
            if !company.studio.operations().actions.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let operations = company.studio.operations();
        assert_eq!(operations.reviews[0].summary, "Checkout needs a next step.");
        assert_eq!(operations.actions.len(), 1);
        assert_eq!(operations.actions[0].state, OperationState::Pending);
        assert!(lock(&company.studio.store).tickets().unwrap().is_empty());
        assert!(
            company.runtime.specs()[0]
                .prompt
                .contains("Return only a JSON object")
        );
        assert_eq!(company.runtime.specs()[0].mcp_config, None);

        let approved = company
            .studio
            .handle(Request::DecideOperation {
                action: operations.actions[0].id.clone(),
                approve: true,
                note: None,
            })
            .await;
        assert!(approved.ok, "{:?}", approved.error);
        let tickets = lock(&company.studio.store).tickets().unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].title, "Cart drawer");
        assert_eq!(tickets[0].hire.0, "dev");
        assert_eq!(
            company.studio.operations().actions[0].state,
            OperationState::Executed
        );
        let budget = company.studio.handle(Request::Budget).await;
        assert_eq!(
            budget.data["spent_24h"]["by_ticket"]["shop/operations"],
            0.04
        );
    }

    #[tokio::test]
    async fn simultaneous_approvals_execute_an_action_only_once() {
        let company = company(
            r#"{"summary":"Delegate once.","actions":[{"title":"Build once","act":{"do":"create_task","title":"Only one","detail":"","hire":"dev"}}]}"#,
        );
        let entry = company.studio.project(&ProjectId::new("shop")).unwrap();
        company.studio.run_review(&entry).await;
        let action = company.studio.operations().actions[0].id.clone();
        let decide = || Request::DecideOperation {
            action: action.clone(),
            approve: true,
            note: None,
        };
        let (first, second) = tokio::join!(
            company.studio.handle(decide()),
            company.studio.handle(decide()),
        );
        assert_ne!(first.ok, second.ok, "exactly one approval must win");
        assert_eq!(lock(&company.studio.store).tickets().unwrap().len(), 1);
        assert_eq!(
            company.studio.operations().actions[0].state,
            OperationState::Executed
        );
    }

    #[tokio::test]
    async fn an_interrupted_execution_never_becomes_pending_again() {
        let company = company(
            r#"{"summary":"Delegate once.","actions":[{"title":"Build once","act":{"do":"create_task","title":"Only one","detail":"","hire":"dev"}}]}"#,
        );
        let entry = company.studio.project(&ProjectId::new("shop")).unwrap();
        company.studio.run_review(&entry).await;
        let action = company.studio.operations().actions[0].id.clone();
        company.studio.claim_operation(&action).unwrap();
        company.studio.recover_operations().unwrap();
        let action = &company.studio.operations().actions[0];
        assert_eq!(action.state, OperationState::Failed);
        assert!(action.result.contains("Interrupted"));
    }

    #[tokio::test]
    async fn an_action_the_engine_refuses_is_marked_failed_not_left_pending() {
        let company = company("Never called");
        let made = company
            .studio
            .handle(Request::CreateTicket {
                project: "shop".into(),
                title: "Queued work".into(),
                goal: String::new(),
                hire: "dev".into(),
                base: None,
                detail: String::new(),
            })
            .await;
        assert!(made.ok, "{:?}", made.error);
        let task = made.data["id"].as_str().unwrap().to_string();
        company
            .studio
            .update_operations(|saved| {
                saved.actions.push(ManagerAction {
                    id: "a1".into(),
                    review: "r1".into(),
                    project: "shop".into(),
                    lead: "nora".into(),
                    title: "Resume too early".into(),
                    reason: String::new(),
                    act: ManagerAct::Resume { task },
                    state: OperationState::Pending,
                    at: now_ms(),
                    resolved_at: None,
                    result: String::new(),
                });
            })
            .unwrap();
        let decided = company
            .studio
            .handle(Request::DecideOperation {
                action: "a1".into(),
                approve: true,
                note: None,
            })
            .await;
        assert!(!decided.ok);
        let action = &company.studio.operations().actions[0];
        assert_eq!(action.state, OperationState::Failed);
        assert!(!action.result.is_empty());
    }

    #[tokio::test]
    async fn autonomy_and_rejections_are_enforced_and_kept() {
        let company = company(
            r#"{"summary":"A task should stop.","actions":[{"title":"Cancel it","act":{"do":"cancel","task":"SHOP-1"}}]}"#,
        );
        company
            .studio
            .set_operations(Some(true), Some(5), Some(Autonomy::Autonomous), None)
            .unwrap();
        company
            .studio
            .run_review(&company.studio.project(&ProjectId::new("shop")).unwrap())
            .await;
        // The unknown task is never accepted, and cancellation would still need approval.
        assert!(company.studio.operations().actions.is_empty());
        let settings = company.studio.read_operations().settings;
        assert!(settings.autopilot);
        assert_eq!(settings.review_minutes, 5);
        assert_eq!(settings.autonomy, Autonomy::Autonomous);
        let path = company.studio.paths.data_dir.join("operations.json");
        assert!(path.exists());
        assert!(
            company
                .studio
                .set_operations(None, Some(1), None, None)
                .is_err()
        );
    }

    #[tokio::test]
    async fn simulation_replaces_every_configured_runtime_with_zero_cost_replies() {
        let company = company("Never called");
        company
            .studio
            .set_operations(None, None, None, Some(RuntimeMode::Simulation))
            .unwrap();
        let entry = company.studio.project(&ProjectId::new("shop")).unwrap();
        company.studio.run_review(&entry).await;
        assert!(company.runtime.specs().is_empty());
        let operations = company.studio.operations();
        assert!(
            operations.reviews[0]
                .summary
                .contains("Simulation review complete")
        );
        assert!(operations.actions.is_empty());
        assert_eq!(operations.reviews[0].usd, 0.0);
        assert!(company.studio.runtime("not-configured").is_ok());
    }

    #[test]
    fn audit_retention_keeps_unresolved_and_the_newest_resolved_actions() {
        let action = |id: usize, state| ManagerAction {
            id: format!("a{id}"),
            review: "r1".into(),
            project: "shop".into(),
            lead: "nora".into(),
            title: format!("Action {id}"),
            reason: String::new(),
            act: ManagerAct::Pause { task: "S-1".into() },
            state,
            at: id as u64,
            resolved_at: None,
            result: String::new(),
        };
        let mut actions = (0..=MAX_AUDIT)
            .map(|id| action(id, OperationState::Executed))
            .collect::<Vec<_>>();
        actions.push(action(MAX_AUDIT + 1, OperationState::Pending));
        retain_actions(&mut actions);
        assert_eq!(actions.len(), MAX_AUDIT + 1);
        assert!(!actions.iter().any(|action| action.id == "a0"));
        assert!(
            actions
                .iter()
                .any(|action| action.id == format!("a{}", MAX_AUDIT + 1))
        );
        actions.push(action(MAX_AUDIT + 2, OperationState::Executing));
        retain_actions(&mut actions);
        assert!(
            actions
                .iter()
                .any(|action| action.id == format!("a{}", MAX_AUDIT + 2))
        );
    }

    #[test]
    fn malformed_operations_storage_is_preserved_and_fails_closed() {
        let company = company("Never called");
        let path = company.studio.paths.data_dir.join("operations.json");
        std::fs::write(&path, "not json").unwrap();
        let reopened = Studio::open(
            StudioPaths::resolve(Some(company._home.path().to_path_buf())),
            HashMap::new(),
        )
        .unwrap();
        assert!(reopened.operations().problem.is_some());
        assert!(
            reopened
                .set_operations(Some(true), None, None, None)
                .is_err()
        );
        assert!(reopened.runtime("fake").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not json");
    }

    #[test]
    fn model_replies_may_be_plain_or_fenced_json() {
        assert!(parse_review("A quiet project.").is_none());
        let parsed = parse_review("```json\n{\"summary\":\"Fine.\",\"actions\":[]}\n```").unwrap();
        assert_eq!(parsed.summary, "Fine.");
        assert!(!automatic(
            Autonomy::Supervised,
            &ManagerAct::Pause { task: "S-1".into() }
        ));
        assert!(automatic(
            Autonomy::Balanced,
            &ManagerAct::Tell {
                task: "S-1".into(),
                text: "Hi".into()
            }
        ));
        assert!(!automatic(
            Autonomy::Autonomous,
            &ManagerAct::Cancel { task: "S-1".into() }
        ));
    }
}
