//! Persistent company and project rooms. Owner messages can mention hires by
//! id; a mentioned hire answers with read-only project context and no actions.

use super::{Studio, StudioError, file_stem, lock, other};
use crate::api::{Response, RoomSummary, RoomView, TalkFrom, TalkMessage};
use crate::leadrun::{self, LeadRun, Tools};
use crate::now_ms;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read as _, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_config::COMPANY_ROOM;
use studio_config::config::ProjectEntry;
use studio_core::ids::HireId;
use studio_core::rule::Hire;
use studio_store::SpendKind;

const ROOM_USD: f64 = 0.25;
const ROOM_TURNS: u32 = 6;
const HISTORY: usize = 30;
const ROOM_PAGE: usize = 200;
const ROOM_PAGE_MAX: usize = 500;
const READ_CHUNK: u64 = 16 * 1024;

#[derive(Default)]
pub(super) struct State {
    files: Mutex<()>,
    turns: Mutex<HashMap<(String, String), RoomTurn>>,
    summaries: Mutex<HashMap<String, CachedSummary>>,
}

#[derive(Default)]
struct RoomTurn {
    running: bool,
    answered: Option<u64>,
}

#[derive(Clone, Default)]
struct CachedSummary {
    count: u64,
    last: Option<TalkMessage>,
    stamp: Option<Stamp>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<std::time::SystemTime>,
    bytes: u64,
}

impl Studio {
    /// The id of `project`'s room: its name, except for a project named like
    /// the company room (one made before that name was refused), which gets a
    /// name no project has.
    pub(super) fn project_room(&self, project: &str) -> String {
        if !project.eq_ignore_ascii_case(COMPANY_ROOM) {
            return project.to_string();
        }
        let config = self.config();
        let mut room = format!("{COMPANY_ROOM}-project");
        while config
            .projects
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(&room))
        {
            room.push_str("-project");
        }
        room
    }

    fn room_project(&self, room: &str) -> Result<Option<ProjectEntry>, StudioError> {
        if room == COMPANY_ROOM {
            return Ok(None);
        }
        self.config()
            .projects
            .iter()
            .find(|project| self.project_room(&project.name) == room)
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

    pub(super) fn room(
        &self,
        room: &str,
        after: Option<u64>,
        before: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Response, StudioError> {
        self.room_project(room)?;
        let limit = limit.unwrap_or(ROOM_PAGE).clamp(1, ROOM_PAGE_MAX);
        let (messages, has_more) = if after.is_some() {
            (self.room_messages(room, after)?, false)
        } else {
            self.room_page(room, before, limit)?
        };
        let total = self.room_summary(room).count;
        Ok(Response::ok(RoomView {
            messages,
            thinking: lock(&self.rooms_state.turns)
                .iter()
                .any(|((in_room, _), turn)| in_room == room && turn.running),
            total,
            has_more,
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
        lock(&self.rooms_state.summaries).insert(
            room.to_string(),
            CachedSummary {
                count: message.id,
                last: Some(message.clone()),
                stamp: stamp(&path),
            },
        );
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

    fn room_page(
        &self,
        room: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<TalkMessage>, bool), StudioError> {
        let _files = lock(&self.rooms_state.files);
        read_page(&room_file(&self.paths.data_dir, room), before, limit)
    }

    fn room_summary(&self, room: &str) -> CachedSummary {
        let path = room_file(&self.paths.data_dir, room);
        let stamp = stamp(&path);
        if let Some(cached) = lock(&self.rooms_state.summaries).get(room)
            && cached.stamp == stamp
        {
            return cached.clone();
        }
        let cached = if stamp.is_some() {
            let _files = lock(&self.rooms_state.files);
            let messages = parse(&read(&path).unwrap_or_default());
            CachedSummary {
                count: messages.last().map_or(0, |message| message.id),
                last: messages.last().cloned(),
                stamp,
            }
        } else {
            CachedSummary {
                stamp,
                ..CachedSummary::default()
            }
        };
        lock(&self.rooms_state.summaries).insert(room.to_string(), cached.clone());
        cached
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
            id: self.project_room(&project.name),
            name: format!("{} team", project.name),
            project: Some(project.name.clone()),
            ..RoomSummary::default()
        }));
        rooms
            .map(|mut summary| {
                let cached = self.room_summary(&summary.id);
                summary.count = cached.count;
                summary.last = cached.last;
                summary.thinking = lock(&self.rooms_state.turns)
                    .iter()
                    .any(|((room, _), turn)| room == &summary.id && turn.running);
                summary
            })
            .collect()
    }

    fn wake_room(self: &Arc<Self>, room: &str, hire: Hire) {
        let key = (room.to_string(), hire.id.0.clone());
        {
            let mut turns = lock(&self.rooms_state.turns);
            let turn = turns.entry(key.clone()).or_default();
            if turn.running {
                return;
            }
            turn.running = true;
        }
        let studio = Arc::clone(self);
        let room = room.to_string();
        tokio::spawn(async move {
            loop {
                let answered = lock(&studio.rooms_state.turns)
                    .get(&key)
                    .and_then(|turn| turn.answered);
                let covered = studio.room_reply(&room, &hire, answered).await;
                let mut turns = lock(&studio.rooms_state.turns);
                let turn = turns.entry(key.clone()).or_default();
                if covered.is_some() {
                    turn.answered = covered;
                }
                if covered.is_none()
                    || studio
                        .unanswered_room(&room, &hire.id.0, turn.answered)
                        .is_empty()
                {
                    turn.running = false;
                    return;
                }
            }
        });
    }

    fn unanswered_room(&self, room: &str, hire: &str, answered: Option<u64>) -> Vec<TalkMessage> {
        let messages = self.room_messages(room, None).unwrap_or_default();
        let since = answered.unwrap_or_else(|| {
            messages
                .iter()
                .rev()
                .find(|message| matches!(&message.from, TalkFrom::Lead { hire: said } if said == hire))
                .map_or(0, |message| message.id)
        });
        messages
            .into_iter()
            .filter(|message| {
                message.id > since
                    && message.from == TalkFrom::Owner
                    && mentions_hire(&message.text, hire)
            })
            .collect()
    }

    async fn room_reply(
        self: &Arc<Self>,
        room: &str,
        hire: &Hire,
        answered: Option<u64>,
    ) -> Option<u64> {
        let upto = self.room_messages(room, None).ok()?.last()?.id;
        if self.unanswered_room(room, &hire.id.0, answered).is_empty() {
            return None;
        }
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
        Some(upto)
    }

    async fn room_reply_run(self: &Arc<Self>, room: &str, hire: &Hire) -> Result<String, String> {
        let (assigned_project, mut tasks) =
            self.hire_context(hire).map_err(|error| error.to_string())?;
        let project = match self.room_project(room).map_err(|error| error.to_string())? {
            Some(project) => {
                tasks.retain(|ticket| ticket.project.0 == project.name);
                project
            }
            None => assigned_project,
        };
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
            kind: SpendKind::Room,
        };
        leadrun::run(self, run).await.map(|reply| reply.text)
    }
}

fn mention_ids(text: &str) -> BTreeSet<&str> {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '@' | '-' | '_' | '.'))
    })
    .filter_map(|word| word.strip_prefix('@'))
    .map(|id| id.trim_end_matches('.'))
    .filter(|id| !id.is_empty())
    .collect()
}

fn mentions_hire(text: &str, hire: &str) -> bool {
    mention_ids(text).contains(hire)
}

fn mentioned(studio: &Studio, text: &str) -> Vec<Hire> {
    let ids = mention_ids(text);
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

fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok(),
        bytes: metadata.len(),
    })
}

fn read_page(
    path: &Path,
    before: Option<u64>,
    limit: usize,
) -> Result<(Vec<TalkMessage>, bool), StudioError> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), false));
        }
        Err(error) => return Err(other(format!("can't read {}: {error}", path.display()))),
    };
    let mut remaining = file
        .seek(SeekFrom::End(0))
        .map_err(|error| other(format!("can't read {}: {error}", path.display())))?;
    let mut bytes = Vec::new();
    loop {
        let messages = parse(&String::from_utf8_lossy(&bytes))
            .into_iter()
            .filter(|message| before.is_none_or(|before| message.id < before))
            .count();
        if messages >= limit || remaining == 0 {
            break;
        }
        let take = remaining.min(READ_CHUNK);
        let start = remaining - take;
        file.seek(SeekFrom::Start(start))
            .and_then(|_| {
                let mut chunk = vec![0; take as usize];
                file.read_exact(&mut chunk)?;
                chunk.extend(std::mem::take(&mut bytes));
                bytes = chunk;
                Ok(())
            })
            .map_err(|error| other(format!("can't read {}: {error}", path.display())))?;
        remaining = start;
    }
    let mut messages = parse(&String::from_utf8_lossy(&bytes))
        .into_iter()
        .filter(|message| before.is_none_or(|before| message.id < before))
        .collect::<Vec<_>>();
    let start = messages.len().saturating_sub(limit);
    let messages = messages.split_off(start);
    let has_more = messages.first().is_some_and(|message| message.id > 1);
    Ok((messages, has_more))
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
        rooms_with(FakeRuntime::new(vec![RunEvent::Finished {
            outcome: Outcome::Success,
            session: Some("s1".into()),
            cost: Cost {
                usd: 0.03,
                reported: true,
                ..Cost::default()
            },
            summary: Some(reply.into()),
        }]))
    }

    fn rooms_with(runtime: FakeRuntime) -> Rooms {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("shop");
        let api = home.path().join("api");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n\n\
                 [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n\n\
                 [[projects]]\nname = \"shop\"\npath = \"{}\"\n\n\
                 [[projects]]\nname = \"api\"\npath = \"{}\"\n",
                repo.display(),
                api.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("hires")).unwrap();
        std::fs::write(
            home.path().join("hires/dev.md"),
            "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: fake-1\n---\nYou build the shop.\n",
        )
        .unwrap();
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

    async fn messages(studio: &Arc<Studio>, room: &str) -> RoomView {
        let response = studio
            .handle(Request::Room {
                room: room.into(),
                after: None,
                before: None,
                limit: None,
            })
            .await;
        assert!(response.ok, "{:?}", response.error);
        serde_json::from_value(response.data).unwrap()
    }

    #[tokio::test]
    async fn a_project_named_company_gets_a_room_of_its_own() {
        let home = tempfile::tempdir().unwrap();
        let mut config = String::from("[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n");
        for name in ["company", "company-project"] {
            let dir = home.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            config.push_str(&format!(
                "\n[[projects]]\nname = \"{name}\"\npath = \"{}\"\n",
                dir.display()
            ));
        }
        std::fs::write(home.path().join("studio.toml"), config).unwrap();
        let studio = Studio::open(
            StudioPaths::resolve(Some(home.path().to_path_buf())),
            HashMap::new(),
        )
        .unwrap();
        let ids: Vec<String> = studio.room_summaries().into_iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            ["company", "company-project-project", "company-project"]
        );
        studio
            .post_room(&studio.project_room("company"), TalkFrom::Owner, "Hello")
            .unwrap();
        assert_eq!(messages(&studio, "company-project-project").await.total, 1);
        assert_eq!(messages(&studio, "company").await.total, 0);
        let sent = studio
            .handle(Request::TalkToRoom {
                room: "company-project-project".into(),
                text: "Hi".into(),
            })
            .await;
        assert!(sent.ok, "{:?}", sent.error);
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
        assert_eq!(budget.data["by_kind"][0]["kind"], "room");
    }

    #[tokio::test]
    async fn a_mention_during_a_reply_gets_the_next_turn() {
        let answer = |text: &str| {
            vec![RunEvent::Finished {
                outcome: Outcome::Success,
                session: Some("s1".into()),
                cost: Cost::default(),
                summary: Some(text.into()),
            }]
        };
        let (runtime, release) =
            FakeRuntime::sequence(vec![answer("First answer"), answer("Second answer")]).held();
        let rooms = rooms_with(runtime);
        for text in ["@dev first question", "@dev second question"] {
            let sent = rooms
                .studio
                .handle(Request::TalkToRoom {
                    room: "company".into(),
                    text: text.into(),
                })
                .await;
            assert!(sent.ok);
            if text.contains("first") {
                for _ in 0..100 {
                    if !rooms.runtime.specs().is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
        assert_eq!(rooms.runtime.specs().len(), 1);
        release.notify_one();
        for _ in 0..100 {
            if rooms.runtime.specs().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(rooms.runtime.specs().len(), 2);
        release.notify_one();
        for _ in 0..100 {
            let view = messages(&rooms.studio, "company").await;
            if view.messages.len() == 4 && !view.thinking {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let view = messages(&rooms.studio, "company").await;
        assert_eq!(view.messages.len(), 4);
        assert_eq!(view.messages[2].text, "First answer");
        assert_eq!(view.messages[3].text, "Second answer");
    }

    #[tokio::test]
    async fn a_project_room_does_not_include_other_project_assignments() {
        let rooms = rooms("Scoped answer");
        for (project, title, detail) in [
            ("shop", "Shop drawer", "Public shop brief"),
            ("api", "Private API migration", "Secret API brief"),
        ] {
            let made = rooms
                .studio
                .handle(Request::CreateTicket {
                    project: project.into(),
                    title: title.into(),
                    goal: String::new(),
                    hire: "dev".into(),
                    base: None,
                    detail: detail.into(),
                })
                .await;
            assert!(made.ok, "{:?}", made.error);
        }
        let sent = rooms
            .studio
            .handle(Request::TalkToRoom {
                room: "shop".into(),
                text: "@dev What are you doing?".into(),
            })
            .await;
        assert!(sent.ok);
        for _ in 0..100 {
            if !rooms.runtime.specs().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let prompt = &rooms.runtime.specs()[0].prompt;
        assert!(prompt.contains("Public shop brief"), "{prompt}");
        assert!(!prompt.contains("Private API migration"), "{prompt}");
        assert!(!prompt.contains("Secret API brief"), "{prompt}");
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
    fn room_history_is_bounded_and_can_page_backwards() {
        let rooms = rooms("Hi");
        for n in 1..=5 {
            rooms
                .studio
                .post_room("company", TalkFrom::Owner, &format!("Message {n}"))
                .unwrap();
        }
        let page = |before, limit| {
            let response = rooms
                .studio
                .room("company", None, before, Some(limit))
                .unwrap();
            serde_json::from_value::<RoomView>(response.data).unwrap()
        };
        let latest = page(None, 2);
        assert_eq!(latest.total, 5);
        assert!(latest.has_more);
        assert_eq!(
            latest.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            [4, 5]
        );
        let earlier = page(Some(4), 2);
        assert!(earlier.has_more);
        assert_eq!(
            earlier.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            [2, 3]
        );
        let first = page(Some(2), 2);
        assert!(!first.has_more);
        assert_eq!(first.messages.iter().map(|m| m.id).collect::<Vec<_>>(), [1]);
        let summary = rooms.studio.room_summaries().remove(0);
        assert_eq!(summary.count, 5);
        assert_eq!(
            summary.last.map(|message| message.text),
            Some("Message 5".to_string())
        );
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
