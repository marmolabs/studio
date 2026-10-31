//! `studio up` and `studio down`: start the daemon in the background and the
//! web UI in the foreground, or stop the daemon.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use studio_config::StudioPaths;
use studiod::api::{Request, call};

/// studiod writes its pid here, so `studio down` can find it.
const PID_FILE: &str = "studiod.pid";

/// Starts studiod unless it is running, then runs studio-web until Ctrl-C.
/// The daemon is left running, so agents keep working with the page closed.
pub fn up(
    paths: &StudioPaths,
    home: Option<&Path>,
    host: &str,
    port: u16,
    open: bool,
    new_token: bool,
) -> Result<(), String> {
    if !paths.config_dir.join("studio.toml").exists() {
        return Err("no studio.toml yet: run `studio init` first".into());
    }
    let socket = studiod::api::socket_path(&paths.data_dir);
    if running(&socket) {
        println!("studiod is already running");
    } else {
        start_daemon(paths, home)?;
        wait_for(&socket)?;
        println!(
            "studiod started; its log is {}",
            paths.data_dir.join("studiod.log").display()
        );
    }

    let mut web = Command::new(sibling("studio-web"));
    web.arg("--port")
        .arg(port.to_string())
        .arg("--host")
        .arg(host);
    if let Some(home) = home {
        web.arg("--home").arg(home);
    }
    if new_token {
        web.arg("--new-token");
    }
    let mut child = web
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't start studio-web: {e}"))?;
    let stdout = child.stdout.take().ok_or("studio-web gave no output")?;
    let mut lines = BufReader::new(stdout).lines();
    // studio-web's first line is `open <url>`.
    match lines.next() {
        Some(Ok(line)) => {
            let url = line.trim_start_matches("open ").trim().to_string();
            println!("web UI: {url}");
            if open {
                open_browser(&url);
            }
        }
        _ => {
            let status = child.wait().map_err(|e| e.to_string())?;
            return Err(format!(
                "studio-web stopped ({status}); is port {port} in use?"
            ));
        }
    }
    println!("Ctrl-C stops the web UI; studiod keeps running (`studio down` stops it)");
    for line in lines.map_while(Result::ok) {
        println!("{line}");
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

/// Stops studiod. Running agents are stopped with it, and their tickets are
/// held until you resume them.
pub fn down(paths: &StudioPaths) -> Result<(), String> {
    let socket = studiod::api::socket_path(&paths.data_dir);
    let pid_file = paths.data_dir.join(PID_FILE);
    let pid = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    let Some(pid) = pid.filter(|pid| is_studiod(*pid)) else {
        if running(&socket) {
            return Err("studiod is running but wrote no pid file; stop it yourself".into());
        }
        println!("studiod is not running");
        return Ok(());
    };
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .map_err(|e| format!("can't run kill: {e}"))?;
    if !status.success() {
        return Err(format!("kill {pid} failed"));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(&socket) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("studiod stopped");
    Ok(())
}

fn running(socket: &Path) -> bool {
    call(socket, &Request::Status).is_ok_and(|r| r.ok)
}

fn start_daemon(paths: &StudioPaths, home: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(&paths.data_dir).map_err(|e| e.to_string())?;
    let log_path = paths.data_dir.join("studiod.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("can't open {}: {e}", log_path.display()))?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut daemon = Command::new(sibling("studiod"));
    if let Some(home) = home {
        daemon.arg("--home").arg(home);
    }
    daemon
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        // Its own process group, so Ctrl-C in this terminal doesn't reach it.
        .process_group(0)
        .spawn()
        .map_err(|e| format!("can't start studiod: {e}"))?;
    Ok(())
}

fn wait_for(socket: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if running(socket) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("studiod didn't start in 10s; see studiod.log in the data folder".into())
}

/// A program installed next to this one, else whatever `PATH` finds.
fn sibling(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// The pid is studiod's, not a recycled number now used by something else.
fn is_studiod(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .ends_with("studiod")
        })
}

fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // The link carries the session token: hand it over as an argument, never
    // through a shell.
    if Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_err()
    {
        println!("open the link above in your browser");
    }
}
