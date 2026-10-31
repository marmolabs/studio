//! Agent runtimes behind one interface. Each adapter turns its runtime's own
//! protocol into [`RunEvent`]s and accepts [`Control`]s (approvals, answers,
//! interrupts) back.
//!
//! Only Claude Code ships today. Another agent (an ACP client, say) is one
//! more adapter implementing [`Runtime`] and one more provider kind in
//! studiod's `runtimes::build`.

mod claude;
mod fake;
mod run;

pub use claude::{ClaudeCode, Parsed, parse_line};
pub use fake::FakeRuntime;
pub use run::{
    Capabilities, Control, Cost, EnvReport, FailureKind, Outcome, Question, Run, RunEvent, RunSpec,
    RuntimeError,
};

pub trait Runtime: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    /// Is the runtime installed and usable? Never reads credentials.
    fn check_env(&self) -> EnvReport;
    /// Starts a run. Must be called from inside a tokio runtime.
    fn start(&self, spec: RunSpec) -> Result<Run, RuntimeError>;
}
