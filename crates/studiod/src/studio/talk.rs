//! Conversations: what the owner and a project's lead say to each other.
//!
//! A conversation is the file `<data>/talk/<project>.jsonl`, with one
//! `TalkMessage` on each line, oldest first. Messages are only ever added, and
//! their ids count up from 1 in each project, so a page that has seen message
//! 7 asks for the ones after 7.
//!
//! The owner's messages start a turn of the project's lead: it reads the
//! project, answers in a message of its own, and acts through the studio's
//! tools, each of which posts a card (`Acted`; `Plan` once a goal the lead
//! started has had its tasks filed). Only one turn of a project runs at a
//! time; what the owner says meanwhile is answered by the next turn.

use super::briefing::named;
use super::{Studio, StudioError, file_stem, lock, other};
use crate::api::{
    ConversationView, LeadAct, PlanTask, Request, Response, TalkCard, TalkFrom, TalkMessage,
    TalkSummary,
};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_core::ids::{ProjectId, TicketId};
use studio_core::ticket::Ticket;
use studio_runtime::RunEvent;

/// The most a lead's turn may cost, in dollars.
pub(crate) const TALK_USD: f64 = 0.50;

/// The most turns of its own model a lead's turn may take.
const TALK_TURNS: u32 = 12;

/// What a project that has no lead is told, with where to give it one: its
/// conversation in the web UI, or the CLI.
fn no_lead(project: &str) -> String {
    format!(
        "{project} has no lead yet. Give it one in its conversation in the web UI, or with \
         `studio lead {project} <hire>`."
    )
}

/// How the agent names the studio's tools.
const TOOL_PREFIX: &str = "mcp__studio__";

/// What the conversations keep in memory.
#[derive(Default)]
pub(super) struct State {
    /// Held while a conversation is read or a message is numbered and added,
    /// so two posts never get the same id and a read never sees half a line.
    files: Mutex<()>,
    /// The projects a lead is working on a reply for, and how far each
    /// conversation has been answered.
    turns: Mutex<HashMap<String, Turns>>,
    /// Held while the list of goals a lead started is read or changed.
    goals: Mutex<()>,
}

/// One project's turns.
#[derive(Default)]
struct Turns {
    /// A turn is running or about to.
    running: bool,
    /// The id of the last message a turn has answered; none until one has.
    answered: Option<u64>,
}

impl Studio {
    /// `Request::Talk`: stores the owner's message in the project's
    /// conversation and answers it. Its lead answers in a turn of its own, as
    /// soon as the lead is free: one turn runs at a time, and a message sent
    /// during one is answered by the next. A project without a lead is an
    /// error.
    pub(super) fn talk(
        self: &Arc<Self>,
        project: &str,
        text: &str,
    ) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let text = text.trim();
        if text.is_empty() {
            return Err(other("there is nothing to send: write a message"));
        }
        if self.lead_of(project).is_none() {
            return Err(other(no_lead(project)));
        }
        let message = self.post(project, TalkFrom::Owner, text, None)?;
        self.wake_lead(project);
        Ok(Response::ok(message))
    }

    /// `Request::Conversation`: the messages of the project's conversation,
    /// after the one numbered `after` when it is given, and whether its lead
    /// is working on a reply.
    pub(super) fn conversation(
        &self,
        project: &str,
        after: Option<u64>,
    ) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        Ok(Response::ok(ConversationView {
            messages: self.messages(project, after)?,
            thinking: self.thinking(project),
        }))
    }

    /// `Request::LeadActs`: what the lead of `project` does to `task` while it
    /// talks with the owner. The task must be one of the project's own, as a
    /// lead has no say in another project's work, and the studio checks it
    /// here, whoever asks. Otherwise it is as when the owner does it, except
    /// that a message reaches the agent as coming from the lead, by name.
    pub(super) fn lead_acts(
        self: &Arc<Self>,
        project: &str,
        task: &str,
        act: LeadAct,
    ) -> Result<Response, StudioError> {
        self.project(&ProjectId::new(project))?;
        let lead = self
            .lead_of(project)
            .ok_or_else(|| other(no_lead(project)))?;
        match self.ticket(&TicketId::new(task)) {
            Ok(ticket) if ticket.project.0 == project => {}
            // A task that doesn't exist is as little the project's.
            _ => return Err(other(format!("{task} isn't one of {project}'s tasks."))),
        }
        let task = task.to_string();
        match act {
            LeadAct::SendBack { note } => self.dispatch(Request::RequestChanges {
                ticket: task,
                feedback: note,
            }),
            LeadAct::Pause => self.dispatch(Request::Pause { ticket: task }),
            LeadAct::Resume => self.dispatch(Request::Resume { ticket: task }),
            LeadAct::Cancel => self.dispatch(Request::Cancel { ticket: task }),
            LeadAct::Tell { text } => self.tell_from(&task, &text, &named(&lead)),
        }
    }

    /// Whether the lead of `project` is working on a reply.
    fn thinking(&self, project: &str) -> bool {
        lock(&self.talk_state.turns)
            .get(project)
            .is_some_and(|turns| turns.running)
    }

    /// Starts the lead's turns on `project`, unless a turn is under way, which
    /// then goes on to the new message when it is done.
    fn wake_lead(self: &Arc<Self>, project: &str) {
        {
            let mut turns = lock(&self.talk_state.turns);
            let turns = turns.entry(project.to_string()).or_default();
            if turns.running {
                return;
            }
            turns.running = true;
        }
        let studio = Arc::clone(self);
        let project = project.to_string();
        tokio::spawn(async move {
            loop {
                let answered = studio.turn(&project).await;
                let mut turns = lock(&studio.talk_state.turns);
                let turns = turns.entry(project.clone()).or_default();
                if answered.is_some() {
                    turns.answered = answered;
                }
                // The check and the end are one step, so a message that comes
                // in after the check starts a turn of its own.
                if answered.is_none() || studio.unanswered(&project, turns.answered).is_empty() {
                    turns.running = false;
                    return;
                }
            }
        });
    }

    /// The owner's messages in `project` that no turn has answered. Where no
    /// turn has yet (after a restart), those after the lead's last words.
    fn unanswered(&self, project: &str, answered: Option<u64>) -> Vec<TalkMessage> {
        let messages = self.messages(project, None).unwrap_or_default();
        let since = answered.unwrap_or_else(|| {
            messages
                .iter()
                .rev()
                .find(|m| matches!(m.from, TalkFrom::Lead { .. }))
                .map_or(0, |m| m.id)
        });
        messages
            .into_iter()
            .filter(|m| m.id > since && m.from == TalkFrom::Owner)
            .collect()
    }

    /// One turn: the lead answers what the owner has said since it last
    /// spoke. Answers the id of the last message it covered; none when there
    /// was nothing to answer or the conversation can't be read.
    async fn turn(self: &Arc<Self>, project: &str) -> Option<u64> {
        let answered = lock(&self.talk_state.turns)
            .get(project)
            .and_then(|turns| turns.answered);
        let messages = match self.messages(project, None) {
            Ok(messages) => messages,
            Err(e) => {
                eprintln!("talk: {project}: {e}");
                return None;
            }
        };
        let upto = messages.last()?.id;
        let new = self.unanswered(project, answered);
        if new.is_empty() {
            return None;
        }
        let history: Vec<TalkMessage> = messages
            .into_iter()
            .filter(|m| !new.iter().any(|n| n.id == m.id))
            .collect();
        let (Some(lead), Ok(entry)) = (
            self.lead_of(project),
            self.project(&ProjectId::new(project)),
        ) else {
            self.say(project, &no_lead(project));
            return Some(upto);
        };

        let mut watch = Watch {
            studio: self,
            project,
            calls: HashMap::new(),
            acted: 0,
        };
        let run = LeadRun {
            project,
            hire: &lead,
            cwd: &entry.path,
            prompt: self.talk_prompt(project, &history, &new),
            tools: Tools::Talk,
            charge_task: None,
            cap_usd: TALK_USD,
            max_turns: TALK_TURNS,
        };
        let reply = leadrun::run_with(self, run, &mut |event| watch.see(event)).await;
        let acted = watch.acted;
        match reply {
            Ok(reply) if !reply.text.is_empty() => {
                self.report(project, TalkFrom::Lead { hire: lead.id.0 }, &reply.text);
            }
            // What it did is on the cards.
            Ok(_) if acted > 0 => {}
            Ok(_) => self.say(project, &format!("{} had nothing to say.", named(&lead))),
            Err(why) => self.say(project, &sentence(&why)),
        }
        Some(upto)
    }

    /// A message from the studio itself.
    fn say(&self, project: &str, text: &str) {
        self.report(project, TalkFrom::Studio, text);
    }

    /// Posts a message with no card. A message that can't be kept is lost to
    /// the conversation but not to the log.
    fn report(&self, project: &str, from: TalkFrom, text: &str) {
        if let Err(e) = self.post(project, from, text, None) {
            eprintln!("talk: {project}: {e}");
        }
    }

    /// Notes that the goal `plan` was started by the lead of `project`, so
    /// that its plan gets a card once it has filed its tasks.
    fn watch_goal(&self, project: &str, plan: &str) {
        let _goals = lock(&self.talk_state.goals);
        let path = goals_file(&self.paths.data_dir, project);
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            })
            .and_then(|mut file| writeln!(file, "{plan}"));
        if let Err(e) = saved {
            eprintln!("talk: can't note the goal {plan}: {e}");
        }
    }

    /// Forgets the goal `plan` of `project` and answers whether the lead had
    /// started it.
    fn take_goal(&self, project: &str, plan: &str) -> bool {
        let _goals = lock(&self.talk_state.goals);
        let path = goals_file(&self.paths.data_dir, project);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let (taken, kept): (Vec<&str>, Vec<&str>) = text.lines().partition(|l| *l == plan);
        if taken.is_empty() {
            return false;
        }
        let rest: String = kept.iter().map(|l| format!("{l}\n")).collect();
        if let Err(e) = std::fs::write(&path, rest) {
            eprintln!("talk: can't forget the goal {plan}: {e}");
        }
        true
    }

    /// A plan ticket has finished its work, which is filing its tasks. If the
    /// lead started the goal, it says so in the conversation, with a card of
    /// the tasks.
    pub(crate) fn plan_filed(&self, plan: &Ticket) {
        let project = plan.project.0.as_str();
        if !self.take_goal(project, &plan.id.to_string()) {
            return;
        }
        let tasks: Vec<PlanTask> = self
            .tickets_of(&plan.id)
            .into_iter()
            .map(|t| PlanTask {
                title: t.title,
                hire: Some(t.hire.0),
            })
            .collect();
        let goal = super::goal_of(plan);
        let text = match tasks.len() {
            0 => format!("The plan for \"{goal}\" filed no tasks."),
            1 => format!("Here is the plan for \"{goal}\": one task."),
            n => format!("Here is the plan for \"{goal}\": {n} tasks."),
        };
        let card = TalkCard::Plan {
            goal: plan.id.to_string(),
            title: goal,
            version: plan.version.clone(),
            tasks,
        };
        let from = TalkFrom::Lead {
            hire: plan.hire.0.clone(),
        };
        if let Err(e) = self.post(project, from, &text, Some(card)) {
            eprintln!("talk: {project}: {e}");
        }
    }

    /// The tickets filed by the ticket `parent`, oldest first.
    fn tickets_of(&self, parent: &TicketId) -> Vec<Ticket> {
        let all = lock(&self.store).tickets().unwrap_or_default();
        all.into_iter()
            .filter(|t| t.parent.as_ref() == Some(parent))
            .collect()
    }

    /// Adds a message to the end of `project`'s conversation, numbered after
    /// the last, and answers it as stored. `card` is what it shows under its
    /// words.
    pub(super) fn post(
        &self,
        project: &str,
        from: TalkFrom,
        text: &str,
        card: Option<TalkCard>,
    ) -> Result<TalkMessage, StudioError> {
        let _files = lock(&self.talk_state.files);
        let path = file_of(&self.paths.data_dir, project);
        let kept = read(&path)?;
        let message = TalkMessage {
            id: last_id(&kept) + 1,
            at: now_ms(),
            from,
            text: text.to_string(),
            card,
        };
        let mut line = serde_json::to_string(&message)
            .map_err(|e| other(format!("can't keep the message: {e}")))?;
        line.push('\n');
        // A line cut short by a crash must not run into this one.
        if !kept.is_empty() && !kept.ends_with('\n') {
            line.insert(0, '\n');
        }
        let saved = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            })
            .and_then(|mut file| file.write_all(line.as_bytes()));
        saved.map_err(|e| other(format!("can't save the message to {}: {e}", path.display())))?;
        Ok(message)
    }

    /// The messages of `project`'s conversation, oldest first: only those after
    /// the one numbered `after` when it is given. A project nobody has talked
    /// about has none.
    pub(super) fn messages(
        &self,
        project: &str,
        after: Option<u64>,
    ) -> Result<Vec<TalkMessage>, StudioError> {
        let _files = lock(&self.talk_state.files);
        let kept = read(&file_of(&self.paths.data_dir, project))?;
        Ok(parse(&kept)
            .into_iter()
            .filter(|m| after.is_none_or(|after| m.id > after))
            .collect())
    }

    /// What the office shows of each project's conversation, by project name:
    /// its last message and how many it holds. A project nobody has talked
    /// about is left out. A conversation that can't be read is left out too,
    /// as the office is read often and must not fail for it.
    pub(super) fn talk_summaries(&self) -> BTreeMap<String, TalkSummary> {
        let config = self.config();
        // Read before the files are held: turns are never waited for there.
        let thinking: Vec<String> = lock(&self.talk_state.turns)
            .iter()
            .filter(|(_, turns)| turns.running)
            .map(|(project, _)| project.clone())
            .collect();
        let _files = lock(&self.talk_state.files);
        config
            .projects
            .iter()
            .filter_map(|project| {
                let kept = read(&file_of(&self.paths.data_dir, &project.name)).ok()?;
                let messages = parse(&kept);
                let summary = TalkSummary {
                    last: Some(messages.last()?.clone()),
                    thinking: thinking.contains(&project.name),
                    count: messages.len() as u64,
                };
                Some((project.name.clone(), summary))
            })
            .collect()
    }
}

/// What a lead's turn has shown so far: the studio's tools it called, so
/// that each can be told to the owner as a card once it has worked.
struct Watch<'a> {
    studio: &'a Studio,
    project: &'a str,
    /// The tools called and not yet answered, by call id: name and input.
    calls: HashMap<String, (String, Value)>,
    /// How many tools have worked.
    acted: u32,
}

impl Watch<'_> {
    /// Takes in one event of the turn. A tool that worked posts its card; one
    /// that failed posts none, as the lead says so in its reply.
    fn see(&mut self, event: &RunEvent) {
        match event {
            RunEvent::ToolCall { id, name, input } => {
                if let Some(tool) = name.strip_prefix(TOOL_PREFIX) {
                    self.calls
                        .insert(id.clone(), (tool.to_string(), input.clone()));
                }
            }
            RunEvent::ToolResult {
                id,
                output,
                is_error: false,
            } => {
                if let Some((tool, input)) = self.calls.remove(id) {
                    self.acted += 1;
                    self.worked(&tool, &input, output);
                }
            }
            RunEvent::ToolResult { id, .. } => {
                self.calls.remove(id);
            }
            _ => {}
        }
    }

    /// The tool `tool` worked, called with `input` and answering `output`.
    fn worked(&self, tool: &str, input: &Value, output: &str) {
        let said = |key: &str| input[key].as_str().unwrap_or_default().trim().to_string();
        let (action, note) = match tool {
            "start_goal" => {
                // The goal gets its card once its plan has filed the tasks.
                let started: Value = serde_json::from_str(output).unwrap_or_default();
                if let Some(plan) = started["goal"].as_str() {
                    self.studio.watch_goal(self.project, plan);
                }
                return;
            }
            "send_back" => ("sent back", said("note")),
            "pause" => ("paused", String::new()),
            "resume" => ("resumed", String::new()),
            "cancel" => ("cancelled", String::new()),
            "tell" => ("told", said("text")),
            _ => return,
        };
        let task = said("task");
        let id = TicketId::new(&task);
        let title = match self.studio.ticket(&id) {
            // A card belongs in the conversation of the task's own project.
            Ok(ticket) if ticket.project.0 != self.project => return,
            Ok(ticket) => ticket.title,
            Err(_) => task.clone(),
        };
        let text = match action {
            "sent back" => format!("Sent \"{title}\" back."),
            "told" => format!("Passed a message on to the agent on \"{title}\"."),
            other => {
                let mut done = other.to_string();
                done[..1].make_ascii_uppercase();
                format!("{done} \"{title}\".")
            }
        };
        let card = TalkCard::Acted {
            task,
            title,
            action: action.to_string(),
            note,
        };
        if let Err(e) = self
            .studio
            .post(self.project, TalkFrom::Studio, &text, Some(card))
        {
            eprintln!("talk: {}: {e}", self.project);
        }
    }
}

/// `text` as a sentence of its own: it starts with a capital and ends with a
/// full stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    out.push_str(chars.as_str());
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// Where the goals a project's lead started, and whose tasks are not filed
/// yet, are listed: one plan ticket id on each line.
fn goals_file(data_dir: &Path, project: &str) -> PathBuf {
    data_dir
        .join("talk")
        .join(format!("{}.goals", file_stem(project)))
}

/// Where a project's conversation is kept.
fn file_of(data_dir: &Path, project: &str) -> PathBuf {
    data_dir
        .join("talk")
        .join(format!("{}.jsonl", file_stem(project)))
}

/// What a conversation's file holds; none if there is no file.
fn read(path: &Path) -> Result<String, StudioError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(other(format!("can't read {}: {e}", path.display()))),
    }
}

/// The messages in a conversation's text, oldest first. A line that isn't a
/// message, such as one cut short by a crash, is skipped.
fn parse(text: &str) -> Vec<TalkMessage> {
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The id of the last message in a conversation's text; 0 for none.
fn last_id(text: &str) -> u64 {
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str::<TalkMessage>(line).ok())
        .map_or(0, |message| message.id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::api::{OfficeView, Request};
    use crate::studio::testing::studio;
    use serde_json::json;
    use studio_runtime::{Cost, FakeRuntime, Outcome};

    fn lead(hire: &str) -> TalkFrom {
        TalkFrom::Lead { hire: hire.into() }
    }

    #[test]
    fn messages_are_numbered_from_one_in_each_project() {
        let (_home, studio) = studio(&["shop", "api"]);
        let first = studio.post("shop", TalkFrom::Owner, "Hello", None).unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(first.from, TalkFrom::Owner);
        assert_eq!(first.text, "Hello");
        assert!(first.at > 0 && first.card.is_none());
        let card = TalkCard::Acted {
            task: "SHOP-3".into(),
            title: "Add a cart drawer".into(),
            action: "sent back".into(),
            note: "Too many colours.".into(),
        };
        let second = studio
            .post("shop", lead("nora"), "Sent it back.", Some(card.clone()))
            .unwrap();
        assert_eq!(second.id, 2);
        assert_eq!(second.card, Some(card));
        // Each project counts for itself.
        let elsewhere = studio
            .post("api", TalkFrom::Studio, "Set up", None)
            .unwrap();
        assert_eq!(elsewhere.id, 1);
        assert_eq!(studio.messages("shop", None).unwrap(), vec![first, second]);
        assert_eq!(studio.messages("api", None).unwrap(), vec![elsewhere]);
        // The numbering goes on from the last message.
        let third = studio.post("shop", lead("nora"), "More", None).unwrap();
        assert_eq!(third.id, 3);
    }

    #[test]
    fn a_message_is_one_json_line_in_the_projects_file() {
        let (home, studio) = studio(&["shop"]);
        studio
            .post("shop", TalkFrom::Owner, "Two\nlines", None)
            .unwrap();
        let plan = TalkCard::Plan {
            goal: "SHOP-9".into(),
            title: "Checkout flow".into(),
            version: Some("0.5".into()),
            tasks: vec![PlanTask {
                title: "Add a cart drawer".into(),
                hire: Some("dev".into()),
            }],
        };
        studio
            .post("shop", lead("nora"), "Planned it.", Some(plan))
            .unwrap();
        let text = std::fs::read_to_string(home.path().join("data/talk/shop.jsonl")).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[0]["from"]["who"], "owner");
        assert_eq!(lines[0]["text"], "Two\nlines");
        assert_eq!(lines[1]["from"]["hire"], "nora");
        assert_eq!(lines[1]["card"]["type"], "plan");
        assert_eq!(lines[1]["card"]["tasks"][0]["title"], "Add a cart drawer");
    }

    #[test]
    fn messages_come_after_the_one_a_page_has_seen() {
        let (_home, studio) = studio(&["shop"]);
        for text in ["one", "two", "three"] {
            studio.post("shop", TalkFrom::Owner, text, None).unwrap();
        }
        let texts = |after| -> Vec<String> {
            studio
                .messages("shop", after)
                .unwrap()
                .into_iter()
                .map(|m| m.text)
                .collect()
        };
        assert_eq!(texts(None), ["one", "two", "three"]);
        assert_eq!(texts(Some(0)), ["one", "two", "three"]);
        assert_eq!(texts(Some(1)), ["two", "three"]);
        assert_eq!(texts(Some(3)), Vec::<String>::new());
        assert!(studio.messages("api", None).unwrap().is_empty());
    }

    #[test]
    fn a_projects_name_cannot_lead_out_of_the_talk_folder() {
        let (home, studio) = studio(&[]);
        studio
            .post("../outside", TalkFrom::Owner, "Hello", None)
            .unwrap();
        assert_eq!(studio.messages("../outside", None).unwrap().len(), 1);
        assert!(!home.path().join("outside.jsonl").exists());
        assert!(!home.path().join("data/outside.jsonl").exists());
        assert!(home.path().join("data/talk/___outside.jsonl").exists());
    }

    #[test]
    fn a_line_cut_short_is_skipped_and_the_next_message_starts_a_new_one() {
        let (home, studio) = studio(&["shop"]);
        let first = studio.post("shop", TalkFrom::Owner, "One", None).unwrap();
        let path = home.path().join("data/talk/shop.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(r#"{"id":2,"at":5,"from":{"who":"ow"#);
        std::fs::write(&path, text).unwrap();
        assert_eq!(studio.messages("shop", None).unwrap(), vec![first.clone()]);
        let next = studio.post("shop", TalkFrom::Owner, "Two", None).unwrap();
        assert_eq!(next.id, 2);
        assert_eq!(studio.messages("shop", None).unwrap(), vec![first, next]);
    }

    #[test]
    fn the_office_sees_the_last_message_and_how_many_there_are() {
        let (_home, studio) = studio(&["shop", "api", "docs"]);
        assert!(studio.talk_summaries().is_empty());
        studio.post("shop", TalkFrom::Owner, "One", None).unwrap();
        studio.post("shop", lead("nora"), "Two", None).unwrap();
        let last = studio
            .post("shop", TalkFrom::Studio, "Three", None)
            .unwrap();
        studio.post("docs", TalkFrom::Owner, "Hi", None).unwrap();
        // A conversation of a project the studio doesn't have is not shown.
        studio.post("gone", TalkFrom::Owner, "Hi", None).unwrap();
        let summaries = studio.talk_summaries();
        assert_eq!(summaries.keys().collect::<Vec<_>>(), ["docs", "shop"]);
        let shop = &summaries["shop"];
        assert_eq!(shop.last.as_ref(), Some(&last));
        assert_eq!((shop.count, shop.thinking), (3, false));
        assert_eq!(summaries["docs"].count, 1);
    }

    #[tokio::test]
    async fn the_office_snapshot_carries_the_conversations_and_nothing_built_yet() {
        let (_home, studio) = studio(&["shop", "docs"]);
        studio.post("shop", TalkFrom::Owner, "Hi", None).unwrap();
        let last = studio.post("shop", lead("nora"), "Hello", None).unwrap();
        let seen = studio.handle(Request::Office).await;
        assert!(seen.ok, "{:?}", seen.error);
        let office: OfficeView = serde_json::from_value(seen.data).unwrap();
        assert_eq!(office.talk.keys().collect::<Vec<_>>(), ["shop"]);
        assert_eq!(office.talk["shop"].last.as_ref(), Some(&last));
        assert_eq!(office.talk["shop"].count, 2);
        // The rest comes with the parts that make it.
        assert!(office.results.is_empty() && office.standups.is_empty());
        assert!(office.ship_checks.is_empty() && office.setups.is_empty());
    }

    /// A studio whose project `shop` is led by Nora on a fake runtime.
    struct Shop {
        _home: tempfile::TempDir,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
    }

    fn shop(runtime: FakeRuntime) -> Shop {
        shop_with(runtime, 5.0)
    }

    fn shop_with(runtime: FakeRuntime, daily_usd: f64) -> Shop {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("shop");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = {daily_usd}\n\n[concurrency]\nmax_runs = 2\n\n\
                 [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\nlead = \"nora\"\n",
                repo.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        std::fs::write(
            home.path().join("hires/nora.md"),
            "---\nname: Nora\nrole: lead\nruntime: fake\nmodel: fake-1\ntools: [file_ticket]\n---\n\
             You lead the shop.\n",
        )
        .unwrap();
        let mut runtimes: HashMap<String, Arc<dyn studio_runtime::Runtime>> = HashMap::new();
        runtimes.insert("fake".into(), Arc::new(runtime.clone()));
        let paths = studio_config::StudioPaths::resolve(Some(home.path().to_path_buf()));
        let studio = Studio::open(paths, runtimes).unwrap();
        Shop {
            _home: home,
            studio,
            runtime,
        }
    }

    impl Shop {
        /// The owner says `text`; answers the stored message.
        async fn say(&self, text: &str) -> TalkMessage {
            let said = self
                .studio
                .handle(Request::Talk {
                    project: "shop".into(),
                    text: text.into(),
                })
                .await;
            assert!(said.ok, "{:?}", said.error);
            serde_json::from_value(said.data).unwrap()
        }

        async fn view(&self) -> ConversationView {
            let seen = self
                .studio
                .handle(Request::Conversation {
                    project: "shop".into(),
                    after: None,
                })
                .await;
            assert!(seen.ok, "{:?}", seen.error);
            serde_json::from_value(seen.data).unwrap()
        }

        /// What the conversation holds, as `who: text` lines.
        fn lines(&self) -> Vec<String> {
            self.studio
                .messages("shop", None)
                .unwrap()
                .into_iter()
                .map(|m| {
                    let who = match m.from {
                        TalkFrom::Owner => "owner".to_string(),
                        TalkFrom::Studio => "studio".to_string(),
                        TalkFrom::Lead { hire } => hire,
                    };
                    format!("{who}: {}", m.text)
                })
                .collect()
        }

        /// A task of the shop, by title; answers its id.
        async fn task(&self, title: &str) -> String {
            let made = self
                .studio
                .handle(Request::CreateTicket {
                    project: "shop".into(),
                    title: title.into(),
                    goal: String::new(),
                    hire: "nora".into(),
                    base: None,
                    detail: String::new(),
                })
                .await;
            assert!(made.ok, "{:?}", made.error);
            made.data["id"].as_str().unwrap().to_string()
        }

        /// The cards in the conversation, oldest first.
        fn cards(&self) -> Vec<TalkCard> {
            self.studio
                .messages("shop", None)
                .unwrap()
                .into_iter()
                .filter_map(|m| m.card)
                .collect()
        }
    }

    /// Waits until `check` holds. A hang is caught after ten seconds.
    async fn until(mut check: impl FnMut() -> bool) {
        for _ in 0..1000 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("it never came about");
    }

    fn started() -> RunEvent {
        RunEvent::Started {
            session: "s1".into(),
            model: Some("fake-1".into()),
        }
    }

    fn finished(summary: Option<&str>, usd: f64) -> RunEvent {
        RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost {
                usd,
                reported: usd > 0.0,
                input_tokens: 100,
                output_tokens: 20,
                cumulative: true,
            },
            summary: summary.map(String::from),
        }
    }

    /// A turn in which the lead only answers `reply`.
    fn answers(reply: &str) -> Vec<RunEvent> {
        vec![started(), finished(Some(reply), 0.07)]
    }

    /// The lead calls the studio's tool `tool` with `input`, and the tool
    /// answers `output`.
    fn uses(id: &str, tool: &str, input: Value, output: &str, is_error: bool) -> [RunEvent; 2] {
        [
            RunEvent::ToolCall {
                id: id.into(),
                name: format!("mcp__studio__{tool}"),
                input,
            },
            RunEvent::ToolResult {
                id: id.into(),
                output: output.into(),
                is_error,
            },
        ]
    }

    #[tokio::test]
    async fn the_owners_message_is_stored_and_answered_by_a_turn() {
        let shop = shop(FakeRuntime::new(answers(
            "It's going well. Two tasks are done.",
        )));
        let stored = shop.say("  How's it going?\n").await;
        assert_eq!((stored.id, stored.text.as_str()), (1, "How's it going?"));
        assert_eq!(stored.from, TalkFrom::Owner);
        // The turn is under way as soon as the message is taken.
        assert!(shop.view().await.thinking);

        until(|| shop.studio.messages("shop", None).unwrap().len() == 2).await;
        let view = shop.view().await;
        assert!(!view.thinking);
        assert_eq!(view.messages[0], stored);
        assert_eq!(view.messages[1].id, 2);
        assert_eq!(view.messages[1].from, lead("nora"));
        assert_eq!(
            view.messages[1].text,
            "It's going well. Two tasks are done."
        );
        assert!(view.messages[1].card.is_none());

        // Only the lead's reply comes after the message the page has seen.
        let later = shop
            .studio
            .handle(Request::Conversation {
                project: "shop".into(),
                after: Some(1),
            })
            .await;
        let view: ConversationView = serde_json::from_value(later.data).unwrap();
        assert_eq!(view.messages.len(), 1);

        // One turn, as Nora, in the project's repository, that cost something.
        let specs = shop.runtime.specs();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].cwd.ends_with("shop"));
        assert_eq!(specs[0].max_budget_usd, Some(0.5));
        assert_eq!(specs[0].max_turns, Some(12));
        let budget = shop.studio.handle(Request::Budget).await;
        assert_eq!(budget.data["spent_24h"]["by_ticket"]["shop/talk"], 0.07);
    }

    #[tokio::test]
    async fn the_turns_prompt_has_the_brief_the_state_and_the_new_message() {
        let shop = shop(FakeRuntime::new(answers("Fine.")));
        let task = shop.task("Add a cart drawer").await;
        shop.say("How's it going?").await;
        until(|| shop.runtime.specs().len() == 1).await;
        let prompt = shop.runtime.specs().remove(0).prompt;
        assert!(
            prompt.starts_with(
                "You lead shop for the owner. Reply in plain words, in the first person"
            )
        );
        assert!(prompt.contains("at most three short sentences of about 15 words each"));
        assert!(prompt.contains("## How shop stands"));
        assert!(prompt.contains("Add a cart drawer"));
        assert!(prompt.contains(&task));
        assert!(prompt.contains("Spent today: $0.00 of $5.00."));
        assert!(prompt.ends_with("## The owner's new message\nHow's it going?\n"));
        // The hire's own prompt goes in the system prompt.
        assert_eq!(
            shop.runtime.specs()[0].append_system_prompt.as_deref(),
            Some("You lead the shop.")
        );
    }

    #[test]
    fn the_prompt_shows_the_last_twenty_messages_and_what_is_new() {
        let shop = shop(FakeRuntime::new(answers("Fine.")));
        let said = |id: u64, from: TalkFrom, text: &str| TalkMessage {
            id,
            at: id,
            from,
            text: text.into(),
            card: None,
        };
        let mut before: Vec<TalkMessage> = (1..=25)
            .map(|n| said(n, TalkFrom::Owner, &format!("message {n}")))
            .collect();
        before.push(said(26, lead("nora"), "A reply\nin two lines"));
        before.push(TalkMessage {
            card: Some(TalkCard::Acted {
                task: "SHOP-1".into(),
                title: "Menu art".into(),
                action: "paused".into(),
                note: String::new(),
            }),
            ..said(27, TalkFrom::Studio, "Paused \"Menu art\".")
        });
        let new = [
            said(28, TalkFrom::Owner, "first new"),
            said(29, TalkFrom::Owner, "second new"),
        ];
        let prompt = shop.studio.talk_prompt("shop", &before, &new);
        // 20 of the 27 earlier messages: 8 to 27.
        assert!(!prompt.contains("message 7\n"));
        assert!(prompt.contains("Owner: message 8\n"));
        assert!(prompt.contains("Nora: A reply in two lines\n"));
        assert!(prompt.contains("Studio: Paused \"Menu art\". [paused: \"Menu art\"]\n"));
        assert!(prompt.ends_with("## The owner's new messages\nfirst new\nsecond new\n"));
        assert!(!prompt.contains("Owner: first new"));
    }

    #[tokio::test]
    async fn a_message_sent_during_a_turn_waits_for_the_next_turn() {
        let (runtime, release) =
            FakeRuntime::sequence(vec![answers("First answer."), answers("Second answer.")]).held();
        let shop = shop(runtime);
        shop.say("First question").await;
        until(|| shop.runtime.specs().len() == 1).await;
        let second = shop.say("Second question").await;
        assert_eq!(second.id, 2);
        // The lead is busy with the first: no second turn starts.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(shop.runtime.specs().len(), 1);
        assert!(shop.view().await.thinking);
        assert!(!shop.runtime.specs()[0].prompt.contains("Second question"));

        // The first turn ends; the next starts on its own, and answers the
        // second message.
        until(|| {
            release.notify_one();
            shop.lines().len() == 4
        })
        .await;
        assert_eq!(
            shop.lines(),
            [
                "owner: First question",
                "owner: Second question",
                "nora: First answer.",
                "nora: Second answer."
            ]
        );
        let specs = shop.runtime.specs();
        assert_eq!(specs.len(), 2);
        let prompt = &specs[1].prompt;
        assert!(prompt.contains("Owner: First question\nNora: First answer.\n"));
        assert!(prompt.ends_with("## The owner's new message\nSecond question\n"));
        until(|| !shop.studio.thinking("shop")).await;
    }

    #[tokio::test]
    async fn two_messages_in_one_turns_wait_are_answered_together() {
        let (runtime, release) =
            FakeRuntime::sequence(vec![answers("First answer."), answers("Both answered.")]).held();
        let shop = shop(runtime);
        shop.say("One").await;
        until(|| shop.runtime.specs().len() == 1).await;
        shop.say("Two").await;
        shop.say("Three").await;
        until(|| {
            release.notify_one();
            shop.lines().len() == 5
        })
        .await;
        assert_eq!(shop.runtime.specs().len(), 2);
        assert!(
            shop.runtime.specs()[1]
                .prompt
                .ends_with("## The owner's new messages\nTwo\nThree\n")
        );
    }

    #[tokio::test]
    async fn thinking_is_true_while_a_turn_runs_in_the_conversation_and_the_office() {
        let (runtime, release) = FakeRuntime::new(answers("Done.")).held();
        let shop = shop(runtime);
        assert!(!shop.view().await.thinking);
        shop.say("Hello").await;
        until(|| shop.runtime.specs().len() == 1).await;
        assert!(shop.view().await.thinking);
        assert!(shop.studio.talk_summaries()["shop"].thinking);
        let office = shop.studio.handle(Request::Office).await;
        let office: OfficeView = serde_json::from_value(office.data).unwrap();
        assert!(office.talk["shop"].thinking);

        until(|| {
            release.notify_one();
            shop.lines().len() == 2
        })
        .await;
        until(|| !shop.studio.thinking("shop")).await;
        assert!(!shop.view().await.thinking);
        assert!(!shop.studio.talk_summaries()["shop"].thinking);
    }

    #[tokio::test]
    async fn a_project_without_a_lead_gets_a_plain_error_and_keeps_nothing() {
        let (_home, studio) = studio(&["shop"]);
        let said = studio
            .handle(Request::Talk {
                project: "shop".into(),
                text: "Hello".into(),
            })
            .await;
        assert!(!said.ok);
        // It says where to give the project a lead.
        assert_eq!(
            said.error.as_deref(),
            Some(
                "shop has no lead yet. Give it one in its conversation in the web UI, or with \
                 `studio lead shop <hire>`."
            )
        );
        assert!(studio.messages("shop", None).unwrap().is_empty());
        assert!(studio.talk_summaries().is_empty());
    }

    #[tokio::test]
    async fn a_lead_that_is_not_a_hire_is_no_lead() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let config = shop.studio.paths.config_dir.join("studio.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, text.replace("lead = \"nora\"", "lead = \"ghost\"")).unwrap();
        shop.studio.reload_config();
        let said = shop
            .studio
            .handle(Request::Talk {
                project: "shop".into(),
                text: "Hello".into(),
            })
            .await;
        assert!(said.error.unwrap().contains("has no lead yet"));
    }

    #[tokio::test]
    async fn only_a_project_the_studio_has_and_a_real_message_are_taken() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let studio = &shop.studio;
        let blank = studio
            .handle(Request::Talk {
                project: "shop".into(),
                text: " \n ".into(),
            })
            .await;
        assert!(!blank.ok);
        let unknown = studio
            .handle(Request::Talk {
                project: "garden".into(),
                text: "Hello".into(),
            })
            .await;
        assert!(unknown.error.unwrap().contains("garden"));
        let nothing = studio
            .handle(Request::Conversation {
                project: "garden".into(),
                after: None,
            })
            .await;
        assert!(!nothing.ok);
        assert!(studio.messages("shop", None).unwrap().is_empty());
        assert!(shop.runtime.specs().is_empty());
    }

    #[tokio::test]
    async fn a_turn_that_cannot_answer_says_why_in_the_conversation() {
        // Less left in the day than a turn may cost.
        let shop = shop_with(FakeRuntime::new(answers("Hi.")), 0.4);
        shop.say("Hello").await;
        until(|| shop.lines().len() == 2).await;
        assert_eq!(shop.lines()[1], "studio: No budget left for Nora.");
        until(|| !shop.studio.thinking("shop")).await;

        let failed = Outcome::Failed {
            kind: studio_runtime::FailureKind::Crash,
            message: "the model fell over".into(),
        };
        let broken = shop_with(
            FakeRuntime::new(vec![
                started(),
                RunEvent::Finished {
                    outcome: failed,
                    session: None,
                    cost: Cost::default(),
                    summary: None,
                },
            ]),
            5.0,
        );
        broken.say("Hello").await;
        until(|| broken.lines().len() == 2).await;
        assert_eq!(
            broken.lines()[1],
            "studio: Nora failed: the model fell over."
        );
    }

    #[tokio::test]
    async fn a_turn_with_nothing_to_say_still_answers() {
        let silent = shop(FakeRuntime::new(vec![started(), finished(None, 0.01)]));
        silent.say("Hello").await;
        until(|| silent.lines().len() == 2).await;
        assert_eq!(silent.lines()[1], "studio: Nora had nothing to say.");
    }

    #[tokio::test]
    async fn each_tool_the_lead_uses_posts_a_card_from_the_studio() {
        // The tasks are the first two tickets of the project.
        let (drawer, menu) = ("SHOP-1", "SHOP-2");
        let mut events = vec![started()];
        for event in [
            uses(
                "t1",
                "send_back",
                json!({ "task": drawer, "note": "Too many colours." }),
                "{}",
                false,
            ),
            uses("t2", "pause", json!({ "task": menu }), "{}", false),
            uses("t3", "resume", json!({ "task": menu }), "{}", false),
            uses("t4", "cancel", json!({ "task": menu }), "{}", false),
            uses(
                "t5",
                "tell",
                json!({ "task": drawer, "text": "Use three." }),
                "{}",
                false,
            ),
            // A tool that failed did nothing, so there is no card for it.
            uses(
                "t6",
                "cancel",
                json!({ "task": drawer }),
                "no such task",
                true,
            ),
            // Nor for the agent's own tools, or one the studio doesn't know.
            [
                RunEvent::ToolCall {
                    id: "t7".into(),
                    name: "Read".into(),
                    input: json!({ "file_path": "src/main.rs" }),
                },
                RunEvent::ToolResult {
                    id: "t7".into(),
                    output: "fn main() {}".into(),
                    is_error: false,
                },
            ],
            uses("t8", "dance", json!({ "task": drawer }), "ok", false),
        ] {
            events.extend(event);
        }
        events.push(finished(Some("Done all of it."), 0.1));
        let shop = shop(FakeRuntime::new(events));
        let made = [
            shop.task("Add a cart drawer").await,
            shop.task("Menu art").await,
        ];
        assert_eq!(made, [drawer, menu]);
        shop.say("Fix the drawer, then pause the menu art.").await;
        until(|| {
            shop.lines()
                .last()
                .is_some_and(|l| l == "nora: Done all of it.")
        })
        .await;

        let acted = |task: &str, title: &str, action: &str, note: &str| TalkCard::Acted {
            task: task.into(),
            title: title.into(),
            action: action.into(),
            note: note.into(),
        };
        assert_eq!(
            shop.cards(),
            [
                acted(
                    "SHOP-1",
                    "Add a cart drawer",
                    "sent back",
                    "Too many colours."
                ),
                acted("SHOP-2", "Menu art", "paused", ""),
                acted("SHOP-2", "Menu art", "resumed", ""),
                acted("SHOP-2", "Menu art", "cancelled", ""),
                acted("SHOP-1", "Add a cart drawer", "told", "Use three."),
            ]
        );
        // The cards come from the studio, before the lead's own words.
        let messages = shop.studio.messages("shop", None).unwrap();
        assert!(messages[1..=5].iter().all(|m| m.from == TalkFrom::Studio));
        assert_eq!(messages[1].text, "Sent \"Add a cart drawer\" back.");
        assert_eq!(messages[2].text, "Paused \"Menu art\".");
        assert_eq!(
            messages[5].text,
            "Passed a message on to the agent on \"Add a cart drawer\"."
        );
        assert_eq!(messages[6].from, lead("nora"));
    }

    #[tokio::test]
    async fn a_lead_that_acts_and_says_nothing_leaves_it_to_the_cards() {
        let task = "SHOP-1";
        let mut events = vec![started()];
        events.extend(uses("t1", "pause", json!({ "task": task }), "{}", false));
        events.push(finished(None, 0.02));
        let shop = shop(FakeRuntime::new(events));
        shop.task("Menu art").await;
        shop.say("Pause the menu art").await;
        until(|| !shop.studio.thinking("shop")).await;
        assert_eq!(
            shop.lines(),
            ["owner: Pause the menu art", "studio: Paused \"Menu art\"."]
        );
    }

    impl Shop {
        /// The studio has the project `garden` too, whose lead is nobody here,
        /// with one task, "Water the beds"; answers its id.
        async fn garden_task(&self) -> String {
            let config = self.studio.paths.config_dir.join("studio.toml");
            let mut text = std::fs::read_to_string(&config).unwrap();
            text.push_str(&format!(
                "\n[[projects]]\nname = \"garden\"\npath = \"{}\"\n",
                self._home.path().join("garden").display()
            ));
            std::fs::write(&config, text).unwrap();
            self.studio.reload_config();
            let made = self
                .studio
                .handle(Request::CreateTicket {
                    project: "garden".into(),
                    title: "Water the beds".into(),
                    goal: String::new(),
                    hire: "nora".into(),
                    base: None,
                    detail: String::new(),
                })
                .await;
            assert!(made.ok, "{:?}", made.error);
            made.data["id"].as_str().unwrap().to_string()
        }

        /// What `act` does to `task` when the lead of the shop does it.
        async fn acts(&self, task: &str, act: LeadAct) -> Response {
            self.studio
                .handle(Request::LeadActs {
                    project: "shop".into(),
                    task: task.into(),
                    act,
                })
                .await
        }

        fn events_of(&self, task: &str) -> usize {
            lock(&self.studio.store)
                .events(&TicketId::new(task))
                .unwrap()
                .len()
        }
    }

    #[tokio::test]
    async fn a_lead_acts_only_on_the_tasks_of_its_own_project() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let mine = shop.task("Menu art").await;
        let theirs = shop.garden_task().await;
        assert_eq!((mine.as_str(), theirs.as_str()), ("SHOP-1", "GARD-1"));
        let events = shop.events_of(&theirs);

        // Every one of the five is refused, in plain words, before it does
        // anything.
        for act in [
            LeadAct::SendBack {
                note: "Redo it.".into(),
            },
            LeadAct::Pause,
            LeadAct::Resume,
            LeadAct::Cancel,
            LeadAct::Tell {
                text: "Hurry.".into(),
            },
        ] {
            let refused = shop.acts(&theirs, act.clone()).await;
            assert!(!refused.ok, "{act:?}");
            assert_eq!(
                refused.error.as_deref(),
                Some("GARD-1 isn't one of shop's tasks."),
                "{act:?}"
            );
        }
        let task = shop.studio.ticket(&TicketId::new(&theirs)).unwrap();
        assert_eq!(task.state.kind(), "queued");
        assert_eq!(shop.events_of(&theirs), events);
        assert!(shop.studio.take_kept(&theirs).is_empty());

        // A task that is nowhere is not the project's either.
        let nowhere = shop.acts("SHOP-9", LeadAct::Cancel).await;
        assert_eq!(
            nowhere.error.as_deref(),
            Some("SHOP-9 isn't one of shop's tasks.")
        );

        // The lead's own project's task it may act on, as the owner may.
        let told = shop
            .acts(
                &mine,
                LeadAct::Tell {
                    text: "Hurry.".into(),
                },
            )
            .await;
        assert!(told.ok, "{:?}", told.error);
        let cancelled = shop.acts(&mine, LeadAct::Cancel).await;
        assert!(cancelled.ok, "{:?}", cancelled.error);
        assert_eq!(
            shop.studio
                .ticket(&TicketId::new(&mine))
                .unwrap()
                .state
                .kind(),
            "cancelled"
        );
        // And what the owner's requests refuse, it is refused as they are: a
        // task with no agent running has nothing to pause.
        let task = shop.task("Logo").await;
        let pause = shop.acts(&task, LeadAct::Pause).await;
        assert!(pause.error.unwrap().contains("has no agent running"));
    }

    #[tokio::test]
    async fn a_leads_message_reaches_the_agent_as_from_the_lead() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let task = shop.task("Menu art").await;
        let told = shop
            .acts(
                &task,
                LeadAct::Tell {
                    text: "  Use three colours.  ".into(),
                },
            )
            .await;
        assert!(told.ok, "{:?}", told.error);
        assert_eq!(told.data, json!({"task": "SHOP-1", "how": "queued"}));
        // The owner's own message, by the plain request, is the owner's.
        let owner = shop
            .studio
            .handle(Request::Tell {
                task: task.clone(),
                text: "And fewer.".into(),
            })
            .await;
        assert!(owner.ok, "{:?}", owner.error);
        assert_eq!(
            shop.studio.take_kept(&task),
            [
                "Message from Nora: Use three colours.",
                "Message from the owner: And fewer."
            ]
        );
    }

    /// A goal the lead started: the plan ticket the tool made, as the tool
    /// answers it.
    async fn start_goal(shop: &Shop, text: &str) -> Ticket {
        let made = shop
            .studio
            .handle(Request::CreateGoal {
                project: "shop".into(),
                text: text.into(),
                hire: None,
                detail: String::new(),
                version: Some("0.5".into()),
            })
            .await;
        assert!(made.ok, "{:?}", made.error);
        serde_json::from_value(made.data).unwrap()
    }

    /// A task the plan `plan` filed.
    fn filed(shop: &Shop, plan: &Ticket, title: &str, hire: &str) {
        shop.studio
            .create_ticket(super::super::NewTicket {
                project: "shop".into(),
                title: title.into(),
                goal: plan.goal.clone(),
                hire: hire.into(),
                base: None,
                kind: studio_core::ticket::TicketKind::Work,
                parent: Some(plan.id.clone()),
                detail: String::new(),
                source: None,
                version: None,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn a_goal_the_lead_started_gets_a_plan_card_once_its_tasks_are_filed() {
        // The goal the lead starts is the first ticket of the project.
        let mut events = vec![started()];
        events.extend(uses(
            "t1",
            "start_goal",
            json!({ "text": "Add a cart drawer", "version": "0.5" }),
            r#"{"goal":"SHOP-1","title":"Add a cart drawer"}"#,
            false,
        ));
        events.push(finished(Some("I started it."), 0.05));
        let shop = shop(FakeRuntime::new(events));
        // What the tool does when the lead calls it.
        let plan = start_goal(&shop, "Add a cart drawer").await;
        assert_eq!(plan.id.0, "SHOP-1");
        shop.say("Plan a cart drawer").await;
        until(|| shop.lines().len() == 2).await;
        // The goal is started and answered, but its tasks are not filed yet.
        assert_eq!(shop.lines()[1], "nora: I started it.");
        assert!(shop.cards().is_empty());

        filed(&shop, &plan, "Draw the drawer", "nora");
        filed(&shop, &plan, "Wire it to the cart", "nora");
        shop.studio.plan_filed(&plan);
        let messages = shop.studio.messages("shop", None).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].from, lead("nora"));
        assert_eq!(
            messages[2].text,
            "Here is the plan for \"Add a cart drawer\": 2 tasks."
        );
        assert_eq!(
            messages[2].card,
            Some(TalkCard::Plan {
                goal: "SHOP-1".into(),
                title: "Add a cart drawer".into(),
                version: Some("0.5".into()),
                tasks: vec![
                    PlanTask {
                        title: "Draw the drawer".into(),
                        hire: Some("nora".into())
                    },
                    PlanTask {
                        title: "Wire it to the cart".into(),
                        hire: Some("nora".into())
                    },
                ],
            })
        );
        // Once only.
        shop.studio.plan_filed(&plan);
        assert_eq!(shop.cards().len(), 1);
    }

    #[tokio::test]
    async fn a_goal_the_owner_started_gets_no_card() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let plan = start_goal(&shop, "Fix the menu").await;
        filed(&shop, &plan, "Fix it", "nora");
        shop.studio.plan_filed(&plan);
        assert!(shop.studio.messages("shop", None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_plan_that_filed_nothing_says_so() {
        let mut events = vec![started()];
        events.extend(uses(
            "t1",
            "start_goal",
            json!({ "text": "Think about it" }),
            r#"{"goal":"SHOP-1"}"#,
            false,
        ));
        events.push(finished(Some("Started."), 0.01));
        let shop = shop(FakeRuntime::new(events));
        let plan = start_goal(&shop, "Think about it").await;
        shop.say("Go").await;
        until(|| shop.lines().len() == 2).await;
        shop.studio.plan_filed(&plan);
        let card = shop.cards().remove(0);
        assert!(matches!(card, TalkCard::Plan { tasks, .. } if tasks.is_empty()));
        assert_eq!(
            shop.lines()[2],
            "nora: The plan for \"Think about it\" filed no tasks."
        );
    }

    #[tokio::test]
    async fn a_failed_start_goal_is_not_waited_for() {
        let mut events = vec![started()];
        events.extend(uses(
            "t1",
            "start_goal",
            json!({ "text": "Nope" }),
            "no hire has the producer role",
            true,
        ));
        events.push(finished(Some("I couldn't."), 0.01));
        let shop = shop(FakeRuntime::new(events));
        let plan = start_goal(&shop, "Nope").await;
        shop.say("Go").await;
        until(|| shop.lines().len() == 2).await;
        shop.studio.plan_filed(&plan);
        assert!(shop.cards().is_empty());
    }

    #[tokio::test]
    async fn after_a_restart_the_owner_messages_since_the_leads_last_words_are_unanswered() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let studio = &shop.studio;
        studio.post("shop", TalkFrom::Owner, "One", None).unwrap();
        studio.post("shop", lead("nora"), "Reply", None).unwrap();
        studio.post("shop", TalkFrom::Owner, "Two", None).unwrap();
        studio.post("shop", TalkFrom::Studio, "Note", None).unwrap();
        let texts = |answered| -> Vec<String> {
            studio
                .unanswered("shop", answered)
                .into_iter()
                .map(|m| m.text)
                .collect()
        };
        assert_eq!(texts(None), ["Two"]);
        assert_eq!(texts(Some(0)), ["One", "Two"]);
        assert_eq!(texts(Some(3)), Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_state_the_lead_reads_names_goals_tasks_and_the_spend() {
        let shop = shop(FakeRuntime::new(answers("Hi.")));
        let state = |shop: &Shop| shop.studio.project_state("shop").unwrap();
        assert_eq!(
            state(&shop),
            "No goal is open.\nSpent today: $0.00 of $5.00."
        );

        let plan = start_goal(&shop, "Add a cart drawer").await;
        assert!(
            state(&shop).contains("Goal \"Add a cart drawer\" (SHOP-1, for 0.5): being planned.")
        );
        filed(&shop, &plan, "Draw the drawer", "nora");
        let alone = shop.task("Fix the menu").await;
        let state = state(&shop);
        assert!(state.contains("- \"Draw the drawer\" (SHOP-2): waiting to start"));
        assert!(state.contains("Tasks that belong to no goal:"));
        assert!(state.contains(&format!("- \"Fix the menu\" ({alone}): waiting to start")));

        // What the lead's own turns cost counts against the day.
        shop.say("Hello").await;
        until(|| shop.lines().len() == 2).await;
        assert!(
            shop.studio
                .project_state("shop")
                .unwrap()
                .ends_with("Spent today: $0.07 of $5.00.")
        );
    }
}
