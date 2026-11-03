//! The rail beside a conversation, "In shop now": who works on the project and
//! on what, the milestone with its tasks, and what the project spent today.

use serde_json::Value;
use std::collections::HashSet;
use studio_core::event::Event as TicketEvent;
use studio_core::ticket::{Ticket, TicketKind};

use super::thread::Person;
use crate::pages::office::model::Role;
use crate::pages::office::{Office, facts};

/// What the rail shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Rail {
    /// Everyone at work on the project, each once, with their tasks.
    pub workers: Vec<Worker>,
    /// People with nothing to do, who can take work.
    pub free: Vec<Person>,
    pub milestone: Option<Milestone>,
    pub money: Money,
}

/// Someone at work on tasks of the project.
#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub person: Person,
    /// Oldest claim first.
    pub tasks: Vec<Job>,
}

/// One task someone is at work on.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub task: String,
    pub title: String,
    /// `run` while it works, `attn` while it waits for the owner.
    pub tone: &'static str,
    /// What is happening to it: "working", "in checks", "in review",
    /// "waiting for you".
    pub status: &'static str,
    /// Since when, in epoch milliseconds, if the office can tell.
    pub since: Option<u64>,
    /// The agent's run is going: a message reaches it as it works.
    pub running: bool,
}

/// The goal the project works towards, with its tasks.
#[derive(Clone, Debug, PartialEq)]
pub struct Milestone {
    pub title: String,
    pub version: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Each task's title and its state as `ui::look` knows it.
    pub tasks: Vec<(String, &'static str)>,
}

/// What the project spent in the last day, against what it may.
#[derive(Clone, Debug, PartialEq)]
pub struct Money {
    pub spent: f64,
    pub cap: f64,
}

/// The rail of `project`.
pub fn rail(office: &Office, project: &str, money: Money) -> Rail {
    Rail {
        workers: workers(office, project),
        free: free(office),
        milestone: milestone(office, project),
        money,
    }
}

/// Who is at work on `task` and what is happening to it, when somebody is:
/// the agent that works on it, or the one that reviews it.
fn at_work(office: &Office, task: &Ticket) -> Option<(String, Job)> {
    let state = task.state.kind();
    let (tone, status) = match state {
        "claimed" | "working" => ("run", "working"),
        "checking" => ("run", "in checks"),
        "reviewing" => ("run", "in review"),
        "asking" => ("attn", "waiting for you"),
        _ => return None,
    };
    let id = task.id.to_string();
    let hire = match office.judges.get(&id) {
        Some(judge) if state == "reviewing" => judge.hire.clone(),
        _ => task.hire.to_string(),
    };
    let since = office.last(&id, |e| matches!(e, TicketEvent::Claimed));
    let job = Job {
        task: id,
        title: task.title.clone(),
        tone,
        status,
        since,
        running: matches!(state, "claimed" | "working"),
    };
    Some((hire, job))
}

/// Who is at work on the project's tasks: each person once, with their tasks
/// oldest claim first, and the person with the oldest claim first.
fn workers(office: &Office, project: &str) -> Vec<Worker> {
    let mut jobs: Vec<(String, Job)> = office
        .tickets
        .iter()
        .filter(|t| t.project.to_string() == project)
        .filter_map(|t| at_work(office, t))
        .collect();
    jobs.sort_by(|(_, a), (_, b)| a.since.cmp(&b.since).then_with(|| a.task.cmp(&b.task)));
    let mut workers: Vec<Worker> = Vec::new();
    for (hire, job) in jobs {
        match workers.iter_mut().find(|w| w.person.hire == hire) {
            Some(worker) => worker.tasks.push(job),
            None => workers.push(Worker {
                person: Person::of(office, &hire),
                tasks: vec![job],
            }),
        }
    }
    workers
}

/// The people who can take work and have none, in any project: the studio's
/// hires, but its leads and the ones who only decide, who get no tasks.
fn free(office: &Office) -> Vec<Person> {
    let busy: HashSet<String> = office
        .tickets
        .iter()
        .filter_map(|t| at_work(office, t))
        .map(|(hire, _)| hire)
        .collect();
    office
        .status
        .team
        .iter()
        .filter(|member| !busy.contains(&member.id) && !facts::leads(office, &member.id))
        .filter(|member| {
            !matches!(
                Role::of(&member.role),
                Role::Lead | Role::Approver | Role::Router | Role::Owner
            )
        })
        .map(|member| Person::of(office, &member.id))
        .collect()
}

/// The project's milestone and the tasks it filed, cancelled ones left out.
fn milestone(office: &Office, project: &str) -> Option<Milestone> {
    let goal = facts::milestone(office, project)?;
    Some(Milestone {
        title: goal.name(),
        version: goal.plan.version.clone(),
        done: goal.landed(),
        total: goal.filed.len(),
        tasks: goal
            .filed
            .iter()
            .filter(|t: &&Ticket| t.kind != TicketKind::Plan)
            .map(|t| (t.title.clone(), crate::ui::shown(&t.state)))
            .collect(),
    })
}

/// What `project` spent in the last day and what it may spend, from
/// `Request::Budget`'s answer: the project's own daily limit when it has
/// one, else the studio's. A lead's talk counts to the project too.
pub fn money(office: &Office, project: &str, budget: &Value) -> Money {
    let by_ticket = budget["spent_24h"]["by_ticket"].as_object();
    let talk = format!("{project}/talk");
    let spent = by_ticket.map_or(0.0, |spent| {
        spent
            .iter()
            .filter(|(id, _)| {
                **id == talk
                    || office
                        .ticket(id)
                        .is_some_and(|t| t.project.to_string() == project)
            })
            .filter_map(|(_, usd)| usd.as_f64())
            .sum()
    });
    let cap = budget["limits"]["projects"][project]
        .as_f64()
        .unwrap_or(office.status.daily_usd);
    Money { spent, cap }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use studiod::api::ProjectView;

    fn ticket(id: &str, project: &str, state: Value, extra: Value) -> Ticket {
        let mut fields = json!({
            "id": id, "project": project, "title": format!("Title {id}"),
            "goal": "", "hire": "dev", "state": state,
            "attempts": 0, "base_branch": "main", "created_at": 0,
        });
        if let (Some(fields), Some(extra)) = (fields.as_object_mut(), extra.as_object()) {
            fields.extend(extra.clone());
        }
        serde_json::from_value(fields).unwrap()
    }

    fn office(tickets: Vec<Ticket>) -> Office {
        let mut office = Office {
            tickets,
            now: 10_000_000,
            ..Office::default()
        };
        office.status = serde_json::from_value(json!({
            "projects": ["shop", "api"], "daily_usd": 10.0,
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
            ],
        }))
        .unwrap();
        office.projects = vec![ProjectView {
            name: "shop".into(),
            lead: Some("nora".into()),
            ..ProjectView::default()
        }];
        office
    }

    #[test]
    fn the_rail_lists_who_is_at_work_on_the_project_and_for_how_long() {
        let mut office = office(vec![
            ticket("S-1", "shop", json!("working"), json!({})),
            ticket("S-2", "shop", json!("checking"), json!({})),
            ticket("S-3", "shop", json!({"asking": {"ask": "a"}}), json!({})),
            ticket("S-4", "shop", json!("queued"), json!({})),
            ticket("S-5", "shop", json!("landed"), json!({})),
            ticket("A-1", "api", json!("working"), json!({})),
        ]);
        office.recent = vec![crate::client::Happened {
            ticket_id: "S-1".into(),
            at_ms: 9_000_000,
            event: TicketEvent::Claimed,
        }];
        let workers = workers(&office, "shop");
        // Dev has three tasks and is listed once, with all of them.
        assert_eq!(workers.len(), 1);
        let dev = &workers[0];
        assert_eq!(dev.person.name, "Dev");
        let listed: Vec<(&str, &str, &str, bool)> = dev
            .tasks
            .iter()
            .map(|job| (job.task.as_str(), job.tone, job.status, job.running))
            .collect();
        assert_eq!(
            listed,
            [
                ("S-2", "run", "in checks", false),
                ("S-3", "attn", "waiting for you", false),
                ("S-1", "run", "working", true),
            ]
        );
        let first = dev.tasks.iter().find(|job| job.task == "S-1").unwrap();
        assert_eq!(first.since, Some(9_000_000));
    }

    #[test]
    fn each_person_is_listed_once_and_the_free_come_after() {
        let mut office = office(vec![
            ticket("S-1", "shop", json!("working"), json!({})),
            ticket("S-2", "shop", json!("working"), json!({"hire": "designer"})),
            ticket("S-3", "shop", json!("working"), json!({})),
            ticket("S-4", "shop", json!("queued"), json!({"hire": "writer"})),
            ticket("A-1", "api", json!("working"), json!({"hire": "reviewer"})),
        ]);
        office.status = serde_json::from_value(json!({
            "projects": ["shop", "api"], "daily_usd": 10.0,
            "team": [
                {"id": "nora", "name": "Nora", "role": "lead", "model": "opus"},
                {"id": "dev", "name": "Dev", "role": "engineer", "model": "sonnet"},
                {"id": "designer", "name": "Designer", "role": "designer", "model": "sonnet"},
                {"id": "reviewer", "name": "Reviewer", "role": "reviewer", "model": "opus"},
                {"id": "writer", "name": "Writer", "role": "writer", "model": "sonnet"},
                {"id": "gate", "name": "Gate", "role": "approver", "model": "haiku"},
            ],
        }))
        .unwrap();
        let rail = rail(
            &office,
            "shop",
            Money {
                spent: 0.0,
                cap: 5.0,
            },
        );
        let at_work: Vec<(&str, usize)> = rail
            .workers
            .iter()
            .map(|w| (w.person.name.as_str(), w.tasks.len()))
            .collect();
        assert_eq!(at_work, [("Dev", 2), ("Designer", 1)]);
        // The writer has nothing to do. The reviewer is busy in another
        // project, the lead and the approver are given no tasks.
        let free: Vec<&str> = rail.free.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(free, ["Writer"]);
    }

    #[test]
    fn the_milestone_has_its_tasks_in_their_states() {
        let office = office(vec![
            ticket(
                "S-1",
                "shop",
                json!("landed"),
                json!({"kind": "plan", "goal": "Give the store its look", "version": "0.4"}),
            ),
            ticket(
                "S-2",
                "shop",
                json!("landed"),
                json!({"parent": "S-1", "title": "Logo"}),
            ),
            ticket(
                "S-3",
                "shop",
                json!("working"),
                json!({"parent": "S-1", "title": "Cards"}),
            ),
            ticket(
                "S-4",
                "shop",
                json!("cancelled"),
                json!({"parent": "S-1", "title": "Old"}),
            ),
        ]);
        let milestone = milestone(&office, "shop").unwrap();
        assert_eq!(milestone.title, "Give the store its look");
        assert_eq!(milestone.version.as_deref(), Some("0.4"));
        assert_eq!((milestone.done, milestone.total), (1, 2));
        assert_eq!(
            milestone.tasks,
            [
                ("Logo".to_string(), "landed"),
                ("Cards".to_string(), "working")
            ]
        );
        assert!(self::milestone(&office, "api").is_none());
    }

    #[test]
    fn the_project_spends_against_its_own_limit_or_the_studios() {
        let office = office(vec![
            ticket("S-1", "shop", json!("working"), json!({})),
            ticket("A-1", "api", json!("working"), json!({})),
        ]);
        let budget = json!({
            "limits": {"projects": {"shop": 4.0}},
            "spent_24h": {"by_ticket": {"S-1": 1.5, "A-1": 2.0, "shop/talk": 0.25}},
        });
        assert_eq!(
            money(&office, "shop", &budget),
            Money {
                spent: 1.75,
                cap: 4.0
            }
        );
        // The studio's limit stands in for a project without one.
        assert_eq!(
            money(&office, "api", &budget),
            Money {
                spent: 2.0,
                cap: 10.0
            }
        );
        assert_eq!(
            money(&office, "api", &Value::Null),
            Money {
                spent: 0.0,
                cap: 10.0
            }
        );
    }
}
