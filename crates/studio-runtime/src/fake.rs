use crate::Runtime;
use crate::run::{
    Capabilities, Control, Cost, EnvReport, Outcome, Run, RunEvent, RunSpec, RuntimeError,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, mpsc};

/// A scripted runtime for tests. It emits its events in order, waits for a
/// control after every `Permission` or `Question`, and records what it got. A
/// `Say` is recorded wherever the run is, and is never the control it waits
/// for.
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
    /// handle, once per run, or until they get a control such as an interrupt.
    pub fn held(mut self) -> (FakeRuntime, Arc<Notify>) {
        let hold = Arc::new(Notify::new());
        self.hold = Some(Arc::clone(&hold));
        (self, hold)
    }

    /// Controls the runs received, in order.
    pub fn received(&self) -> Vec<Control> {
        self.received.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// What the runs were told with `Control::Say`, in order.
    pub fn told(&self) -> Vec<String> {
        self.received()
            .into_iter()
            .filter_map(|control| match control {
                Control::Say { text } => Some(text),
                _ => None,
            })
            .collect()
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
        let (control_tx, control_rx) = mpsc::channel(16);
        let script = self
            .sequence
            .get(nth)
            .or(self.sequence.last())
            .unwrap_or(&self.script)
            .clone();
        let mut inbox = Inbox {
            controls: control_rx,
            held_back: VecDeque::new(),
            received: Arc::clone(&self.received),
        };
        let hold = self.hold.clone();
        tokio::spawn(async move {
            let mut finished = false;
            for event in script {
                inbox.drain();
                if let (RunEvent::Finished { .. }, Some(hold)) = (&event, &hold) {
                    // An interrupt ends a held run early, as it would a real one.
                    tokio::select! {
                        _ = hold.notified() => {}
                        Some(_) = inbox.next() => {}
                    }
                }
                let waits = matches!(
                    event,
                    RunEvent::Permission { .. } | RunEvent::Question { .. }
                );
                finished = matches!(event, RunEvent::Finished { .. });
                if event_tx.send(event).await.is_err() {
                    return;
                }
                if waits && inbox.next().await.is_none() {
                    return;
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

/// What a fake run was sent. A `Say` is recorded as soon as the run sees it,
/// wherever the run is. Any other control is kept back until the run waits for
/// one, so a decision or an interrupt sent early still counts when it does.
struct Inbox {
    controls: mpsc::Receiver<Control>,
    held_back: VecDeque<Control>,
    received: Arc<Mutex<Vec<Control>>>,
}

impl Inbox {
    fn record(&self, control: Control) {
        if let Ok(mut log) = self.received.lock() {
            log.push(control);
        }
    }

    /// Takes in what was sent so far, without waiting.
    fn drain(&mut self) {
        while let Ok(control) = self.controls.try_recv() {
            self.take(control);
        }
    }

    fn take(&mut self, control: Control) {
        if matches!(control, Control::Say { .. }) {
            self.record(control);
        } else {
            self.held_back.push_back(control);
        }
    }

    /// The next control that is not a `Say`, recorded as it is taken; waits
    /// for it. None once the studio has dropped the run.
    async fn next(&mut self) -> Option<Control> {
        loop {
            if let Some(control) = self.held_back.pop_front() {
                self.record(control.clone());
                return Some(control);
            }
            let sent = self.controls.recv().await?;
            self.take(sent);
        }
    }
}
