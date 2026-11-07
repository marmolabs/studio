//! A hire's read-only run, started for a moment to answer the owner or to read
//! a project and propose how it runs. It may read the project and, in the lead
//! modes, use the studio's tools. It has a cost cap and a time limit; what it
//! costs is charged to a synthetic task, which has no ticket behind it.
//!
//! Lead talks, employee talks, rooms, manager reviews, setup runs and standups
//! share this, so each only decides what to ask and what to do with the answer.

use crate::budget::Meter;
use crate::studio::{Studio, file_stem};
use crate::worker::{mcp_config, tool_paths};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use studio_core::budget::run_spend;
use studio_core::ids::TicketId;
use studio_core::rule::Hire;
use studio_runtime::{Cost, Outcome, Run, RunEvent, RunSpec};
use studio_store::Spend;

/// How long a lead's run may take before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long a stopped run gets to report its final cost.
const DRAIN: Duration = Duration::from_secs(15);

/// What a lead may never use: it reads the project and uses the studio's tools.
const DISALLOWED: [&str; 7] = [
    "Bash",
    "Edit",
    "Write",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
];

/// The tools that only read.
const READING: [&str; 4] = ["Read", "Grep", "Glob", "LS"];

/// The studio's tools, as the agent names them.
const STUDIO_TOOLS: &str = "mcp__studio__";

/// Which of the studio's tools the run gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tools {
    /// A lead talking with the owner: `studio mcp --talk <project>`.
    Talk,
    /// A lead setting a project up: `studio mcp --setup <project>`.
    Setup,
    /// A hire answering the owner directly: project reading, no studio tools.
    ReadOnly,
}

/// One read-only run of a project's hire.
pub(crate) struct LeadRun<'a> {
    /// The project whose repository is read and whose budget pays for the run.
    pub project: &'a str,
    /// The hire: its model, effort and prompt are the run's.
    pub hire: &'a Hire,
    /// Where the run works: the project's repository.
    pub cwd: &'a Path,
    /// What the hire is asked.
    pub prompt: String,
    pub tools: Tools,
    /// A separate ledger id for this run, or the project's usual talk id.
    pub charge_task: Option<&'a str>,
    /// The most the run may cost, in dollars. The run is refused when the
    /// studio, the project or the hire has less than this left for the day.
    pub cap_usd: f64,
    pub max_turns: u32,
}

/// What the lead said at the end of its run, and what the run cost.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LeadReply {
    /// The lead's last words: what it said at the end, or if it was cut short,
    /// what it had said. Empty when it ended with nothing to say.
    pub text: String,
    pub usd: f64,
}

/// The task a project's talk costs are charged to: `{project}/talk`. It is not
/// a ticket; the ledger does not link to the tickets.
pub(crate) fn talk_task(project: &str) -> TicketId {
    TicketId::new(format!("{project}/talk"))
}

/// Runs the lead and answers its last words. An error is why it gave none:
/// the run was refused for lack of budget, could not start, failed, was cut
/// short with nothing said, or took more than ten minutes and was stopped.
/// Once it has started, what it spent is recorded either way.
pub(crate) async fn run(studio: &Arc<Studio>, run: LeadRun<'_>) -> Result<LeadReply, String> {
    go(studio, run, &mut |_| {}, TIMEOUT).await
}

/// `run`, showing `observe` every event of the run as it happens, such as the
/// lead's tool calls and what they answered. `observe` must not wait.
pub(crate) async fn run_with(
    studio: &Arc<Studio>,
    run: LeadRun<'_>,
    observe: &mut (dyn FnMut(&RunEvent) + Send),
) -> Result<LeadReply, String> {
    go(studio, run, observe, TIMEOUT).await
}

/// `run_with`, stopping the run after `limit`.
async fn go(
    studio: &Arc<Studio>,
    lead: LeadRun<'_>,
    observe: &mut (dyn FnMut(&RunEvent) + Send),
    limit: Duration,
) -> Result<LeadReply, String> {
    let LeadRun {
        project,
        hire,
        cwd,
        prompt,
        tools,
        charge_task,
        cap_usd,
        max_turns,
    } = lead;
    let who = called(hire);
    let task = charge_task.map_or_else(|| talk_task(project), TicketId::new);
    let Some(_reservation) = studio
        .reserve_extra(project, hire, cap_usd, &task.to_string(), &task.to_string())
        .map_err(|error| error.to_string())?
    else {
        return Err(format!("no budget left for {who}"));
    };
    let runtime = studio
        .conversation_runtime(&hire.runtime)
        .map_err(|e| e.to_string())?;
    let mcp = if runtime.capabilities().mcp {
        write_config(studio, project, tools)
            .map_err(|e| format!("{who}'s tools could not be set up: {e}"))?
    } else {
        None
    };
    let spec = RunSpec {
        cwd: cwd.to_path_buf(),
        prompt,
        model: Some(hire.model.clone()).filter(|m| !m.is_empty()),
        effort: hire.effort.clone(),
        append_system_prompt: Some(hire.prompt.clone()).filter(|p| !p.trim().is_empty()),
        max_turns: Some(max_turns),
        max_budget_usd: Some(cap_usd),
        mcp_config: mcp,
        // Only the studio's tools, never the owner's own.
        strict_mcp: true,
        disallowed_tools: DISALLOWED.map(String::from).to_vec(),
        ..RunSpec::default()
    };
    let mut run = runtime
        .start(spec)
        .map_err(|e| format!("{who} could not start: {e}"))?;

    let mut tally = Tally {
        model: hire.model.clone(),
        session: None,
        said: String::new(),
        meter: Meter::new(studio.price(&hire.model)),
    };
    let (finish, stopped) = match tokio::time::timeout(
        limit,
        drive(&mut run, &mut tally, cwd, tools, &mut *observe, false),
    )
    .await
    {
        Ok(finish) => (finish, false),
        Err(_) => {
            // Too long: stop it, and give it a moment to say what it cost.
            let _ = run.interrupt().await;
            let drained = tokio::time::timeout(
                DRAIN,
                drive(&mut run, &mut tally, cwd, tools, &mut *observe, true),
            )
            .await;
            (drained.ok().flatten(), true)
        }
    };

    let (usd, reported) = match &finish {
        Some(Finish { cost, .. }) if cost.reported => (run_spend(cost.usd, 0.0, false), true),
        _ => (tally.meter.usd(), false),
    };
    let (input_tokens, output_tokens) = tally.meter.tokens();
    studio
        .record_spend(&Spend {
            at_ms: crate::now_ms(),
            ticket_id: task,
            project: project.to_string(),
            hire: hire.id.0.clone(),
            runtime: runtime.id().to_string(),
            model: tally.model,
            session: tally.session,
            usd,
            reported,
            input_tokens,
            output_tokens,
        })
        .map_err(|e| e.to_string())?;

    if stopped {
        return Err(format!("{who} took too long"));
    }
    let Some(Finish {
        outcome, summary, ..
    }) = finish
    else {
        return Err(format!("{who} ended without an answer"));
    };
    let text = summary
        .map(|summary| summary.trim().to_string())
        .filter(|summary| !summary.is_empty())
        .unwrap_or_else(|| tally.said.trim().to_string());
    match outcome {
        Outcome::Failed { message, .. } => Err(format!("{who} failed: {message}")),
        Outcome::MaxTurns if text.is_empty() => {
            Err(format!("{who} ran out of turns before it could answer"))
        }
        Outcome::Budget if text.is_empty() => {
            Err(format!("{who} ran out of money before it could answer"))
        }
        _ => Ok(LeadReply { text, usd }),
    }
}

/// What a run has shown so far.
struct Tally {
    model: String,
    session: Option<String>,
    /// Everything the lead said, in case it is cut short before its last words.
    said: String,
    meter: Meter,
}

/// How a run ended.
struct Finish {
    outcome: Outcome,
    cost: Cost,
    summary: Option<String>,
}

/// Reads the run's events until it finishes, answering what it asks, and
/// answers how it ended; none if its events stop without that. A run that is
/// `stopping` may ask for nothing.
async fn drive(
    run: &mut Run,
    tally: &mut Tally,
    cwd: &Path,
    tools: Tools,
    observe: &mut (dyn FnMut(&RunEvent) + Send),
    stopping: bool,
) -> Option<Finish> {
    while let Some(event) = run.next().await {
        observe(&event);
        match event {
            RunEvent::Started { session, model } => {
                tally.session = Some(session);
                if let Some(model) = model {
                    tally.model = model;
                }
            }
            RunEvent::Text { text } => {
                tally.said.push_str(&text);
                tally.said.push('\n');
            }
            RunEvent::Usage {
                message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                ..
            } => tally.meter.update(
                &message_id,
                (
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                ),
            ),
            RunEvent::Permission {
                request_id,
                tool,
                input,
            } => {
                let verdict = if stopping {
                    Err("The studio is stopping this run.".to_string())
                } else {
                    allowed(&tool, &input, cwd, tools)
                };
                let _ = match verdict {
                    Ok(()) => run.allow(&request_id).await,
                    Err(why) => run.deny(&request_id, &why).await,
                };
            }
            RunEvent::Question { request_id, .. } => {
                let _ = run
                    .deny(
                        &request_id,
                        "There is no one to ask here: put the question in your reply.",
                    )
                    .await;
            }
            RunEvent::Finished {
                outcome,
                session,
                cost,
                summary,
            } => {
                if session.is_some() {
                    tally.session = session;
                }
                return Some(Finish {
                    outcome,
                    cost,
                    summary,
                });
            }
            _ => {}
        }
    }
    None
}

/// Whether a hire may make a tool call: read inside its project, plus the
/// studio's tools when this run has them. If not, why, in words for the hire.
fn allowed(tool: &str, input: &Value, cwd: &Path, tools: Tools) -> Result<(), String> {
    if tool.starts_with(STUDIO_TOOLS) {
        return if tools == Tools::ReadOnly {
            Err(
                "This conversation is read-only: the studio's action tools are not available."
                    .to_string(),
            )
        } else {
            Ok(())
        };
    }
    if !READING.contains(&tool) {
        return Err(format!(
            "{tool} isn't allowed here: this conversation can only read the project."
        ));
    }
    let (_, outside) = tool_paths(input, cwd);
    if outside {
        return Err("A hire may only read inside its project.".to_string());
    }
    Ok(())
}

/// Writes the config that gives the run the studio's tools in the mode of
/// `tools`, next to the project's other files of that kind: in `<data>/talk/`
/// or `<data>/setup/`. A read-only conversation gets no config.
fn write_config(studio: &Studio, project: &str, tools: Tools) -> std::io::Result<Option<PathBuf>> {
    let (folder, mode) = match tools {
        Tools::Talk => ("talk", "--talk"),
        Tools::Setup => ("setup", "--setup"),
        Tools::ReadOnly => return Ok(None),
    };
    let path = studio
        .paths
        .data_dir
        .join(folder)
        .join(format!("{}.mcp.json", file_stem(project)));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, mcp_config(&studio.paths, mode, project).to_string())?;
    Ok(Some(path))
}

/// What to call a lead in a message: the name in its file, else its id.
fn called(hire: &Hire) -> String {
    let name = hire.name.trim();
    if name.is_empty() {
        hire.id.0.clone()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::api::Request;
    use serde_json::json;
    use std::collections::HashMap;
    use studio_config::StudioPaths;
    use studio_core::ids::HireId;
    use studio_runtime::{Control, FakeRuntime, Question, Runtime};

    /// A studio with the project `shop`, led by Nora on a fake runtime that
    /// plays `runtime`'s script, and what the tests need to look at.
    struct Lead {
        home: tempfile::TempDir,
        studio: Arc<Studio>,
        runtime: FakeRuntime,
        repo: PathBuf,
        hire: Hire,
    }

    impl Lead {
        /// A folder for the studio, and the shop's repository in it.
        fn folders() -> (tempfile::TempDir, PathBuf) {
            let home = tempfile::tempdir().unwrap();
            let repo = home.path().join("shop");
            std::fs::create_dir_all(&repo).unwrap();
            (home, repo)
        }

        fn new(runtime: FakeRuntime) -> Lead {
            Lead::with_day(runtime, 5.0)
        }

        /// The same, with the studio's daily limit at `daily_usd`.
        fn with_day(runtime: FakeRuntime, daily_usd: f64) -> Lead {
            let (home, repo) = Lead::folders();
            Lead::open(home, repo, runtime, daily_usd)
        }

        /// The studio in `home`, whose shop is at `repo`.
        fn open(
            home: tempfile::TempDir,
            repo: PathBuf,
            runtime: FakeRuntime,
            daily_usd: f64,
        ) -> Lead {
            std::fs::write(
                home.path().join("studio.toml"),
                format!(
                    "[budget]\ndaily_usd = {daily_usd}\n\n[concurrency]\nmax_runs = 2\n\n\
                     [prices.fake-1]\ninput_per_mtok = 1.0\noutput_per_mtok = 5.0\n\n\
                     [[projects]]\nname = \"shop\"\npath = \"{}\"\n",
                    repo.display()
                ),
            )
            .unwrap();
            std::fs::create_dir_all(home.path().join("hires")).unwrap();
            std::fs::write(
                home.path().join("hires/nora.md"),
                "---\nname: Nora\nrole: lead\nruntime: fake\nmodel: fake-1\neffort: high\n---\n\
                 You lead the shop.\n",
            )
            .unwrap();
            let mut runtimes: HashMap<String, Arc<dyn Runtime>> = HashMap::new();
            runtimes.insert("fake".into(), Arc::new(runtime.clone()));
            let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
            let studio = Studio::open(paths, runtimes).unwrap();
            let hire = studio.hire(&HireId::new("nora")).unwrap();
            Lead {
                home,
                studio,
                runtime,
                repo,
                hire,
            }
        }

        /// A talk run for the shop: a short question, capped at 50 cents.
        fn talk(&self) -> LeadRun<'_> {
            LeadRun {
                project: "shop",
                hire: &self.hire,
                cwd: &self.repo,
                prompt: "How's it going?".into(),
                tools: Tools::Talk,
                charge_task: None,
                cap_usd: 0.5,
                max_turns: 8,
            }
        }

        /// What the ledger holds for the last 24 hours, as `Budget` tells it.
        async fn spent(&self) -> Value {
            let budget = self.studio.handle(Request::Budget).await;
            assert!(budget.ok, "{:?}", budget.error);
            budget.data["spent_24h"].clone()
        }
    }

    fn finished(outcome: Outcome, summary: Option<&str>, usd: f64) -> RunEvent {
        RunEvent::Finished {
            outcome,
            session: Some("s1".into()),
            cost: Cost {
                usd,
                reported: usd > 0.0,
                input_tokens: 1_000,
                output_tokens: 200,
                cumulative: true,
            },
            summary: summary.map(String::from),
        }
    }

    fn started() -> RunEvent {
        RunEvent::Started {
            session: "s1".into(),
            model: Some("fake-1".into()),
        }
    }

    fn said(text: &str) -> RunEvent {
        RunEvent::Text { text: text.into() }
    }

    fn usage(input: u64, output: u64) -> RunEvent {
        RunEvent::Usage {
            message_id: "m1".into(),
            model: Some("fake-1".into()),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    /// A run in which the lead says something on the way and ends with `last`.
    fn script(last: &str, usd: f64) -> Vec<RunEvent> {
        vec![
            started(),
            said("Let me look."),
            usage(1_000, 200),
            said(last),
            finished(Outcome::Success, Some(last), usd),
        ]
    }

    fn ask(request_id: &str, tool: &str, input: Value) -> RunEvent {
        RunEvent::Permission {
            request_id: request_id.into(),
            tool: tool.into(),
            input,
        }
    }

    #[tokio::test]
    async fn a_conversation_in_simulation_mode_costs_nothing_and_calls_no_model() {
        let lead = Lead::new(FakeRuntime::new(script("Real words.", 0.07)));
        let set = lead
            .studio
            .handle(Request::SetOperations {
                autopilot: None,
                review_minutes: None,
                autonomy: None,
                mode: Some(crate::api::RuntimeMode::Simulation),
            })
            .await;
        assert!(set.ok, "{:?}", set.error);
        let reply = run(&lead.studio, lead.talk()).await.unwrap();
        assert!(reply.text.contains("Simulated response"), "{}", reply.text);
        assert_eq!(reply.usd, 0.0);
        assert!(lead.runtime.specs().is_empty());
        assert_eq!(lead.spent().await["total"], 0.0);
    }

    #[tokio::test]
    async fn the_leads_last_words_come_back_with_what_the_run_cost() {
        let lead = Lead::new(FakeRuntime::new(script(
            "The logo's done. Two things wait for you.",
            0.07,
        )));
        let reply = run(&lead.studio, lead.talk()).await.unwrap();
        // Not what it said on the way.
        assert_eq!(
            reply,
            LeadReply {
                text: "The logo's done. Two things wait for you.".into(),
                usd: 0.07
            }
        );
    }

    #[tokio::test]
    async fn the_run_starts_as_the_leads_read_only_turn() {
        let lead = Lead::new(FakeRuntime::new(script("Hi.", 0.01)));
        run(&lead.studio, lead.talk()).await.unwrap();
        let specs = lead.runtime.specs();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.cwd, lead.repo);
        assert_eq!(spec.prompt, "How's it going?");
        assert_eq!(spec.model.as_deref(), Some("fake-1"));
        assert_eq!(spec.effort.as_deref(), Some("high"));
        assert_eq!(
            spec.append_system_prompt.as_deref(),
            Some("You lead the shop.")
        );
        assert_eq!(spec.max_turns, Some(8));
        assert_eq!(spec.max_budget_usd, Some(0.5));
        assert!(spec.strict_mcp);
        assert_eq!(spec.resume, None);
        assert_eq!(
            spec.disallowed_tools,
            [
                "Bash",
                "Edit",
                "Write",
                "NotebookEdit",
                "WebFetch",
                "WebSearch",
                "Task"
            ]
        );
        // This runtime has no MCP, so there is nothing to point it at.
        assert_eq!(spec.mcp_config, None);
    }

    #[tokio::test]
    async fn the_studios_tools_are_for_the_mode_the_run_is_in() {
        let lead = Lead::new(FakeRuntime::new(script("Hi.", 0.01)).with_mcp());
        let socket = crate::api::socket_path(&lead.studio.paths.data_dir);
        let in_mode = |tools: Tools| LeadRun {
            tools,
            ..lead.talk()
        };
        run(&lead.studio, in_mode(Tools::Talk)).await.unwrap();
        run(&lead.studio, in_mode(Tools::Setup)).await.unwrap();
        let specs = lead.runtime.specs();
        let data = lead.home.path().join("data");
        for (spec, folder, mode) in [
            (&specs[0], "talk", "--talk"),
            (&specs[1], "setup", "--setup"),
        ] {
            let path = spec.mcp_config.clone().unwrap();
            assert_eq!(path, data.join(folder).join("shop.mcp.json"));
            let config: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let server = &config["mcpServers"]["studio"];
            assert!(server["command"].is_string());
            assert_eq!(
                server["args"],
                json!(["mcp", mode, "shop", "--socket", socket.to_string_lossy()])
            );
        }
    }

    #[tokio::test]
    async fn a_direct_employee_conversation_has_no_action_tools() {
        let events = vec![
            started(),
            ask("tool", "mcp__studio__cancel", json!({"task": "SHOP-3"})),
            finished(Outcome::Success, Some("I can't do that here."), 0.01),
        ];
        let lead = Lead::new(FakeRuntime::new(events).with_mcp());
        let read_only = LeadRun {
            tools: Tools::ReadOnly,
            ..lead.talk()
        };
        run(&lead.studio, read_only).await.unwrap();
        assert_eq!(lead.runtime.specs()[0].mcp_config, None);
        let answers = answers(&lead.runtime);
        assert_eq!(answers.len(), 1);
        assert!(answers[0].1.as_deref().unwrap().contains("read-only"));
    }

    #[tokio::test]
    async fn the_spend_is_recorded_under_the_projects_talk_task() {
        let lead = Lead::new(FakeRuntime::new(script("Hi.", 0.07)));
        assert_eq!(talk_task("shop").to_string(), "shop/talk");
        assert!(
            lead.spent().await["by_ticket"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        run(&lead.studio, lead.talk()).await.unwrap();
        let spent = lead.spent().await;
        assert_eq!(spent["by_ticket"], json!({"shop/talk": 0.07}));
        assert_eq!(spent["by_project"], json!({"shop": 0.07}));
        assert_eq!(spent["by_hire"], json!({"nora": 0.07}));
        assert_eq!(spent["by_model"], json!({"fake-1": 0.07}));
        // One row for the run, which counts against the day.
        assert_eq!(spent["runs_by_hire"], json!({"nora": 1}));
        assert_eq!(spent["total"], 0.07);
    }

    #[tokio::test]
    async fn a_run_that_reports_no_cost_is_charged_what_its_tokens_come_to() {
        let events = vec![
            started(),
            usage(100_000, 20_000),
            finished(Outcome::Success, Some("Hi."), 0.0),
        ];
        let lead = Lead::new(FakeRuntime::new(events));
        let reply = run(&lead.studio, lead.talk()).await.unwrap();
        // $1 and $5 a million tokens.
        assert!((reply.usd - 0.2).abs() < 1e-9, "{}", reply.usd);
        let spent = lead.spent().await;
        assert!((spent["by_ticket"]["shop/talk"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    }

    #[tokio::test]
    async fn no_headroom_refuses_the_run_before_it_starts() {
        // Less left in the day than the run may cost.
        let lead = Lead::with_day(FakeRuntime::new(script("Hi.", 0.07)), 0.4);
        let refused = run(&lead.studio, lead.talk()).await;
        assert_eq!(refused, Err("no budget left for Nora".to_string()));
        assert!(lead.runtime.specs().is_empty());
        assert!(
            lead.spent().await["by_ticket"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        // The same run with room for its cap goes ahead.
        let room = Lead::with_day(FakeRuntime::new(script("Hi.", 0.07)), 0.5);
        assert!(run(&room.studio, room.talk()).await.is_ok());
        assert_eq!(room.runtime.specs().len(), 1);
    }

    #[tokio::test]
    async fn a_concurrent_lead_run_cannot_spend_another_runs_reservation() {
        let (runtime, release) = FakeRuntime::new(script("Hi.", 0.07)).held();
        // After the first run costs $0.07, another full-cap run still fits.
        let lead = Lead::with_day(runtime, 0.58);
        let studio = Arc::clone(&lead.studio);
        let hire = lead.hire.clone();
        let repo = lead.repo.clone();
        let first = tokio::spawn(async move {
            run(
                &studio,
                LeadRun {
                    project: "shop",
                    hire: &hire,
                    cwd: &repo,
                    prompt: "First".into(),
                    tools: Tools::Talk,
                    charge_task: None,
                    cap_usd: 0.5,
                    max_turns: 8,
                },
            )
            .await
        });
        for _ in 0..100 {
            if !lead.runtime.specs().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(lead.runtime.specs().len(), 1);
        let refused = run(&lead.studio, lead.talk()).await;
        assert_eq!(refused, Err("no budget left for Nora".to_string()));
        assert_eq!(lead.runtime.specs().len(), 1);
        release.notify_one();
        assert!(first.await.unwrap().is_ok());
        // The reservation goes away, so another full-cap allowance is available.
        let again = lead
            .studio
            .reserve_extra("shop", &lead.hire, 0.5, "shop/talk", "shop/talk")
            .unwrap();
        assert!(again.is_some());
    }

    #[tokio::test]
    async fn a_lead_the_runtime_does_not_know_is_refused_and_costs_nothing() {
        let lead = Lead::new(FakeRuntime::new(script("Hi.", 0.07)));
        let elsewhere = Hire {
            runtime: "elsewhere".into(),
            ..lead.hire.clone()
        };
        let run_it = LeadRun {
            hire: &elsewhere,
            ..lead.talk()
        };
        let refused = run(&lead.studio, run_it).await.unwrap_err();
        assert!(refused.contains("elsewhere"), "{refused}");
        assert!(lead.runtime.specs().is_empty());
        assert!(
            lead.spent().await["by_ticket"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    /// What the lead's run was told for each request, in order: none for an
    /// allowed one, else the reason.
    fn answers(runtime: &FakeRuntime) -> Vec<(String, Option<String>)> {
        runtime
            .received()
            .into_iter()
            .map(|control| match control {
                Control::Allow { request_id } => (request_id, None),
                Control::Deny {
                    request_id,
                    message,
                } => (request_id, Some(message)),
                other => panic!("the run was sent {other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn a_bash_request_is_denied_in_plain_words() {
        let events = vec![
            started(),
            ask("p1", "Bash", json!({"command": "rm -rf ."})),
            finished(Outcome::Success, Some("Done."), 0.01),
        ];
        let lead = Lead::new(FakeRuntime::new(events));
        assert!(run(&lead.studio, lead.talk()).await.is_ok());
        let answers = answers(&lead.runtime);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].0, "p1");
        let why = answers[0].1.as_deref().unwrap();
        assert!(why.contains("Bash isn't allowed here"), "{why}");
    }

    #[tokio::test]
    async fn a_lead_may_read_its_project_and_use_the_studios_tools_and_nothing_else() {
        let (home, repo) = Lead::folders();
        let events = vec![
            started(),
            ask("read", "Read", json!({"file_path": "src/main.rs"})),
            ask(
                "whole",
                "Read",
                json!({"file_path": repo.join("src/main.rs")}),
            ),
            ask("grep", "Grep", json!({"pattern": "TODO"})),
            ask("glob", "Glob", json!({"pattern": "**/*.rs", "path": "src"})),
            ask("ls", "LS", json!({"path": "."})),
            ask("tool", "mcp__studio__send_back", json!({"task": "SHOP-3"})),
            ask("outside", "Read", json!({"file_path": "/etc/hosts"})),
            ask("up", "Read", json!({"file_path": "../elsewhere/notes.txt"})),
            ask("listing", "LS", json!({"path": "/"})),
            ask("edit", "Edit", json!({"file_path": "src/main.rs"})),
            ask("web", "WebFetch", json!({"url": "https://example.com"})),
            RunEvent::Question {
                request_id: "question".into(),
                questions: vec![Question {
                    question: "Which one?".into(),
                    header: "Pick".into(),
                    options: vec!["A".into(), "B".into()],
                    multi_select: false,
                }],
            },
            finished(Outcome::Success, Some("Done."), 0.01),
        ];
        let lead = Lead::open(home, repo, FakeRuntime::new(events), 5.0);
        assert!(run(&lead.studio, lead.talk()).await.is_ok());
        let answers = answers(&lead.runtime);
        let allowed: Vec<&str> = answers
            .iter()
            .filter(|(_, why)| why.is_none())
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(allowed, ["read", "whole", "grep", "glob", "ls", "tool"]);
        let denied = |id: &str| {
            answers
                .iter()
                .find(|(answered, _)| answered == id)
                .and_then(|(_, why)| why.clone())
                .unwrap_or_else(|| panic!("{id} was not denied"))
        };
        for id in ["outside", "up", "listing"] {
            assert!(denied(id).contains("only read inside"), "{id}");
        }
        assert!(denied("edit").contains("Edit isn't allowed"));
        assert!(denied("web").contains("WebFetch isn't allowed"));
        assert!(denied("question").contains("no one to ask"));
    }

    #[tokio::test]
    async fn a_run_that_fails_says_why_and_is_still_charged() {
        let failed = Outcome::Failed {
            kind: studio_runtime::FailureKind::Crash,
            message: "the model fell over".into(),
        };
        let lead = Lead::new(FakeRuntime::new(vec![
            started(),
            finished(failed, None, 0.03),
        ]));
        let error = run(&lead.studio, lead.talk()).await.unwrap_err();
        assert_eq!(error, "Nora failed: the model fell over");
        assert_eq!(lead.spent().await["by_ticket"], json!({"shop/talk": 0.03}));
    }

    #[tokio::test]
    async fn a_run_cut_short_gives_what_the_lead_had_said_or_says_why_not() {
        for (outcome, cut) in [(Outcome::MaxTurns, "turns"), (Outcome::Budget, "money")] {
            let with_words = vec![
                started(),
                said("I'll start the goal now."),
                finished(outcome.clone(), None, 0.5),
            ];
            let lead = Lead::new(FakeRuntime::new(with_words));
            let reply = run(&lead.studio, lead.talk()).await.unwrap();
            assert_eq!(reply.text, "I'll start the goal now.");
            assert_eq!(reply.usd, 0.5);

            let silent = vec![started(), finished(outcome, None, 0.5)];
            let lead = Lead::new(FakeRuntime::new(silent));
            let error = run(&lead.studio, lead.talk()).await.unwrap_err();
            assert_eq!(
                error,
                format!("Nora ran out of {cut} before it could answer")
            );
            // Whatever came of it, it cost what it cost.
            assert_eq!(lead.spent().await["by_ticket"], json!({"shop/talk": 0.5}));
        }
    }

    #[tokio::test]
    async fn a_run_that_ends_with_nothing_to_say_answers_nothing() {
        let lead = Lead::new(FakeRuntime::new(vec![
            started(),
            finished(Outcome::Success, None, 0.02),
        ]));
        let reply = run(&lead.studio, lead.talk()).await.unwrap();
        assert_eq!(
            reply,
            LeadReply {
                text: String::new(),
                usd: 0.02
            }
        );
    }

    #[tokio::test]
    async fn a_run_that_takes_too_long_is_stopped_and_charged_what_it_spent() {
        // The fake holds its run back from finishing until it is interrupted.
        let (runtime, _release) = FakeRuntime::new(script("Hi.", 0.03)).held();
        let lead = Lead::new(runtime);
        let late = go(
            &lead.studio,
            lead.talk(),
            &mut |_| {},
            Duration::from_millis(150),
        )
        .await;
        assert_eq!(late, Err("Nora took too long".to_string()));
        assert!(lead.runtime.received().contains(&Control::Interrupt));
        assert_eq!(lead.spent().await["by_ticket"], json!({"shop/talk": 0.03}));
    }

    #[tokio::test]
    async fn what_the_lead_does_can_be_watched_as_it_happens() {
        let events = vec![
            started(),
            said("Sending it back."),
            RunEvent::ToolCall {
                id: "t1".into(),
                name: "mcp__studio__send_back".into(),
                input: json!({"task": "SHOP-3", "note": "Too many colours."}),
            },
            ask("tool", "mcp__studio__send_back", json!({"task": "SHOP-3"})),
            RunEvent::ToolResult {
                id: "t1".into(),
                output: "sent back".into(),
                is_error: false,
            },
            finished(Outcome::Success, Some("Sent it back."), 0.04),
        ];
        let lead = Lead::new(FakeRuntime::new(events));
        let mut calls = Vec::new();
        let mut results = Vec::new();
        let reply = run_with(&lead.studio, lead.talk(), &mut |event| match event {
            RunEvent::ToolCall { name, input, .. } => calls.push((name.clone(), input.clone())),
            RunEvent::ToolResult { output, .. } => results.push(output.clone()),
            _ => {}
        })
        .await
        .unwrap();
        assert_eq!(reply.text, "Sent it back.");
        assert_eq!(
            calls,
            [(
                "mcp__studio__send_back".to_string(),
                json!({"task": "SHOP-3", "note": "Too many colours."})
            )]
        );
        assert_eq!(results, ["sent back"]);
    }
}
