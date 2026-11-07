//! What kind of run each ledger row is, and how rows add up by kind.

use serde::{Deserialize, Serialize};

/// What a run was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpendKind {
    /// A hire doing a task.
    Coding,
    /// A reviewer judging a task.
    Review,
    /// A critic judging a task against a gate.
    Critic,
    /// A run that plans a goal.
    Plan,
    /// The approver answering a question about a tool call.
    Approval,
    /// The router choosing who takes a task.
    Routing,
    /// The owner talking with a lead or a hire.
    Conversation,
    /// A hire answering in a room.
    Room,
    /// A standup.
    Standup,
    /// A lead reading a project to set it up.
    Setup,
    /// A lead's regular review of how a project runs.
    ManagerReview,
    /// A run from before the ledger kept the kind.
    Earlier,
}

impl SpendKind {
    /// The name the ledger stores.
    pub fn as_str(self) -> &'static str {
        match self {
            SpendKind::Coding => "coding",
            SpendKind::Review => "review",
            SpendKind::Critic => "critic",
            SpendKind::Plan => "plan",
            SpendKind::Approval => "approval",
            SpendKind::Routing => "routing",
            SpendKind::Conversation => "conversation",
            SpendKind::Room => "room",
            SpendKind::Standup => "standup",
            SpendKind::Setup => "setup",
            SpendKind::ManagerReview => "manager_review",
            SpendKind::Earlier => "earlier",
        }
    }

    /// The kind a stored name stands for; a name nobody knows is an earlier run.
    pub fn from_name(name: &str) -> SpendKind {
        [
            SpendKind::Coding,
            SpendKind::Review,
            SpendKind::Critic,
            SpendKind::Plan,
            SpendKind::Approval,
            SpendKind::Routing,
            SpendKind::Conversation,
            SpendKind::Room,
            SpendKind::Standup,
            SpendKind::Setup,
            SpendKind::ManagerReview,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == name)
        .unwrap_or(SpendKind::Earlier)
    }

    /// What to call the kind in a list, in the plural.
    pub fn label(self) -> &'static str {
        match self {
            SpendKind::Coding => "Coding",
            SpendKind::Review => "Reviews",
            SpendKind::Critic => "Gate checks",
            SpendKind::Plan => "Plans",
            SpendKind::Approval => "Approvals",
            SpendKind::Routing => "Routing",
            SpendKind::Conversation => "Conversations",
            SpendKind::Room => "Room replies",
            SpendKind::Standup => "Standups",
            SpendKind::Setup => "Setup",
            SpendKind::ManagerReview => "Manager reviews",
            SpendKind::Earlier => "Earlier runs",
        }
    }
}

/// Runs of one kind by one hire on one model in one project: what they came to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendLine {
    pub kind: SpendKind,
    pub project: String,
    pub hire: String,
    pub model: String,
    pub calls: u64,
    pub usd: f64,
}

/// All the runs of one kind, added up.
#[derive(Clone, Debug, PartialEq)]
pub struct KindTotal {
    pub kind: SpendKind,
    pub calls: u64,
    pub usd: f64,
    /// The models that ran them, the dearest first.
    pub models: Vec<String>,
}

/// The lines added up by kind, the dearest kind first.
pub fn by_kind(lines: &[SpendLine]) -> Vec<KindTotal> {
    let mut totals: Vec<(KindTotal, Vec<(String, f64)>)> = Vec::new();
    for line in lines {
        let at = match totals.iter().position(|(t, _)| t.kind == line.kind) {
            Some(at) => at,
            None => {
                let total = KindTotal {
                    kind: line.kind,
                    calls: 0,
                    usd: 0.0,
                    models: Vec::new(),
                };
                totals.push((total, Vec::new()));
                totals.len() - 1
            }
        };
        let (total, models) = &mut totals[at];
        total.calls += line.calls;
        total.usd += line.usd;
        match models.iter_mut().find(|(m, _)| *m == line.model) {
            Some((_, usd)) => *usd += line.usd,
            None => models.push((line.model.clone(), line.usd)),
        }
    }
    let mut out: Vec<KindTotal> = totals
        .into_iter()
        .map(|(mut total, mut models)| {
            models.sort_by(|a, b| b.1.total_cmp(&a.1));
            total.models = models.into_iter().map(|(m, _)| m).collect();
            total
        })
        .collect();
    out.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    out
}

/// The least a kind must cost before it is worth a warning.
const WARN_MIN_USD: f64 = 0.25;

/// A plain line when approvals or routing cost more than the coding, or
/// none when they don't. `scope` finishes the sentence: "on this goal".
pub fn warning(lines: &[SpendLine], scope: &str) -> Option<String> {
    let totals = by_kind(lines);
    let of = |kind: SpendKind| {
        totals
            .iter()
            .find(|t| t.kind == kind)
            .map_or(0.0, |t| t.usd)
    };
    let coding = of(SpendKind::Coding);
    let (kind, usd) = [SpendKind::Approval, SpendKind::Routing]
        .into_iter()
        .map(|kind| (kind, of(kind)))
        .filter(|(_, usd)| *usd >= WARN_MIN_USD && *usd > coding)
        .max_by(|a, b| a.1.total_cmp(&b.1))?;
    let name = match kind {
        SpendKind::Approval => "Approvals",
        _ => "Routing",
    };
    Some(format!(
        "{name} cost {} {scope}, more than the coding, {}.",
        dollars(usd),
        dollars(coding)
    ))
}

/// Money as the pages show it: cents, or a fraction of a cent when tiny.
pub fn dollars(usd: f64) -> String {
    if usd.abs() < 0.01 && usd != 0.0 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn line(kind: SpendKind, model: &str, calls: u64, usd: f64) -> SpendLine {
        SpendLine {
            kind,
            project: "shop".into(),
            hire: "h".into(),
            model: model.into(),
            calls,
            usd,
        }
    }

    #[test]
    fn names_round_trip_and_unknown_names_are_earlier_runs() {
        for kind in [
            SpendKind::Coding,
            SpendKind::ManagerReview,
            SpendKind::Approval,
        ] {
            assert_eq!(SpendKind::from_name(kind.as_str()), kind);
        }
        assert_eq!(SpendKind::from_name("nonsense"), SpendKind::Earlier);
    }

    #[test]
    fn kinds_add_up_dearest_first_with_their_models() {
        let totals = by_kind(&[
            line(SpendKind::Coding, "sonnet", 2, 1.0),
            line(SpendKind::Approval, "haiku", 10, 0.1),
            line(SpendKind::Approval, "opus", 9, 1.6),
        ]);
        assert_eq!(totals[0].kind, SpendKind::Approval);
        assert_eq!(totals[0].calls, 19);
        assert_eq!(totals[0].models, ["opus", "haiku"]);
        assert_eq!(totals[1].kind, SpendKind::Coding);
    }

    #[test]
    fn it_warns_only_when_approvals_or_routing_beat_the_coding() {
        let loud = [
            line(SpendKind::Coding, "sonnet", 2, 1.0),
            line(SpendKind::Approval, "opus", 19, 1.61),
        ];
        let said = warning(&loud, "on this goal").unwrap();
        assert_eq!(
            said,
            "Approvals cost $1.61 on this goal, more than the coding, $1.00."
        );
        let quiet = [
            line(SpendKind::Coding, "sonnet", 2, 3.0),
            line(SpendKind::Approval, "opus", 19, 1.61),
        ];
        assert_eq!(warning(&quiet, "today"), None);
        let tiny = [line(SpendKind::Routing, "opus", 2, 0.1)];
        assert_eq!(warning(&tiny, "today"), None);
    }
}
