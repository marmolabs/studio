//! Persistent company and project rooms. Owner messages can mention hires by
//! id; a mentioned hire answers with read-only project context and no actions.

use super::{Studio, StudioError, file_stem, lock, other};
use crate::api::{ConversationView, Response, RoomSummary, TalkFrom, TalkMessage};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_config::config::ProjectEntry;
use studio_core::ids::HireId;
use studio_core::rule::Hire;

const ROOM_USD: f64 = 0.25;
const ROOM_TURNS: u32 = 6;
const HISTORY: usize = 30;

#[derive(Default)]
pub(super) struct State {
    files: Mutex<()>,
    replying: Mutex<BTreeSet<(String, String)>>,
}

impl Studio {
    fn room_project(&self, room: &str) -> Result<Option<ProjectEntry>, StudioError> {
        if room == "company" {
            return Ok(None);
        }
        self.config()
            .projects
            .iter()
            .find(|project| project.name == room)
            .cloned()
            .map(Some)
            .ok_or_else(|| other(format!("no room named {room}")))
    }

    pub(super) fn talk_to_room(
        self: &Arc<Self>,
        room: &str,
        text: &str,
    ) -> Result<Response, StudioError> {
        self.room_project(room)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(other("there is nothing to send: write a message"));
        }
        let message = self.post_room(room, TalkFrom::Owner, text)?;
        for hire in mentioned(self, text) {
            self.wake_room(room, hire);
        }
        Ok(Response::ok(message))
    }

    pub(super) fn room(&self, room: &str, after: Option<u64>) -> Result<Response, StudioError> {
        self.room_project(room)?;
        Ok(Response::ok(ConversationView {
            messages: self.room_messages(room, after)?,
            thinking: lock(&self.rooms_state.replying)
                .iter()
                .any(|(in_room, _)| in_room == room),
        }))
    }

    pub(super) fn post_room(
        &self,
        room: &str,
        from: TalkFrom,
        text: &str,
    ) -> Result<TalkMessage, StudioError> {
        let _files = lock(&self.rooms_state.files);
        let path = room_file(&self.paths.data_dir, room);
        let kept = read(&path)?;
        let message = TalkMessage {
            id: last_id(&kept) + 1,
            at: now_ms(),
            from,
            text: text.to_string(),
            card: None,
        };
        let mut line = serde_json::to_string(&message)
            .map_err(|error| other(format!("can't keep the room message: {error}")))?;
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
            .map_err(|error| other(format!("can't save the room message: {error}")))?;
        Ok(message)
    }

    fn room_messages(
        &self,
        room: &str,
        after: Option<u64>,
    ) -> Result<Vec<TalkMessage>, StudioError> {
        let _files = lock(&self.rooms_state.files);
        Ok(parse(&read(&room_file(&self.paths.data_dir, room))?)
            .into_iter()
            .filter(|message| after.is_none_or(|after| message.id > after))
            .collect())
    }

    pub(super) fn room_summaries(&self) -> Vec<RoomSummary> {
        let config = self.config();
        let rooms = std::iter::once(RoomSummary {
            id: "company".to_string(),
            name: "Company room".to_string(),
            project: None,
            ..RoomSummary::default()
        })
        .chain(config.projects.iter().map(|project| RoomSummary {
            id: project.name.clone(),
            name: format!("{} team", project.name),
            project: Some(project.name.clone()),
            ..RoomSummary::default()
        }));
        rooms
            .map(|mut summary| {
                let messages = self.room_messages(&summary.id, None).unwrap_or_default();
                summary.count = messages.len() as u64;
                summary.last = messages.last().cloned();
                summary.thinking = lock(&self.rooms_state.replying)
                    .iter()
                    .any(|(room, _)| room == &summary.id);
                summary
            })
            .collect()
    }

    fn wake_room(self: &Arc<Self>, room: &str, hire: Hire) {
        let key = (room.to_string(), hire.id.0.clone());
        if !lock(&self.rooms_state.replying).insert(key.clone()) {
            return;
        }
        let studio = Arc::clone(self);
        let room = room.to_string();
        tokio::spawn(async move {
            studio.room_reply(&room, &hire).await;
            lock(&studio.rooms_state.replying).remove(&key);
        });
    }

    async fn room_reply(self: &Arc<Self>, room: &str, hire: &Hire) {
        let result = self.room_reply_run(room, hire).await;
        match result {
            Ok(reply) if !reply.trim().is_empty() => {
                if let Err(error) = self.post_room(
                    room,
                    TalkFrom::Lead {
                        hire: hire.id.0.clone(),
                    },
                    reply.trim(),
                ) {
                    eprintln!("rooms: {room}: {error}");
                }
            }
            Ok(_) => {}
            Err(error) => {
                let name = super::briefing::named(hire);
                if let Err(error) = self.post_room(
                    room,
                    TalkFrom::Studio,
                    &format!("{name} could not respond: {error}"),
                ) {
                    eprintln!("rooms: {room}: {error}");
                }
            }
        }
    }

    async fn room_reply_run(self: &Arc<Self>, room: &str, hire: &Hire) -> Result<String, String> {
        let (assigned_project, tasks) =
            self.hire_context(hire).map_err(|error| error.to_string())?;
        let project = self
            .room_project(room)
            .map_err(|error| error.to_string())?
            .unwrap_or(assigned_project);
        let messages = self
            .room_messages(room, None)
            .map_err(|error| error.to_string())?;
        let history = messages[messages.len().saturating_sub(HISTORY)..]
            .iter()
            .map(|message| {
                let who = match &message.from {
                    TalkFrom::Owner => "Owner".to_string(),
                    TalkFrom::Studio => "Studio".to_string(),
                    TalkFrom::Lead { hire } => self
                        .hire(&HireId::new(hire))
                        .map(|hire| super::briefing::named(&hire))
                        .unwrap_or_else(|_| hire.clone()),
                };
                format!("{who}: {}", message.text.trim().replace('\n', " "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let name = super::briefing::named(hire);
        let prompt = format!(
            "You are {name}, a {role}, in the {room} company room. The owner mentioned you. \
             Answer the room directly in at most three short sentences, using your actual assignments \
             and files you read. Be explicit about blockers and uncertainty. This is read-only: never \
             claim you changed work or sent an instruction.\n\n## Your assignments\n{}\n\n\
             ## Project files\n{} ({})\n\n## Room conversation\n{history}\n",
            super::people::assignment_state(self, &tasks),
            project.name,
            project.path.display(),
            role = if hire.role.trim().is_empty() {
                "team member"
            } else {
                hire.role.trim()
            },
        );
        let charge = format!("{}/rooms/{room}", project.name);
        let run = LeadRun {
            project: &project.name,
            hire,
            cwd: &project.path,
            prompt,
            tools: Tools::ReadOnly,
            charge_task: Some(&charge),
            cap_usd: ROOM_USD,
            max_turns: ROOM_TURNS,
        };
        leadrun::run(self, run).await.map(|reply| reply.text)
    }
}

fn mentioned(studio: &Studio, text: &str) -> Vec<Hire> {
    let ids = text
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '@' | '-' | '_' | '.'))
        })
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|id| !id.is_empty())
        .collect::<BTreeSet<_>>();
    let mut hires = studio
        .all_hires()
        .into_iter()
        .filter(|hire| ids.contains(hire.id.0.as_str()))
        .collect::<Vec<_>>();
    hires.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    hires
}

fn room_file(data_dir: &Path, room: &str) -> PathBuf {
    data_dir
        .join("rooms")
        .join(format!("{}.jsonl", file_stem(room)))
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::api::Request;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::Duration;
    use studio_config::StudioPaths;
    use studio_runtime::{Cost, FakeRuntime, Outcome, RunEvent, Runtime};

    struct Rooms {
        _home: tempfile::TempDir,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
    }

    fn rooms(reply: &str) -> Rooms {
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
        let runtime = FakeRuntime::new(vec![RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost {
                usd: 0.03,
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
        Rooms {
            _home: home,
            studio,
            runtime,
        }
    }

    async fn messages(studio: &Arc<Studio>, room: &str) -> ConversationView {
        let response = studio
            .handle(Request::Room {
                room: room.into(),
                after: None,
            })
            .await;
        assert!(response.ok, "{:?}", response.error);
        serde_json::from_value(response.data).unwrap()
    }

    #[tokio::test]
    async fn a_mentioned_employee_answers_in_the_persistent_room() {
        let rooms = rooms("I can take the cart drawer.");
        let sent = rooms
            .studio
            .handle(Request::TalkToRoom {
                room: "company".into(),
                text: "@dev Can you take the cart drawer?".into(),
            })
            .await;
        assert!(sent.ok, "{:?}", sent.error);
        for _ in 0..100 {
            let view = messages(&rooms.studio, "company").await;
            if view.messages.len() == 2 && !view.thinking {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let view = messages(&rooms.studio, "company").await;
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[1].text, "I can take the cart drawer.");
        assert_eq!(view.messages[1].from, TalkFrom::Lead { hire: "dev".into() });
        let spec = &rooms.runtime.specs()[0];
        assert!(spec.prompt.contains("The owner mentioned you"));
        assert!(spec.prompt.contains("@dev Can you take the cart drawer?"));
        assert_eq!(spec.mcp_config, None);
        let summaries = rooms.studio.room_summaries();
        assert_eq!(summaries[0].count, 2);
        assert_eq!(summaries[1].id, "shop");
        let budget = rooms.studio.handle(Request::Budget).await;
        assert_eq!(
            budget.data["spent_24h"]["by_ticket"],
            json!({"shop/rooms/company": 0.03})
        );
    }

    #[tokio::test]
    async fn announcements_do_not_spend_and_rooms_are_validated() {
        let rooms = rooms("Never called");
        let sent = rooms
            .studio
            .handle(Request::TalkToRoom {
                room: "shop".into(),
                text: "Release is tomorrow. @nobody is not on the team.".into(),
            })
            .await;
        assert!(sent.ok);
        assert_eq!(messages(&rooms.studio, "shop").await.messages.len(), 1);
        assert!(rooms.runtime.specs().is_empty());
        let missing = rooms
            .studio
            .handle(Request::TalkToRoom {
                room: "gone".into(),
                text: "Hi".into(),
            })
            .await;
        assert!(!missing.ok);
    }

    #[test]
    fn room_names_stay_inside_the_rooms_folder() {
        let rooms = rooms("Hi");
        rooms
            .studio
            .post_room("../outside", TalkFrom::Owner, "Hello")
            .unwrap();
        assert!(
            rooms
                ._home
                .path()
                .join("data/rooms/___outside.jsonl")
                .exists()
        );
        assert!(!rooms._home.path().join("outside.jsonl").exists());
    }
}
