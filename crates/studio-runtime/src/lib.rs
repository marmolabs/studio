//! Agent runtimes behind one interface. Each adapter turns its runtime's own
//! protocol into [`RunEvent`]s and accepts [`Control`]s (approvals, answers,
//! interrupts) back.
//!
//! Claude Code is the live adapter; a local simulation adapter supports
//! zero-cost conversations and reviews. Another agent (an ACP client, say) is
//! one more adapter implementing [`Runtime`], added to studiod's runtime list.

mod claude;
mod fake;
mod run;
mod simulation;

pub use claude::ClaudeCode;
pub use fake::FakeRuntime;
pub use run::{
    Capabilities, Control, Cost, EnvReport, FailureKind, Outcome, Question, Run, RunEvent, RunSpec,
    RuntimeError,
};
pub use simulation::SimulatedRuntime;

pub trait Runtime: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Is the runtime installed and usable? Never reads credentials.
    fn check_env(&self) -> EnvReport;
    /// Starts a run. Must be called from inside a tokio runtime.
    fn start(&self, spec: RunSpec) -> Result<Run, RuntimeError>;
}
