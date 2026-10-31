use crate::Runtime;
use crate::run::{
    Capabilities, Control, Cost, EnvReport, Outcome, Run, RunEvent, RunSpec, RuntimeError,
};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

/// A scripted runtime for tests. It emits its events in order, waits for a
/// control after every `Permission` or `Question`, and records what it got.
#[derive(Clone, Default)]
pub struct FakeRuntime {
    script: Vec<RunEvent>,
    /// When set, the nth run plays the nth script, and the last one repeats.
    sequence: Vec<Vec<RunEvent>>,
    no_cost_reports: bool,
    mcp: bool,
    /// When set, a run waits for this before finishing.
    hold: Option<Arc<Notify>>,
    received: Arc<Mutex<Vec<Control>>>,
    specs: Arc<Mutex<Vec<RunSpec>>>,
}

impl FakeRuntime {
    pub fn new(script: Vec<RunEvent>) -> FakeRuntime {
        FakeRuntime {
            script,
            ..FakeRuntime::default()
        }
    }

    /// A fake whose runs play one script each, in order; the last repeats.
    pub fn sequence(scripts: Vec<Vec<RunEvent>>) -> FakeRuntime {
        FakeRuntime {
            sequence: scripts,
            ..FakeRuntime::default()
        }
    }

    /// A fake that claims it can't report cost, like a model API without usage data.
    pub fn without_cost_reports(script: Vec<RunEvent>) -> FakeRuntime {
        FakeRuntime {
            script,
            no_cost_reports: true,
            ..FakeRuntime::default()
        }
    }

    /// Claims MCP support, so the studio hands its runs an MCP config.
    pub fn with_mcp(mut self) -> FakeRuntime {
        self.mcp = true;
        self
    }

    /// Runs wait before finishing until `notify_one` is called on the returned
    /// handle, once per run.
    pub fn held(mut self) -> (FakeRuntime, Arc<Notify>) {
        let hold = Arc::new(Notify::new());
        self.hold = Some(Arc::clone(&hold));
        (self, hold)
    }

    /// Controls the runs received, in order.
    pub fn received(&self) -> Vec<Control> {
        self.received.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Specs the runs were started with, in order.
    pub fn specs(&self) -> Vec<RunSpec> {
        self.specs.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Runtime for FakeRuntime {
    fn id(&self) -> &str {
        "fake"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            skills: false,
            mcp: self.mcp,
            resume: false,
            approvals: true,
            reports_cost: !self.no_cost_reports,
        }
    }

    fn check_env(&self) -> EnvReport {
        EnvReport {
            installed: true,
            version: Some("fake".to_string()),
            problem: None,
        }
    }

    fn start(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        let mut nth = 0;
        if let Ok(mut specs) = self.specs.lock() {
            nth = specs.len();
            specs.push(spec);
        }
        let (event_tx, event_rx) = mpsc::channel(64);
        let (control_tx, mut control_rx) = mpsc::channel(16);
        let script = self
            .sequence
            .get(nth)
            .or(self.sequence.last())
            .unwrap_or(&self.script)
            .clone();
        let received = Arc::clone(&self.received);
        let hold = self.hold.clone();
        tokio::spawn(async move {
            let mut finished = false;
            for event in script {
                if let (RunEvent::Finished { .. }, Some(hold)) = (&event, &hold) {
                    hold.notified().await;
                }
                let waits = matches!(
                    event,
                    RunEvent::Permission { .. } | RunEvent::Question { .. }
                );
                finished = matches!(event, RunEvent::Finished { .. });
                if event_tx.send(event).await.is_err() {
                    return;
                }
                if waits {
                    let Some(control) = control_rx.recv().await else {
                        return;
                    };
                    if let Ok(mut log) = received.lock() {
                        log.push(control);
                    }
                }
                if finished {
                    break;
                }
            }
            if !finished {
                let _ = event_tx
                    .send(RunEvent::Finished {
                        outcome: Outcome::Success,
                        session: None,
                        cost: Cost::default(),
                        summary: None,
                    })
                    .await;
            }
        });
        Ok(Run::new(event_rx, control_tx))
    }
}
