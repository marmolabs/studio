//! The router: a cheap hire that picks who works on a new ticket.

use crate::quick::{self, Reply};
use crate::studio::{Studio, StudioError, other};
use std::path::Path;
use std::sync::Arc;
use studio_config::ProjectConfig;
use studio_core::rule::Hire;

/// Roles that never take tickets, so the router is never offered them.
const NOT_CANDIDATES: [&str; 3] = ["approver", "router", "reviewer"];

/// Who was picked, and why. `reply` carries the router's spend, for the caller
/// to charge to the new ticket.
pub(crate) struct Routed {
    pub hire: String,
    pub reason: String,
    pub reply: Option<Reply>,
}

/// Hires that may be offered for a ticket, by id. A gate's critic judges work,
/// so it never takes a ticket either.
pub(crate) fn candidates<'a>(hires: &'a [Hire], critics: &[String]) -> Vec<&'a Hire> {
    let mut found: Vec<&Hire> = hires
        .iter()
        .filter(|h| !NOT_CANDIDATES.contains(&h.role.as_str()) && !critics.contains(&h.id.0))
        .collect();
    found.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    found
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

pub(crate) fn prompt(candidates: &[&Hire], title: &str, detail: &str) -> String {
    let list: Vec<String> = candidates
        .iter()
        .map(|h| {
            format!(
                "- {}: {}, role {}, model {}. {}",
                h.id,
                h.name,
                h.role,
                h.model,
                cut(h.prompt.trim(), 300)
            )
        })
        .collect();
    format!(
        "Pick the hire best suited to this ticket.\n\nHires:\n{}\n\n\
         Ticket: {title}\n\n{}\n\n\
         Reply with one or two sentences, then end with exactly one line:\n\
         HIRE: <id>",
        list.join("\n"),
        cut(detail, 2000)
    )
}

/// The id on the last `HIRE:` line, if it is one of the candidates, with the
/// reply's words as the reason.
pub(crate) fn parse(reply: &str, candidates: &[&Hire]) -> Option<(String, String)> {
    let is_hire = |line: &str| {
        line.trim()
            .get(..5)
            .is_some_and(|head| head.eq_ignore_ascii_case("HIRE:"))
    };
    let line = reply.lines().rev().find(|line| is_hire(line))?;
    let id = line.trim()[5..].trim().trim_matches('`');
    let hire = candidates.iter().find(|h| h.id.0 == id)?;
    let reason: Vec<&str> = reply
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !is_hire(l))
        .collect();
    let reason = cut(&reason.join(" "), 200);
    let reason = if reason.is_empty() {
        format!("{} suits it", hire.name)
    } else {
        reason
    };
    Some((hire.id.0.clone(), reason))
}

fn engineer(studio: &Studio) -> Result<String, StudioError> {
    studio
        .first_with_role("engineer")
        .map(|h| h.id.0)
        .ok_or_else(|| other("no hire given and no router or engineer hire to pick one"))
}

/// Picks the hire for a new ticket in `project`.
pub(crate) async fn pick(
    studio: &Arc<Studio>,
    project: &str,
    path: &Path,
    title: &str,
    detail: &str,
) -> Result<Routed, StudioError> {
    let Some(router) = studio.first_with_role("router") else {
        return Ok(Routed {
            hire: engineer(studio)?,
            reason: "no router; the first engineer".to_string(),
            reply: None,
        });
    };
    let hires = studio.all_hires();
    let critics: Vec<String> = ProjectConfig::load(path)
        .ok()
        .flatten()
        .map(|config| config.gates.into_iter().map(|g| g.hire).collect())
        .unwrap_or_default();
    let found = candidates(&hires, &critics);
    let asked = quick::ask(
        studio,
        None,
        project,
        &router,
        path,
        &prompt(&found, title, detail),
    )
    .await;
    match asked {
        Ok(reply) => {
            let picked = match &reply.answer {
                Ok(text) => parse(text, &found)
                    .ok_or("the router named no known hire; the first engineer".to_string()),
                Err(err) => Err(format!("{err}; the first engineer")),
            };
            let (hire, reason) = match picked {
                Ok(picked) => picked,
                Err(reason) => (engineer(studio)?, reason),
            };
            Ok(Routed {
                hire,
                reason,
                reply: Some(reply),
            })
        }
        Err(err) => Ok(Routed {
            hire: engineer(studio)?,
            reason: format!("{err}; the first engineer"),
            reply: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::ids::HireId;

    fn hire(id: &str, role: &str) -> Hire {
        Hire {
            id: HireId::new(id),
            name: id.to_uppercase(),
            role: role.into(),
            runtime: "fake".into(),
            model: "m".into(),
            escalate: Vec::new(),
            effort: None,
            budget_per_ticket_usd: None,
            daily_usd: None,
            tools: Vec::new(),
            user_mcp: false,
            prompt: "You write code.".into(),
        }
    }

    #[test]
    fn system_hires_are_never_candidates() {
        let hires = vec![
            hire("dev", "engineer"),
            hire("approver", "approver"),
            hire("router", "router"),
            hire("critic", "reviewer"),
            hire("art", "artist"),
        ];
        let ids: Vec<&str> = candidates(&hires, &[])
            .iter()
            .map(|h| h.id.0.as_str())
            .collect();
        assert_eq!(ids, ["art", "dev"]);
        let critics = ["art".to_string()];
        let ids: Vec<&str> = candidates(&hires, &critics)
            .iter()
            .map(|h| h.id.0.as_str())
            .collect();
        assert_eq!(ids, ["dev"]);
    }

    #[test]
    fn the_prompt_lists_hires_and_the_ticket() {
        let hires = vec![hire("dev", "engineer")];
        let text = prompt(&candidates(&hires, &[]), "Add a menu", "Escape opens it.");
        assert!(text.contains("- dev: DEV, role engineer, model m. You write code."));
        assert!(text.contains("Add a menu"));
        assert!(text.contains("Escape opens it."));
    }

    #[test]
    fn the_last_hire_line_must_name_a_candidate() {
        let hires = vec![hire("dev", "engineer"), hire("art", "artist")];
        let found = candidates(&hires, &[]);
        assert_eq!(
            parse("It is code work.\nHIRE: dev", &found),
            Some(("dev".into(), "It is code work.".into()))
        );
        assert_eq!(
            parse("HIRE: dev\nActually.\nhire: art", &found).map(|r| r.0),
            Some("art".into())
        );
        assert_eq!(parse("HIRE: nobody", &found), None);
        assert_eq!(parse("I pick dev.", &found), None);
        assert_eq!(parse("HIRE: approver", &found), None);
    }
}
