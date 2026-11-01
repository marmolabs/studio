use crate::ids::{HireId, RuleId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Guide,
    Protect,
    Gate,
    Check,
    Lint,
    Review,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Block,
    Ask,
    Warn,
    /// Approve without asking (gate rules on agent tools).
    Allow,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub kind: RuleKind,
    pub level: Level,
    #[serde(default)]
    pub when: Scope,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub action: Option<String>,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Studio,
    Pack(String),
    Project(String),
    Hire(String),
}

impl Layer {
    fn prefix(&self) -> String {
        match self {
            Layer::Studio => "studio".to_string(),
            Layer::Pack(name) => format!("pack/{name}"),
            Layer::Project(name) => format!("project/{name}"),
            Layer::Hire(name) => format!("hire/{name}"),
        }
    }

    pub fn rule_id(&self, name: &str) -> RuleId {
        RuleId::new(format!("{}/{name}", self.prefix()))
    }

    pub fn is_pack(&self) -> bool {
        matches!(self, Layer::Pack(_))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hire {
    pub id: HireId,
    pub name: String,
    pub role: String,
    pub runtime: String,
    pub model: String,
    /// Stronger models to move to, one per failed attempt, staying on the last.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub escalate: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub budget_per_ticket_usd: Option<f64>,
    /// Cap for this hire over any rolling 24 hours.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub daily_usd: Option<f64>,
    #[serde(default)]
    pub tools: Vec<String>,
    /// Give runs the owner's own MCP servers too, not just the studio's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user_mcp: bool,
    pub prompt: String,
}

impl Hire {
    /// The model for a ticket that has had `failed` attempts that didn't pass.
    pub fn model_for(&self, failed: u32) -> &str {
        match failed {
            0 => &self.model,
            n => self
                .escalate
                .get(n as usize - 1)
                .or(self.escalate.last())
                .unwrap_or(&self.model),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("protect rules require when.paths")]
    ProtectNeedsPaths,
    #[error("check rules require command")]
    CheckNeedsCommand,
    #[error("lint rules require pattern")]
    LintNeedsPattern,
    #[error("gate rules require action")]
    GateNeedsAction,
    #[error("guide and review rules require a non-empty why")]
    GuideReviewNeedsWhy,
}

impl Rule {
    pub fn validate(&self) -> Result<(), RuleError> {
        match self.kind {
            RuleKind::Protect if self.when.paths.is_empty() => Err(RuleError::ProtectNeedsPaths),
            RuleKind::Check if self.command.is_none() => Err(RuleError::CheckNeedsCommand),
            RuleKind::Lint if self.pattern.is_none() => Err(RuleError::LintNeedsPattern),
            RuleKind::Gate if self.action.is_none() => Err(RuleError::GateNeedsAction),
            RuleKind::Guide | RuleKind::Review if self.why.trim().is_empty() => {
                Err(RuleError::GuideReviewNeedsWhy)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: RuleKind) -> Rule {
        Rule {
            id: RuleId::new("studio/test"),
            kind,
            level: Level::Block,
            when: Scope {
                projects: vec!["*".to_string()],
                ..Default::default()
            },
            pattern: None,
            command: None,
            action: None,
            why: "because".to_string(),
        }
    }

    #[test]
    fn model_follows_failed_attempts() {
        let mut hire = Hire {
            id: HireId::new("dev"),
            name: "Dev".into(),
            role: "engineer".into(),
            runtime: "claude".into(),
            model: "small".into(),
            escalate: Vec::new(),
            effort: None,
            budget_per_ticket_usd: None,
            daily_usd: None,
            tools: Vec::new(),
            user_mcp: false,
            prompt: String::new(),
        };
        assert_eq!(hire.model_for(0), "small");
        assert_eq!(hire.model_for(5), "small");
        hire.escalate = vec!["mid".into(), "big".into()];
        assert_eq!(hire.model_for(0), "small");
        assert_eq!(hire.model_for(1), "mid");
        assert_eq!(hire.model_for(2), "big");
        assert_eq!(hire.model_for(9), "big");
    }

    #[test]
    fn protect_requires_paths() {
        let mut r = rule(RuleKind::Protect);
        r.when.paths.clear();
        assert_eq!(r.validate(), Err(RuleError::ProtectNeedsPaths));
        r.when.paths.push("*.toml".to_string());
        assert!(r.validate().is_ok());
    }

    #[test]
    fn check_requires_command() {
        let mut r = rule(RuleKind::Check);
        assert_eq!(r.validate(), Err(RuleError::CheckNeedsCommand));
        r.command = Some(vec!["cargo".to_string(), "test".to_string()]);
        assert!(r.validate().is_ok());
    }

    #[test]
    fn lint_requires_pattern() {
        let mut r = rule(RuleKind::Lint);
        assert_eq!(r.validate(), Err(RuleError::LintNeedsPattern));
        r.pattern = Some("unwrap".to_string());
        assert!(r.validate().is_ok());
    }

    #[test]
    fn gate_requires_action() {
        let mut r = rule(RuleKind::Gate);
        assert_eq!(r.validate(), Err(RuleError::GateNeedsAction));
        r.action = Some("merge".to_string());
        assert!(r.validate().is_ok());
    }

    #[test]
    fn guide_and_review_require_why() {
        let mut r = rule(RuleKind::Guide);
        r.why = "   ".to_string();
        assert_eq!(r.validate(), Err(RuleError::GuideReviewNeedsWhy));
        r.why = "do this".to_string();
        assert!(r.validate().is_ok());

        let mut r = rule(RuleKind::Review);
        r.why = "".to_string();
        assert_eq!(r.validate(), Err(RuleError::GuideReviewNeedsWhy));
    }

    #[test]
    fn layer_prefixes() {
        assert_eq!(
            Layer::Studio.rule_id("keep-app-name").to_string(),
            "studio/keep-app-name"
        );
        assert_eq!(
            Layer::Pack("acme".to_string())
                .rule_id("no-unwrap")
                .to_string(),
            "pack/acme/no-unwrap"
        );
        assert_eq!(
            Layer::Project("shop".to_string())
                .rule_id("protect-rules-rs")
                .to_string(),
            "project/shop/protect-rules-rs"
        );
    }
}
