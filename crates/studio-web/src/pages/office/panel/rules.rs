//! The Rules tab: the rules in force, each linking to its editor.

use serde::Deserialize;
use studiod::api::RuleFile;
use topcoat::view::{View, component, view};

use super::{quiet, section};
use crate::summary::truncate;
use crate::ui;

/// A rule's kind and level, from its front matter.
#[derive(Debug, Default, Deserialize, PartialEq)]
struct RuleHead {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    level: String,
}

fn rule_head(text: &str) -> RuleHead {
    studio_config::frontmatter::split(text)
        .ok()
        .flatten()
        .and_then(|(yaml, _)| serde_yaml_ng::from_str(yaml).ok())
        .unwrap_or_default()
}

/// The first paragraph of a rule after its front matter, as one line.
fn rule_about(text: &str) -> String {
    let body = studio_config::frontmatter::split(text)
        .ok()
        .flatten()
        .map_or(text, |(_, body)| body);
    let paragraph: Vec<&str> = body
        .lines()
        .map(str::trim)
        .skip_while(|l| l.is_empty())
        .take_while(|l| !l.is_empty())
        .collect();
    truncate(&paragraph.join(" "), 140)
}

/// The link to a rule in its editor.
fn rule_href(rule: &RuleFile) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("file", &rule.name);
    if rule.origin != "studio" {
        query.append_pair("from", &format!("{}/{}", rule.origin, rule.owner));
    }
    format!("/rules?{}", query.finish())
}

/// The chip colour for a rule's level.
fn level_tone(level: &str) -> &'static str {
    match level {
        "allow" => "chip ok",
        "ask" | "warn" => "chip needs",
        "block" => "chip bad",
        _ => "chip",
    }
}

#[component]
pub(super) async fn rules_pane(rules: Vec<RuleFile>) -> topcoat::Result<impl View> {
    Ok(view! {
        section(label: "House rules", note: "agents follow these",
            rule_cards(rules: rules)
        )
        new_rule()
    })
}

#[component]
pub(super) async fn new_rule() -> topcoat::Result<impl View> {
    Ok(view! {
        <div>
            <a href="/rules?new" class=(ui::BUTTON)>
                ui::icon(name: "plus")
                "New rule"
            </a>
        </div>
    })
}

#[component]
pub(super) async fn rule_cards(rules: Vec<RuleFile>) -> topcoat::Result<impl View> {
    let rows: Vec<(String, RuleHead, String, String, String, bool)> = rules
        .iter()
        .map(|r| {
            let owner = match r.origin.as_str() {
                "studio" => String::new(),
                origin => format!("{origin} {}", r.owner),
            };
            (
                r.name.clone(),
                rule_head(&r.text),
                rule_about(&r.text),
                rule_href(r),
                owner,
                r.disabled,
            )
        })
        .collect();
    Ok(view! {
        if rows.is_empty() {
            quiet(text: "No rules yet.")
        }
        for (name, head, about, href, owner, off) in rows {
            <a href=(href) class="card text-fg no-underline hover:border-line-2">
                <div class="flex flex-wrap items-center gap-1.5">
                    <b class="mr-auto min-w-0 truncate text-[13px] font-semibold">(name)</b>
                    if !head.kind.is_empty() {
                        <span class="chip">(head.kind.clone())</span>
                    }
                    if !head.level.is_empty() {
                        <span class=(level_tone(&head.level))>(head.level.clone())</span>
                    }
                    if off {
                        <span class="chip held">"off"</span>
                    }
                </div>
                if !owner.is_empty() {
                    <span class="text-[12px] text-fg-3">(owner)</span>
                }
                if !about.is_empty() {
                    <p class="m-0 text-[12.5px] leading-[18px] text-fg-2">(about)</p>
                }
            </a>
        }
    })
}

#[cfg(test)]
mod tests {

    use crate::pages::office::panel::fixtures::{extra, html_of, office};
    use crate::probe::{find, with};

    #[tokio::test]
    async fn rules_link_to_their_editor() {
        let html = html_of(office(), extra(), "").await;
        let card = find(&html, "href", "/rules?file=no-push").text();
        assert_eq!(card, "no-push gate block Agents never push.", "{html}");
        assert!(!with(&html, "href", "/rules?new").is_empty(), "{html}");
    }
}
