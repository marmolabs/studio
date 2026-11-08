//! Direct, persistent conversations between the owner and any hire.
//!
//! Each hire has one append-only conversation in `<data>/people/<hire>.jsonl`.
//! A reply is a separate read-only run with the hire's current assignments and
//! recent results as context. These runs cannot change files or act on tasks;
//! the web UI uses the existing task-message route when the owner wants to
//! steer an active run.

use super::{Studio, StudioError, file_stem, lock, other};
use crate::api::{ConversationView, Response, TalkFrom, TalkMessage};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_config::config::ProjectEntry;
use studio_core::ids::HireId;
use studio_core::rule::Hire;
use studio_core::ticket::Ticket;
use studio_store::SpendKind;

const TALK_USD: f64 = 0.25;
const TALK_TURNS: u32 = 8;
const HISTORY: usize = 20;
const TASKS: usize = 12;

#[derive(Default)]
pub(super) struct State {
    files: Mutex<()>,
    turns: Mutex<HashMap<String, Turns>>,
}

#[derive(Default)]
struct Turns {
    running: bool,
    answered: Option<u64>,
}

impl Studio {
    pub(super) fn talk_to_hire(
        self: &Arc<Self>,
        hire: &str,
        text: &str,
    ) -> Result<Response, StudioError> {
        self.hire(&HireId::new(hire))?;
        let text = text.trim();
        if text.is_empty() {
            return Err(other("there is nothing to send: write a message"));
        }
        let message = self.post_to_hire(hire, TalkFrom::Owner, text)?;
        self.wake_hire(hire);
        Ok(Response::ok(message))
    }

    pub(super) fn hire_conversation(
        &self,
        hire: &str,
        after: Option<u64>,
    ) -> Result<Response, StudioError> {
        self.hire(&HireId::new(hire))?;
        Ok(Response::ok(ConversationView {
            messages: self.hire_messages(hire, after)?,
            thinking: lock(&self.people_state.turns)
                .get(hire)
                .is_some_and(|turns| turns.running),
        }))
    }

    fn wake_hire(self: &Arc<Self>, hire: &str) {
        {
            let mut turns = lock(&self.people_state.turns);
            let turns = turns.entry(hire.to_string()).or_default();
            if turns.running {
                return;
            }
            turns.running = true;
        }
        let studio = Arc::clone(self);
        let hire = hire.to_string();
        tokio::spawn(async move {
            loop {
                let answered = studio.hire_turn(&hire).await;
                let mut turns = lock(&studio.people_state.turns);
                let turns = turns.entry(hire.clone()).or_default();
                if answered.is_some() {
                    turns.answered = answered;
                }
                if answered.is_none() || studio.unanswered_hire(&hire, turns.answered).is_empty() {
                    turns.running = false;
                    return;
                }
            }
        });
    }

    fn unanswered_hire(&self, hire: &str, answered: Option<u64>) -> Vec<TalkMessage> {
        let messages = self.hire_messages(hire, None).unwrap_or_default();
        let since = answered.unwrap_or_else(|| {
            messages
                .iter()
                .rev()
                .find(|message| matches!(message.from, TalkFrom::Lead { .. }))
                .map_or(0, |message| message.id)
        });
        messages
            .into_iter()
            .filter(|message| message.id > since && message.from == TalkFrom::Owner)
            .collect()
    }

    async fn hire_turn(self: &Arc<Self>, hire_id: &str) -> Option<u64> {
        let answered = lock(&self.people_state.turns)
            .get(hire_id)
            .and_then(|turns| turns.answered);
        let messages = match self.hire_messages(hire_id, None) {
            Ok(messages) => messages,
            Err(error) => {
                eprintln!("people: {hire_id}: {error}");
                return None;
            }
        };
        let upto = messages.last()?.id;
        let new = self.unanswered_hire(hire_id, answered);
        if new.is_empty() {
            return None;
        }
        let history: Vec<TalkMessage> = messages
            .into_iter()
            .filter(|message| !new.iter().any(|new| new.id == message.id))
            .collect();
        let hire = match self.hire(&HireId::new(hire_id)) {
            Ok(hire) => hire,
            Err(error) => {
                self.say_to_hire(hire_id, &error.to_string());
                return Some(upto);
            }
        };
        let (project, prompt) = match self.hire_prompt(&hire, &history, &new) {
            Ok(context) => context,
            Err(error) => {
                self.say_to_hire(hire_id, &error.to_string());
                return Some(upto);
            }
        };
        let charge = format!("{}/people/{hire_id}", project.name);
        let run = LeadRun {
            project: &project.name,
            hire: &hire,
            cwd: &project.path,
            prompt,
            tools: Tools::ReadOnly,
            charge_task: Some(&charge),
            cap_usd: TALK_USD,
            max_turns: TALK_TURNS,
            kind: SpendKind::Conversation,
        };
        match leadrun::run(self, run).await {
            Ok(reply) if !reply.text.is_empty() => {
                self.say_to_hire(hire_id, &reply.text);
            }
            Ok(_) => self.say_to_hire(
                hire_id,
                &format!("{} had nothing to say.", super::briefing::named(&hire)),
            ),
            Err(why) => self.say_to_hire(hire_id, &sentence(&why.message)),
        }
        Some(upto)
    }

    fn hire_prompt(
        &self,
        hire: &Hire,
        before: &[TalkMessage],
        new: &[TalkMessage],
    ) -> Result<(ProjectEntry, String), StudioError> {
        let (project, tasks) = self.hire_context(hire)?;
        let name = super::briefing::named(hire);
        let role = if hire.role.trim().is_empty() {
            "team member"
        } else {
            hire.role.trim()
        };
        let mut prompt = format!(
            "You are {name}, a {role}, talking directly with the company owner. Answer what \
             they ask in plain words and in the first person, in at most three short sentences. \
             Ground answers in your assignments, recent results and the project files you read. \
             Distinguish facts from uncertainty. Never claim to have changed work or sent an \
             instruction: this conversation is read-only and has no action tools.\n\n\
             ## Your assignments\n{}\n\n## Reading context\nProject: {} ({}).\n",
            assignment_state(self, &tasks),
            project.name,
            project.path.display(),
        );
        let earlier = &before[before.len().saturating_sub(HISTORY)..];
        if !earlier.is_empty() {
            prompt.push_str("\n## The conversation so far\n");
            for message in earlier {
                let speaker = match &message.from {
                    TalkFrom::Owner => "Owner",
                    TalkFrom::Studio => "Studio",
                    TalkFrom::Lead { .. } => name.as_str(),
                };
                prompt.push_str(&format!(
                    "{speaker}: {}\n",
                    message.text.trim().replace('\n', " ")
                ));
            }
        }
        prompt.push_str(if new.len() == 1 {
            "\n## The owner's new message\n"
        } else {
            "\n## The owner's new messages\n"
        });
        for message in new {
            prompt.push_str(&message.text);
            prompt.push('\n');
        }
        Ok((project, prompt))
    }

    /// The project whose files this hire may read, and all of its assignments.
    /// Current work picks the project; otherwise the latest assignment, a
    /// project the hire leads, or the studio's first project does.
    pub(super) fn hire_context(
        &self,
        hire: &Hire,
    ) -> Result<(ProjectEntry, Vec<Ticket>), StudioError> {
        let tickets = lock(&self.store).tickets()?;
        let mut mine: Vec<Ticket> = tickets
            .into_iter()
            .filter(|ticket| ticket.hire == hire.id)
            .collect();
        mine.sort_by_key(|ticket| ticket.created_at);
        let current = mine
            .iter()
            .rev()
            .find(|ticket| !matches!(ticket.state.kind(), "landed" | "failed" | "cancelled"))
            .or_else(|| mine.last());
        let config = self.config();
        let project = current
            .and_then(|ticket| {
                config
                    .projects
                    .iter()
                    .find(|project| project.name == ticket.project.0)
            })
            .or_else(|| {
                config
                    .projects
                    .iter()
                    .find(|project| project.lead.as_deref() == Some(hire.id.0.as_str()))
            })
            .or_else(|| config.projects.first())
            .cloned()
            .ok_or_else(|| other("add a project before talking to your hires"))?;
        Ok((project, mine))
    }

    fn say_to_hire(&self, hire: &str, text: &str) {
        if let Err(error) = self.post_to_hire(
            hire,
            TalkFrom::Lead {
                hire: hire.to_string(),
            },
            text,
        ) {
            eprintln!("people: {hire}: {error}");
        }
    }

    fn post_to_hire(
        &self,
        hire: &str,
        from: TalkFrom,
        text: &str,
    ) -> Result<TalkMessage, StudioError> {
        let _files = lock(&self.people_state.files);
        let path = file_of(&self.paths.data_dir, hire);
        let kept = read(&path)?;
        let message = TalkMessage {
            id: last_id(&kept) + 1,
            at: now_ms(),
            from,
            text: text.to_string(),
            card: None,
        };
        let mut line = serde_json::to_string(&message)
            .map_err(|error| other(format!("can't keep the message: {error}")))?;
        line.push('\n');
        if !kept.is_empty() && !kept.ends_with('\n') {
            line.insert(0, '\n');
        }
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
            })
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .map_err(|error| {
                other(format!(
                    "can't save the message to {}: {error}",
                    path.display()
                ))
            })?;
        Ok(message)
    }

    fn hire_messages(
        &self,
        hire: &str,
        after: Option<u64>,
    ) -> Result<Vec<TalkMessage>, StudioError> {
        let _files = lock(&self.people_state.files);
        let kept = read(&file_of(&self.paths.data_dir, hire))?;
        Ok(parse(&kept)
            .into_iter()
            .filter(|message| after.is_none_or(|after| message.id > after))
            .collect())
    }
}

pub(super) fn assignment_state(studio: &Studio, tasks: &[Ticket]) -> String {
    if tasks.is_empty() {
        return "No tasks have been assigned to you yet.".to_string();
    }
    let results = studio.results(tasks);
    tasks
        .iter()
        .rev()
        .take(TASKS)
        .map(|task| {
            let mut line = format!(
                "- \"{}\" ({}) in {}: {}.",
                task.title,
                task.id,
                task.project,
                task.state.kind()
            );
            if !task.goal.trim().is_empty() {
                line.push_str(&format!(" Goal: {}.", task.goal.trim()));
            }
            if !task.detail.trim().is_empty() {
                let detail: String = task.detail.trim().chars().take(500).collect();
                line.push_str(&format!(" Brief: {detail}"));
            }
            if let Some(result) = results.get(&task.id.to_string()) {
                line.push_str(&format!(" Latest result: {result}"));
            }
            if !task.feedback.as_deref().unwrap_or_default().is_empty() {
                line.push_str(&format!(
                    " Latest feedback: {}",
                    task.feedback.as_deref().unwrap_or_default()
                ));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_of(data_dir: &Path, hire: &str) -> PathBuf {
    data_dir
        .join("people")
        .join(format!("{}.jsonl", file_stem(hire)))
}

fn read(path: &Path) -> Result<String, StudioError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(other(format!("can't read {}: {error}", path.display()))),
    }
}

fn parse(text: &str) -> Vec<TalkMessage> {
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn last_id(text: &str) -> u64 {
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str::<TalkMessage>(line).ok())
        .map_or(0, |message| message.id)
}

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::api::Request;
    use serde_json::json;
    use std::time::Duration;
    use studio_config::StudioPaths;
    use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};

    struct Team {
        _home: tempfile::TempDir,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
    }

    fn team(events: Vec<RunEvent>) -> Team {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("shop");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
                 [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\n",
                repo.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        std::fs::write(
            home.path().join("hires/dev.md"),
            "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nYou build the shop.\n",
        )
        .unwrap();
        let runtime = FakeRuntime::new(events);
        let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
        runtimes.insert("fake".into(), Arc::new(runtime.clone()));
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        let studio = Studio::open(paths, runtimes).unwrap();
        Team {
            _home: home,
            studio,
            runtime,
        }
    }

    fn finished(text: &str) -> Vec<RunEvent> {
        vec![
            RunEvent::Started {
                session: "s1".into(),
                model: Some("fake-1".into()),
            },
            RunEvent::Finished {
                outcome: Outcome::Success,
                session: Some("s1".into()),
                cost: Cost {
                    usd: 0.04,
                    reported: true,
                    input_tokens: 100,
                    output_tokens: 20,
                    cumulative: true,
                },
                summary: Some(text.into()),
            },
        ]
    }

    async fn conversation(studio: &Arc<Studio>) -> ConversationView {
        let reply = studio
            .handle(Request::HireConversation {
                hire: "dev".into(),
                after: None,
            })
            .await;
        assert!(reply.ok, "{:?}", reply.error);
        serde_json::from_value(reply.data).unwrap()
    }

    #[tokio::test]
    async fn an_owner_can_keep_a_read_only_conversation_with_an_employee() {
        let team = team(finished("I'm working on the cart drawer."));
        let made = team
            .studio
            .handle(Request::CreateTicket {
                project: "shop".into(),
                title: "Cart drawer".into(),
                goal: "Checkout".into(),
                hire: "dev".into(),
                base: None,
                detail: "Keep the current cart visible.".into(),
            })
            .await;
        assert!(made.ok, "{:?}", made.error);
        let sent = team
            .studio
            .handle(Request::TalkToHire {
                hire: "dev".into(),
                text: "What are you working on?".into(),
            })
            .await;
        assert!(sent.ok, "{:?}", sent.error);
        let owner: TalkMessage = serde_json::from_value(sent.data).unwrap();
        assert_eq!(owner.id, 1);
        assert_eq!(owner.from, TalkFrom::Owner);

        let mut view = conversation(&team.studio).await;
        for _ in 0..100 {
            if view.messages.len() == 2 && !view.thinking {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            view = conversation(&team.studio).await;
        }
        assert_eq!(view.messages.len(), 2, "{view:?}");
        assert_eq!(view.messages[1].text, "I'm working on the cart drawer.");
        assert_eq!(view.messages[1].from, TalkFrom::Lead { hire: "dev".into() });
        let spec = &team.runtime.specs()[0];
        assert_eq!(spec.mcp_config, None);
        assert!(spec.prompt.contains("Cart drawer"));
        assert!(spec.prompt.contains("Goal: Checkout."));
        assert!(
            spec.prompt
                .contains("Brief: Keep the current cart visible.")
        );
        assert!(spec.prompt.contains("read-only"));
        let budget = team.studio.handle(Request::Budget).await;
        assert_eq!(
            budget.data["spent_24h"]["by_ticket"],
            json!({"shop/people/dev": 0.04})
        );
        assert_eq!(budget.data["spent_24h"]["by_hire"], json!({"dev": 0.04}));
        assert_eq!(budget.data["by_kind"][0]["kind"], "conversation");
    }

    #[tokio::test]
    async fn direct_conversations_validate_the_hire_and_message() {
        let team = team(vec![]);
        let missing = team
            .studio
            .handle(Request::TalkToHire {
                hire: "nobody".into(),
                text: "Hello".into(),
            })
            .await;
        assert!(!missing.ok);
        assert!(missing.error.unwrap().contains("no hire named nobody"));
        let empty = team
            .studio
            .handle(Request::TalkToHire {
                hire: "dev".into(),
                text: "  ".into(),
            })
            .await;
        assert!(!empty.ok);
        assert!(team.runtime.specs().is_empty());
    }

    #[test]
    fn conversation_files_stay_inside_the_people_folder() {
        let team = team(vec![]);
        team.studio
            .post_to_hire("../outside", TalkFrom::Owner, "Hello")
            .unwrap();
        assert!(
            team._home
                .path()
                .join("data/people/___outside.jsonl")
                .exists()
        );
        assert!(!team._home.path().join("outside.jsonl").exists());
    }
}
