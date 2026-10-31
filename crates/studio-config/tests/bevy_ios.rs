//! The open `bevy-ios` pack loads through the same loaders the studio uses.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use studio_config::config::{PackEntry, StudioConfig};
use studio_config::{ProjectConfig, StudioPaths, load_pack_hires, load_rules};

fn pack_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs/bevy-ios")
}

/// A studio.toml that lists the pack, loaded the way the daemon loads it.
fn config() -> StudioConfig {
    let home = tempfile::tempdir().unwrap();
    let text = format!(
        "[budget]\ndaily_usd = 1.0\n\n[concurrency]\nmax_runs = 1\n\n[[packs]]\nname = \"bevy-ios\"\npath = \"{}\"\n",
        pack_dir().display()
    );
    std::fs::write(home.path().join("studio.toml"), text).unwrap();
    let config = StudioConfig::load(&StudioPaths::resolve(Some(home.path().to_path_buf())));
    let config = config.unwrap();
    assert_eq!(
        config.packs,
        [PackEntry {
            name: "bevy-ios".to_string(),
            path: pack_dir()
        }]
    );
    config
}

#[test]
fn the_manifest_is_valid() {
    let manifest = studio_config::pack::load_manifest(&pack_dir()).unwrap();
    assert_eq!(manifest.name, "bevy-ios");
}

#[test]
fn every_hire_loads() {
    let hires: Vec<_> = load_pack_hires(&config())
        .into_iter()
        .map(|(path, hire)| hire.unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect();
    let mut ids: Vec<_> = hires.iter().map(|h| h.id.to_string()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "art-critic",
            "artist",
            "engineer",
            "producer",
            "reviewer",
            "store"
        ]
    );
    assert!(hires.iter().all(|h| h.runtime == "claude-code"));
    let hire = |id: &str| hires.iter().find(|h| h.id.to_string() == id).unwrap();
    assert_eq!(hire("producer").tools, ["file_ticket"]);
    assert_eq!(hire("engineer").escalate, ["opus"]);
    // Only one hire has the reviewer role; critics are called by gates.
    assert_eq!(hires.iter().filter(|h| h.role == "reviewer").count(), 1);
    assert_ne!(hire("engineer").model, hire("reviewer").model);
    assert_ne!(hire("engineer").model, hire("art-critic").model);
}

#[test]
fn every_rule_loads() {
    let home = tempfile::tempdir().unwrap();
    let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
    let loaded = load_rules(&config(), &paths);
    assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    let mut ids: Vec<_> = loaded.rules.iter().map(|(_, r)| r.id.to_string()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "pack/bevy-ios/no-push",
            "pack/bevy-ios/protect-signing",
            "pack/bevy-ios/protect-xcodeproj",
            "pack/bevy-ios/run-the-repos-checks",
        ]
    );
}

#[test]
fn skills_exist() {
    for skill in ["bevy-capture", "ios-release-checklist"] {
        let path = pack_dir().join("skills").join(skill).join("SKILL.md");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.starts_with("---\n"), "{skill}");
        assert!(text.contains(&format!("name: {skill}")));
    }
}

#[test]
fn the_project_template_names_real_hires() {
    let text = std::fs::read_to_string(pack_dir().join("project.toml")).unwrap();
    let project = ProjectConfig::parse(&text).unwrap();
    assert_eq!(project.verify.len(), 3);
    assert_eq!(project.capture.last().map(String::as_str), Some("{out}"));
    let hires: Vec<_> = load_pack_hires(&config())
        .into_iter()
        .filter_map(|(_, h)| h.ok())
        .map(|h| h.id.to_string())
        .collect();
    let names: Vec<_> = project.gates.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["art", "store-copy"]);
    for gate in &project.gates {
        assert!(hires.contains(&gate.hire), "{}", gate.hire);
    }
}
