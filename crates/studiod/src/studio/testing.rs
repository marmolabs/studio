//! A studio to try things on, for the unit tests of the parts of `studio`.

#![allow(clippy::unwrap_used)]

use super::Studio;
use std::collections::HashMap;
use std::sync::Arc;
use studio_config::StudioPaths;

/// A studio with these projects and no runtimes, in a folder of its own that
/// goes when the returned handle is dropped. The projects' folders are not
/// there: they are only named in `studio.toml`.
pub(super) fn studio(projects: &[&str]) -> (tempfile::TempDir, Arc<Studio>) {
    let home = tempfile::tempdir().unwrap();
    let mut text = "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 2\n".to_string();
    for name in projects {
        text.push_str(&format!(
            "\n[[projects]]\nname = \"{name}\"\npath = \"{}\"\n",
            home.path().join(name).display()
        ));
    }
    std::fs::write(home.path().join("studio.toml"), text).unwrap();
    let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
    let studio = Studio::open(paths, HashMap::new()).unwrap();
    (home, studio)
}
