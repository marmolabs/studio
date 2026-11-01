//! `studio doctor`: checks a studio home, its projects and runtimes without
//! needing the daemon. Each check is a result; printing is the caller's job.

use crate::api::{Request, call, socket_path};
use std::process::Command;
use studio_config::{ProjectConfig, StudioConfig, StudioPaths, load_hires, load_rules};
use studio_git::Repo;

pub struct Check {
    pub ok: bool,
    pub label: String,
    /// What to do about it, when `ok` is false.
    pub fix: String,
}

fn pass(label: impl Into<String>) -> Check {
    Check {
        ok: true,
        label: label.into(),
        fix: String::new(),
    }
}

fn fail(label: impl Into<String>, fix: impl Into<String>) -> Check {
    Check {
        ok: false,
        label: label.into(),
        fix: fix.into(),
    }
}

/// Everything except whether the daemon is up.
pub fn check_home(paths: &StudioPaths) -> Vec<Check> {
    let mut checks = Vec::new();
    let home = paths.config_dir.display();
    if !paths.config_dir.is_dir() {
        checks.push(fail(
            format!("studio home {home} does not exist"),
            "run `studio init`",
        ));
    } else {
        checks.push(pass(format!("studio home {home}")));
    }

    let config = match StudioConfig::load(paths) {
        Ok(config) => {
            checks.push(pass("studio.toml loads"));
            Some(config)
        }
        Err(err) => {
            checks.push(fail(
                format!("studio.toml: {err}"),
                "fix the file, or run `studio init` in an empty home",
            ));
            None
        }
    };

    let hires = load_hires(paths);
    if hires.is_empty() {
        checks.push(fail(
            "no hires found",
            "add a file to hires/ (`studio init` writes three)",
        ));
    }
    for (path, hire) in hires {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        checks.push(match hire {
            Ok(_) => pass(format!("hire {name}")),
            Err(problem) => fail(
                format!("hire {name}: {problem}"),
                format!("fix {}", path.display()),
            ),
        });
    }

    if let Some(config) = &config {
        let loaded = load_rules(config, paths);
        if loaded.problems.is_empty() {
            checks.push(pass(format!("{} rule(s) load", loaded.rules.len())));
        }
        for problem in loaded.problems {
            checks.push(fail(
                format!("rule {}: {}", problem.path.display(), problem.message),
                "fix or remove the file",
            ));
        }
    }

    for (name, runtime) in &crate::runtimes::build() {
        let report = runtime.check_env();
        checks.push(match report.problem {
            None => pass(format!(
                "runtime {name} {}",
                report.version.unwrap_or_default()
            )),
            Some(problem) => fail(
                format!("runtime {name}: {problem}"),
                "install it and sign in, or stop using it in hires",
            ),
        });
    }

    for project in config.iter().flat_map(|c| &c.projects) {
        let name = &project.name;
        let repo = match Repo::open(&project.path) {
            Ok(repo) => {
                checks.push(pass(format!(
                    "project {name}: git repo at {}",
                    project.path.display()
                )));
                repo
            }
            Err(err) => {
                checks.push(fail(
                    format!(
                        "project {name}: {} is not a git repo ({err})",
                        project.path.display()
                    ),
                    "fix `path` in studio.toml",
                ));
                continue;
            }
        };
        if let Some(branch) = &project.integration_branch {
            checks.push(match repo.resolve(branch) {
                Ok(_) => pass(format!("project {name}: branch {branch}")),
                Err(_) => fail(
                    format!("project {name}: branch {branch} does not exist"),
                    "fix `integration_branch` in studio.toml, or create the branch",
                ),
            });
        }
        checks.push(match ProjectConfig::load(&project.path) {
            Ok(_) => pass(format!("project {name}: .studio.toml")),
            Err(err) => fail(
                format!("project {name}: .studio.toml: {err}"),
                "fix the file",
            ),
        });
    }

    if config.iter().flat_map(|c| &c.projects).any(|p| p.watch_ci) {
        checks.extend(check_gh());
    }
    checks
}

fn check_gh() -> Vec<Check> {
    let fix = "install the GitHub CLI and run `gh auth login`";
    let ran = |args: &[&str]| {
        Command::new("gh")
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !ran(&["--version"]) {
        return vec![fail("gh is not installed (needed by watch_ci)", fix)];
    }
    let mut checks = vec![pass("gh installed")];
    if !ran(&["auth", "status"]) {
        checks.push(fail("gh is not logged in", fix));
    }
    checks
}

/// Whether studiod answers on its socket.
pub fn check_daemon(paths: &StudioPaths) -> Check {
    let socket = socket_path(&paths.data_dir);
    match call(&socket, &Request::Status) {
        Ok(r) if r.ok => pass(format!("studiod answers on {}", socket.display())),
        _ => fail(
            format!("studiod is not reachable on {}", socket.display()),
            "start it with `studio up`",
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    fn home(project: &Path, branch: &str, hire: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("studio.toml"),
            format!(
                "[budget]\ndaily_usd = 5.0\n\n[concurrency]\nmax_runs = 1\n\n\
                 [[projects]]\nname = \"game\"\npath = \"{}\"\nintegration_branch = \"{branch}\"\n",
                project.display()
            ),
        )
        .unwrap();
        fs::create_dir_all(home.path().join("hires")).unwrap();
        fs::write(home.path().join("hires/dev.md"), hire).unwrap();
        home
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "master"]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=a",
                "-c",
                "user.email=a@b",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        dir
    }

    const HIRE: &str = "---\nname: Dev\nrole: engineer\nruntime: fake\nmodel: m\n---\nCode.\n";

    // The claude-code runtime depends on this machine, so leave it out.
    fn failures(checks: &[Check]) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| !c.ok && !c.label.starts_with("runtime "))
            .map(|c| c.label.as_str())
            .collect()
    }

    #[test]
    fn a_good_home_passes() {
        let project = repo();
        let home = home(project.path(), "master", HIRE);
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        let checks = check_home(&paths);
        assert!(failures(&checks).is_empty(), "{:?}", failures(&checks));
        assert!(checks.iter().any(|c| c.label == "hire dev"));
        assert!(checks.iter().any(|c| c.label.contains("branch master")));
    }

    #[test]
    fn a_broken_home_says_what_is_wrong() {
        let project = repo();
        let home = home(project.path(), "nope", "no frontmatter");
        fs::write(project.path().join(".studio.toml"), "verify = 3 3").unwrap();
        let paths = StudioPaths::resolve(Some(home.path().to_path_buf()));
        let bad = failures(&check_home(&paths)).join("\n");
        assert!(bad.contains("hire dev"), "{bad}");
        assert!(bad.contains("branch nope"), "{bad}");
        assert!(bad.contains(".studio.toml"), "{bad}");

        let empty = tempfile::tempdir().unwrap();
        let paths = StudioPaths::resolve(Some(empty.path().to_path_buf()));
        let bad = failures(&check_home(&paths)).join("\n");
        assert!(bad.contains("studio.toml"), "{bad}");
        assert!(bad.contains("no hires"), "{bad}");
    }
}
