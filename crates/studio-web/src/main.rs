use clap::Parser;
use std::path::PathBuf;
use studio_config::StudioPaths;
use studio_web::{AppConfig, generate_token, router};

/// Web UI for a running studio (`studiod`). Listens on 127.0.0.1 unless told otherwise.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Studio folder (default: $STUDIO_HOME, else the platform config dir).
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long, default_value_t = 4870)]
    port: u16,
    /// Address to listen on. Give your LAN address to reach the UI from
    /// other devices; anyone on the network with the link can act as you.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    if let Err(err) = run(args).await {
        eprintln!("studio-web: {err}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let paths = StudioPaths::resolve(args.home);
    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port)).await?;
    let port = listener.local_addr()?.port();
    let config = AppConfig {
        socket: studiod::api::socket_path(&paths.data_dir),
        data_dir: paths.data_dir,
        token: generate_token()?,
        base_url: format!("http://{}:{port}", args.host),
    };
    println!("open {}/?token={}", config.base_url, config.token);
    topcoat::serve(listener, router(config)).await?;
    Ok(())
}
