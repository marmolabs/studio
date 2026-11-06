use crate::{
    Capabilities, Cost, EnvReport, Outcome, Run, RunEvent, RunSpec, Runtime, RuntimeError,
};

pub struct SimulatedRuntime;

impl Runtime for SimulatedRuntime {
    fn id(&self) -> &str {
        "simulation"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            skills: false,
            mcp: false,
            resume: false,
            approvals: true,
            reports_cost: true,
        }
    }

    fn check_env(&self) -> EnvReport {
        EnvReport {
            installed: true,
            version: Some("simulation".to_string()),
            problem: None,
        }
    }

    fn start(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        let summary = if spec.prompt.contains("Return only a JSON object") {
            "{\"summary\":\"Simulation review complete. No model was called and no action is proposed.\",\"actions\":[]}".to_string()
        } else if spec.prompt.contains("VERDICT: PASS") {
            "Simulated review.\nVERDICT: PASS".to_string()
        } else if spec.prompt.contains("DECISION: ALLOW") {
            "Simulated approval.\nDECISION: ALLOW: simulation".to_string()
        } else {
            "Simulated response. No model was called and no changes were made.".to_string()
        };
        let (event_tx, event_rx) = tokio::sync::mpsc::channel(4);
        let (control_tx, control_rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = control_rx;
            let _ = event_tx
                .send(RunEvent::Started {
                    session: "simulation".to_string(),
                    model: Some("simulation".to_string()),
                })
                .await;
            let _ = event_tx
                .send(RunEvent::Finished {
                    outcome: Outcome::Success,
                    session: Some("simulation".to_string()),
                    cost: Cost {
                        usd: 0.0,
                        reported: true,
                        ..Cost::default()
                    },
                    summary: Some(summary),
                })
                .await;
        });
        Ok(Run::new(event_rx, control_tx))
    }
}
