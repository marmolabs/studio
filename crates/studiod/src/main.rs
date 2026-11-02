use clap::Parser;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use studio_config::StudioPaths;
use studio_runtime::Runtime;
use studiod::{CiWatcher, Studio, serve};

/// The studio daemon. Runs tickets and serves the local API on
/// `<data>/studiod.sock`.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Studio folder (default: $STUDIO_HOME, else the platform config dir).
    #[arg(long)]
    home: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let paths = StudioPaths::resolve(args.home);
    let built = studiod::runtimes::build();
    for (name, runtime) in &built {
        if let Some(problem) = runtime.check_env().problem {
            eprintln!("runtime {name}: {problem}; its hires won't run");
        }
    }
    let runtimes: HashMap<String, Arc<dyn Runtime>> = built.into_iter().collect();

    let studio = match Studio::open(paths, runtimes) {
        Ok(studio) => studio,
        Err(err) => {
            eprintln!("studiod: {err}");
            std::process::exit(1);
        }
    };
    match studio.recover() {
        Ok(0) => {}
        Ok(n) => eprintln!("held {n} ticket(s) that were running before the restart"),
        Err(err) => eprintln!("recovery: {err}"),
    }
    studio.spawn_scheduler();
    studio.spawn_ci_watcher(CiWatcher::new("gh"));
    let pid_file = studio.paths.data_dir.join("studiod.pid");
    if let Err(err) = std::fs::write(&pid_file, std::process::id().to_string()) {
        eprintln!("studiod: can't write {}: {err}", pid_file.display());
    }
    let socket = studio.socket_path();
    eprintln!("studiod listening on {}", socket.display());
    tokio::select! {
        result = serve(Arc::clone(&studio)) => {
            if let Err(err) = result {
                eprintln!("studiod: {err}");
            }
        }
        _ = shutdown_signal() => {}
    }
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(pid_file);
    // Returning drops every run, and each agent process is killed on drop.
    // Without this, a plain `kill` would leave agents running and spending.
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => terminate,
        Err(err) => {
            eprintln!("studiod: can't watch SIGTERM: {err}");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
