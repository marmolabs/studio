//! Saves a screenshot of every web page, served against a stand-in studiod
//! full of sample data.
//!
//! `cargo run -q -p studio-web --example gallery -- <out-dir>`
//!
//! Uses a Chromium browser (`$STUDIO_BROWSER`, Chrome, Brave, Chromium). When
//! there is none, or it can't run (as in an agent's sandbox), macOS Quick Look
//! renders the desktop pages instead, without the phone sizes. With neither it
//! prints a line and exits 0.

use serde_json::{Value, json};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::time::Duration;
use studio_web::{AppConfig, router};
use studiod::api::{FileKind, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TOKEN: &str = "0123abcd";
const DESKTOP: (u32, u32) = (1400, 1000);
const PHONE: (u32, u32) = (390, 844);
const PAGE_LIMIT: Duration = Duration::from_secs(30);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A ticket with the fields every ticket has, plus any `extra` ones.
fn ticket(id: &str, title: &str, hire: &str, state: Value, extra: Value) -> Value {
    let project = if id.starts_with("EDIT") {
        "editor"
    } else {
        "game"
    };
    let mut ticket = json!({
        "id": id, "project": project, "title": title, "goal": "", "hire": hire,
        "state": state, "attempts": 1, "base_branch": "main",
        "created_at": now_ms() - 3_600_000,
    });
    if let (Some(ticket), Some(extra)) = (ticket.as_object_mut(), extra.as_object()) {
        ticket.extend(extra.clone());
    }
    ticket
}

fn tickets() -> Value {
    let child = json!({"parent": "EDIT-1"});
    json!([
        ticket(
            "GAME-1",
            "Add a pause menu",
            "dev",
            json!("queued"),
            json!({"attempts": 0})
        ),
        ticket(
            "GAME-2",
            "Fix the jump buffer",
            "dev",
            json!("working"),
            json!({"goal": "Polish the player movement feel"})
        ),
        ticket(
            "GAME-3",
            "Spawn enemies in waves",
            "dev",
            json!({"asking": {"ask": "ASK-1"}}),
            json!({})
        ),
        ticket(
            "GAME-4",
            "Tune the coyote time",
            "dev",
            json!({"asking": {"ask": "ASK-2"}}),
            json!({})
        ),
        ticket(
            "GAME-5",
            "Shake the screen on a hit",
            "dev",
            json!("checking"),
            json!({})
        ),
        ticket(
            "GAME-6",
            "Save the high score to disk",
            "dev",
            json!("reviewing"),
            json!({})
        ),
        ticket(
            "GAME-7",
            "Add a title screen",
            "dev",
            json!("ready"),
            json!({})
        ),
        ticket(
            "GAME-8",
            "Fix the player sprite flicker",
            "dev",
            json!("landed"),
            json!({})
        ),
        ticket(
            "GAME-9",
            "Load the tilemap faster",
            "dev",
            json!({"failed": {"class": "checks", "detail": "cargo clippy: 2 warnings"}}),
            json!({"attempts": 3})
        ),
        ticket(
            "GAME-10",
            "Add gamepad support",
            "dev",
            json!({"held": {"reason": "budget"}}),
            json!({})
        ),
        ticket(
            "EDIT-1",
            "Plan: ship the level editor",
            "producer",
            json!("working"),
            json!({"goal": "Ship a level editor: place tiles, undo mistakes, save levels",
                   "kind": "plan"})
        ),
        ticket(
            "EDIT-2",
            "Add a tile palette",
            "dev",
            json!("landed"),
            child.clone()
        ),
        ticket(
            "EDIT-3",
            "Add undo and redo",
            "dev",
            json!("queued"),
            child.clone()
        ),
        ticket(
            "EDIT-4",
            "Save levels as RON files",
            "artist",
            json!("queued"),
            child
        ),
        ticket(
            "GAME-11",
            "Draw the title logo",
            "artist",
            json!("landed"),
            json!({"parent": "GAME-14"})
        ),
        ticket(
            "GAME-12",
            "Paint the forest tileset",
            "artist",
            json!({"failed": {"class": "review", "detail": "palette too wide"}}),
            json!({"parent": "GAME-14"})
        ),
        ticket(
            "GAME-13",
            "Mix the boss theme",
            "composer",
            json!({"asking": {"ask": "ASK-3"}}),
            json!({"parent": "GAME-14"})
        ),
        ticket(
            "GAME-14",
            "Plan: give the game its look and sound",
            "producer",
            json!("landed"),
            json!({"goal": "Give the game its look and sound: title logo, forest tiles, boss music",
                   "kind": "plan"})
        ),
        ticket(
            "GAME-15",
            "Plan: add a settings screen",
            "producer",
            json!("working"),
            json!({"goal": "Add a settings screen for volume and key bindings", "kind": "plan"})
        ),
    ])
}

fn hire_file(name: &str, role: &str, model: &str, budget: f64, tools: &str) -> Value {
    json!({
        "name": name, "problem": null,
        "text": format!(
            "---\nname: {name}\nrole: {role}\nruntime: claude-code\nmodel: {model}\nescalate: [opus]\neffort: high\nbudget_per_ticket_usd: {budget}\ntools: [{tools}]\n---\nWork in small steps and keep the build green.\n"
        ),
    })
}

fn reply(request: &Request) -> Value {
    let at = |minutes_ago: u64| now_ms() - minutes_ago * 60_000;
    match request {
        Request::Status => json!({
            "asks": 3, "running": 3, "daily_usd": 10.0, "spent_24h": 6.4,
            "projects": ["game", "editor"], "hires": ["dev", "artist", "producer"],
        }),
        Request::Tickets => tickets(),
        Request::Events {
            ticket: Some(id), ..
        } => json!([
            {"seq": 1, "ticket_id": id, "at_ms": at(42), "event": {"type": "claimed"}},
            {"seq": 2, "ticket_id": id, "at_ms": at(30), "event": {"type": "checks_started"}},
            {"seq": 3, "ticket_id": id, "at_ms": at(24),
             "event": {"type": "checks_failed", "detail": "cargo test: 1 failed"}},
            {"seq": 4, "ticket_id": id, "at_ms": at(9), "event": {"type": "checks_passed"}},
        ]),
        Request::Estimates => json!([
            {"ticket": "GAME-2", "quote_usd": 0.38, "eta_ms": now_ms() + 9 * 60_000},
            {"ticket": "GAME-3", "quote_usd": 0.27, "eta_ms": now_ms() + 11 * 60_000},
            {"ticket": "GAME-1", "quote_usd": 0.42, "eta_ms": now_ms() + 14 * 60_000},
            {"ticket": "EDIT-3", "quote_usd": 0.65, "eta_ms": now_ms() + 22 * 60_000},
            {"ticket": "EDIT-4", "quote_usd": 0.30, "eta_ms": now_ms() + 31 * 60_000},
        ]),
        Request::Diff { .. } => json!(
            "diff --git a/src/title.rs b/src/title.rs\n--- a/src/title.rs\n+++ b/src/title.rs\n@@ -4,7 +4,11 @@ pub fn draw_title(ui: &mut Ui) {\n     ui.heading(\"Starlight Run\");\n-    ui.label(\"Press any key\");\n+    if ui.button(\"Start\").clicked() {\n+        next.set(GameState::Playing);\n+    }\n+    if ui.button(\"Quit\").clicked() {\n+        exit.send(AppExit);\n+    }\n }\n"
        ),
        Request::Diffstat { .. } => json!(
            " src/title.rs | 6 +++++-\n src/main.rs  | 1 +\n 2 files changed, 6 insertions(+), 1 deletion(-)\n"
        ),
        // Nothing is booked for a run that has only just started.
        Request::TicketSpend { ticket } if matches!(ticket.as_str(), "GAME-2" | "GAME-3") => {
            json!(0.0)
        }
        Request::TicketSpend { .. } => json!(0.31),
        Request::TicketSpends => json!({
            "GAME-11": 0.31, "GAME-12": 0.84, "GAME-13": 0.22, "GAME-14": 0.12, "GAME-15": 0.09,
        }),
        Request::Asks => json!([{
            "id": "ASK-1", "ticket": "GAME-3", "hire": "dev", "kind": "permission",
            "tool": "Bash",
            "input": {"command": "cargo run --example waves", "description": "Try the wave spawner"},
            "reasons": ["no rule allows it"],
        }, {
            "id": "ASK-2", "ticket": "GAME-4", "hire": "dev", "kind": "permission",
            "tool": "Edit",
            "input": {"file_path": "src/player/movement.rs",
                      "old_string": "const COYOTE_TIME: f32 = 0.08;",
                      "new_string": "const COYOTE_TIME: f32 = 0.12;"},
            "reasons": ["src/player is protected"],
        }, {
            "id": "ASK-3", "ticket": "GAME-13", "hire": "composer", "kind": "permission",
            "tool": "Bash",
            "input": {"command": "ffmpeg -i assets/music/boss_theme_raw.wav -af loudnorm=I=-16:TP=-1.5:LRA=11 -c:a libvorbis -q:a 5 assets/music/boss_theme.ogg"},
            "reasons": ["no rule allows it"],
        }]),
        Request::Budget => json!({
            "limits": {"daily_usd": 10.0, "ticket_usd": 2.0, "run_usd": null, "min_run_usd": 0.05,
                       "projects": {"game": 6.0, "editor": 4.0}, "hires": {"dev": 8.0, "artist": 2.0}},
            "spent_24h": {"total": 6.4, "by_project": {"game": 4.2, "editor": 1.5, "assets": 0.7},
                          "by_hire": {"dev": 4.6, "artist": 0.6, "producer": 0.6, "reviewer": 0.6}},
            "runs_24h": 17,
            "reserved": {"GAME-2": 0.5, "GAME-5": 0.4, "EDIT-1": 0.25},
        }),
        Request::Files {
            kind: FileKind::Hires,
        } => json!([
            hire_file("dev", "engineer", "sonnet", 0.5, "file_ticket"),
            hire_file("artist", "pixel artist", "haiku", 0.3, ""),
            hire_file(
                "producer",
                "producer",
                "opus",
                1.0,
                "file_ticket, ask_owner"
            ),
            hire_file("tester", "qa", "haiku", 0.2, ""),
            hire_file("reviewer", "reviewer", "sonnet", 0.2, ""),
            {"name": "composer", "problem": "line 3: did not find expected key", "text": ""},
        ]),
        Request::Files { .. } => json!([
            {"name": "protect-player", "problem": null,
             "text": "---\nkind: gate\nlevel: ask\naction: \"edit:src/player/**\"\nwhen:\n  projects: [game]\n---\nMovement feel is tuned by hand: ask before changing it.\n"},
            {"name": "allow-cargo-test", "problem": null,
             "text": "---\nkind: gate\nlevel: allow\naction: \"bash:cargo test\"\n---\nTests are always safe to run.\n"},
            {"name": "small-commits", "problem": null,
             "text": "---\nkind: guide\nlevel: warn\n---\nKeep each ticket to one topic and a small diff.\n"},
        ]),
        _ => Value::Null,
    }
}

/// A studiod stand-in on a Unix socket: one JSON request line, one reply.
fn fake_studiod(socket: &Path) -> Result<()> {
    let listener = UnixListener::bind(socket)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                if BufReader::new(read).read_line(&mut line).await.is_err() {
                    return;
                }
                let Ok(request) = serde_json::from_str::<Request>(&line) else {
                    return;
                };
                let Ok(mut out) = serde_json::to_string(&Response::ok(reply(&request))) else {
                    return;
                };
                out.push('\n');
                let _ = write.write_all(out.as_bytes()).await;
            });
        }
    });
    Ok(())
}

/// Run logs for the tickets whose pages are photographed.
fn write_logs(runs: &Path) -> Result<()> {
    std::fs::create_dir_all(runs)?;
    let lines = |entries: &[Value]| {
        let mut text = entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        text
    };
    let text = |text: &str| json!({"event": {"type": "text", "text": text}});
    let call = |name: &str, input: Value| json!({"event": {"type": "tool_call", "id": "t", "name": name, "input": input}});
    let fin = json!({"event": {"type": "finished", "outcome": {"outcome": "success"},
                               "cost": {"usd": 0.31}}});
    // A long run: the agent's words between bursts of the same few tools.
    let rule = json!({"note": "allowed Edit: inside the worktree"});
    let read = |file: &str| {
        call(
            "Read",
            json!({"file_path": format!("src/player/{file}.rs")}),
        )
    };
    let edit = |file: &str| {
        call(
            "Edit",
            json!({"file_path": format!("src/player/{file}.rs")}),
        )
    };
    let bash = |command: &str| call("Bash", json!({"command": command}));
    let mut long = vec![
        text(
            "The jump buffer drops inputs pressed just before landing. Let me look at the player controller.",
        ),
        read("jump"),
        read("input"),
        read("state"),
        read("movement"),
        call("Grep", json!({"pattern": "jump_buffer"})),
        call("Grep", json!({"pattern": "COYOTE_TIME"})),
        text(
            "The timer resets every frame while the key is held. I'll store the press time instead.",
        ),
        edit("jump"),
        rule.clone(),
        edit("input"),
        rule.clone(),
        edit("state"),
        rule.clone(),
        bash("cargo test jump"),
        bash(
            "cargo test --workspace --locked -- --test-threads=1 --nocapture player::jump::buffered_press_fires_after_landing",
        ),
        text("Two tests fail: the buffer is cleared before the landing check runs."),
        read("jump"),
        read("movement"),
        edit("jump"),
        rule.clone(),
        bash("cargo test jump"),
        text("Buffered jumps now fire up to 120ms after the press."),
        call("Glob", json!({"pattern": "tests/**/*.rs"})),
        read("jump"),
        edit("jump"),
        rule.clone(),
        edit("state"),
        rule.clone(),
        bash("cargo fmt --all"),
        bash("cargo clippy --workspace --all-targets -- -D warnings"),
        text("Formatting and lints are clean. Running the full test suite."),
        read("input"),
        read("jump"),
        bash("cargo test --workspace"),
    ];
    long.push(bash("cargo test --workspace --release"));
    long.push(json!({"event": {"type": "permission", "tool": "Bash"}}));
    long.push(json!({"note": "allowed Bash: cargo test (rule allow-cargo-test)"}));
    std::fs::write(runs.join("GAME-2.jsonl"), lines(&long))?;
    std::fs::write(
        runs.join("GAME-3.jsonl"),
        lines(&[
            text("Waves need a spawner resource and a timer. I'll add both, then try them out."),
            call("Write", json!({"file_path": "src/enemies/waves.rs"})),
            call("Edit", json!({"file_path": "src/main.rs"})),
            json!({"event": {"type": "permission", "tool": "Bash"}}),
        ]),
    )?;
    std::fs::write(
        runs.join("GAME-7.jsonl"),
        lines(&[
            text("Adding a title screen with Start and Quit buttons."),
            call("Write", json!({"file_path": "src/title.rs"})),
            call("Edit", json!({"file_path": "src/main.rs"})),
            call("Bash", json!({"command": "cargo test --workspace"})),
            json!({"note": "checks passed"}),
            json!({"note": "review passed"}),
            fin,
        ]),
    )?;
    Ok(())
}

/// The first Chromium browser found, if any.
fn find_browser() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("STUDIO_BROWSER").filter(|p| !p.is_empty()) {
        return Some(path.into());
    }
    let apps = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    if let Some(app) = apps.iter().map(PathBuf::from).find(|p| p.exists()) {
        return Some(app);
    }
    let path = std::env::var_os("PATH")?;
    ["google-chrome", "chromium", "chromium-browser"]
        .iter()
        .flat_map(|name| std::env::split_paths(&path).map(move |dir| dir.join(name)))
        .find(|p| p.is_file())
}

/// Loads `url` in the browser and saves a screenshot of it. Pages keep a
/// live connection open, so a headless browser writes the screenshot but
/// may never exit: it is stopped once the file has stopped growing.
async fn shoot(
    browser: &Path,
    profile: &Path,
    url: &str,
    size: (u32, u32),
    out: &Path,
) -> Result<()> {
    // An old file must not pass for a new screenshot.
    let _ = std::fs::remove_file(out);
    let mut child = tokio::process::Command::new(browser)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--hide-scrollbars",
            "--timeout=4000",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={},{}", size.0, size.1))
        .arg(format!("--screenshot={}", out.display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", browser.display()))?;
    let size_of = || std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    let started = std::time::Instant::now();
    let mut last = 0;
    while started.elapsed() < PAGE_LIMIT {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(format!("the browser failed on {url}: {status}").into());
            }
            if size_of() == 0 {
                return Err(format!("the browser wrote no screenshot of {url}").into());
            }
            return Ok(());
        }
        let now = size_of();
        if now > 0 && now == last {
            let _ = child.kill().await;
            return Ok(());
        }
        last = now;
    }
    let _ = child.kill().await;
    Err(format!("{url} took longer than {}s", PAGE_LIMIT.as_secs()).into())
}

/// How pages become pictures.
enum Renderer {
    Browser(PathBuf),
    QuickLook,
}

/// Renders `url` with Quick Look: fetches the page, inlines the stylesheet
/// (Quick Look loads nothing) and thumbnails the HTML at desktop width.
async fn quick_look(base: &str, path: &str, scratch: &Path, out: &Path) -> Result<()> {
    let client = reqwest::Client::new();
    let get = |url: String| {
        client
            .get(url)
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
    };
    let html = get(format!("{base}{path}"))
        .await?
        .error_for_status()?
        .text()
        .await?;
    let css = get(format!("{base}/static/app.css")).await?.text().await?;
    let start = html
        .find("<link rel=\"stylesheet\"")
        .ok_or("the page has no stylesheet")?;
    let end = start
        + html[start..]
            .find('>')
            .ok_or("the stylesheet link isn't closed")?
        + 1;
    let page = format!("{}<style>{css}</style>{}", &html[..start], &html[end..]);
    let name = out.file_stem().ok_or("bad output name")?.to_string_lossy();
    let file = scratch.join(format!("{name}.html"));
    std::fs::write(&file, page)?;
    let status = tokio::process::Command::new("qlmanage")
        .args(["-t", "-s", &DESKTOP.0.to_string(), "-o"])
        .arg(scratch)
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status();
    let status = tokio::time::timeout(PAGE_LIMIT, status)
        .await
        .map_err(|_| {
            format!(
                "Quick Look took longer than {}s on {path}",
                PAGE_LIMIT.as_secs()
            )
        })??;
    let made = scratch.join(format!("{name}.html.png"));
    if !status.success() || !made.exists() {
        return Err(format!("Quick Look made no picture of {path}").into());
    }
    std::fs::rename(made, out)?;
    Ok(())
}

fn has_quick_look() -> bool {
    Path::new("/usr/bin/qlmanage").exists()
}

async fn run(out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;

    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("s.sock");
    fake_studiod(&socket)?;
    write_logs(&dir.path().join("runs"))?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let config = AppConfig {
        socket,
        data_dir: dir.path().to_path_buf(),
        token: TOKEN.into(),
        base_url: base.clone(),
    };
    tokio::spawn(async move { topcoat::serve(listener, router(config)).await });

    // One profile for every page, so the login cookie carries over. Logging
    // in also shows whether the browser works here at all.
    let profile = dir.path().join("profile");
    let mut renderer = None;
    if let Some(browser) = find_browser() {
        let login = shoot(
            &browser,
            &profile,
            &format!("{base}/?token={TOKEN}"),
            DESKTOP,
            &dir.path().join("login.png"),
        )
        .await;
        match login {
            Ok(()) => renderer = Some(Renderer::Browser(browser)),
            Err(e) if has_quick_look() => println!("{e}; using Quick Look"),
            Err(e) => return Err(e),
        }
    }
    let renderer = match renderer {
        Some(r) => r,
        None if has_quick_look() => Renderer::QuickLook,
        None => {
            println!("no Chromium browser found (set STUDIO_BROWSER); skipping the page gallery");
            return Ok(());
        }
    };

    // (name, path, also at phone size)
    let pages = [
        ("board", "/", true),
        ("ticket-working", "/tickets/GAME-2", true),
        ("ticket-asking", "/tickets/GAME-3", true),
        ("ticket-ready", "/tickets/GAME-7", false),
        ("asks", "/asks", true),
        ("goals", "/goals", false),
        ("budget", "/budget", false),
        ("hires", "/hires", false),
        ("rules", "/rules?file=protect-player", false),
        ("office", "/office", false),
    ];
    for (name, path, phone) in pages {
        let png = out_dir.join(format!("{name}.png"));
        match &renderer {
            Renderer::Browser(browser) => {
                shoot(browser, &profile, &format!("{base}{path}"), DESKTOP, &png).await?;
                if phone {
                    let small = out_dir.join(format!("{name}-phone.png"));
                    shoot(browser, &profile, &format!("{base}{path}"), PHONE, &small).await?;
                    println!("wrote {}", small.display());
                }
            }
            Renderer::QuickLook => quick_look(&base, path, dir.path(), &png).await?,
        }
        println!("wrote {}", png.display());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let Some(out_dir) = std::env::args_os().nth(1) else {
        eprintln!("usage: gallery <out-dir>");
        return ExitCode::FAILURE;
    };
    match run(Path::new(&out_dir)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gallery: {e}");
            ExitCode::FAILURE
        }
    }
}
