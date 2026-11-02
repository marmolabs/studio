//! Saves a screenshot of every web page, served against real studiods.
//!
//! `cargo run -q -p studio-web --example gallery -- <out-dir>`
//!
//! Writes `<name>.png` at 1024px wide, `<name>-1440.png` and `<name>-phone.png`
//! (390px) for every page, plus a few in the light theme at 1024px and 1440px.
//! Several studiods run in-process in temporary homes with git repositories as
//! projects and a scripted runtime for their hires (see `studios.rs`): a busy
//! company of four projects, each with a lead and in a different phase, with
//! the shop's conversation and every lead's standup written in, whose office
//! is closed for the last picture; an idle one; one with a release that waits;
//! one whose project has no lead; a new studio with nothing in it yet; and one
//! with a project in each state of being set up. Everything on the pages is
//! what the real studiod answers, so every time reads "just now".
//!
//! Uses a Chromium browser (`$STUDIO_BROWSER`, Chrome, Brave, Chromium). When
//! there is none, or it can't run (as in an agent's sandbox), macOS Quick Look
//! renders the pages instead, the way the design references were made: the
//! page goes in an iframe of the target width, scaled to fit 1024px. With
//! neither it prints a line and exits 0.

mod render;
mod studios;

use render::{Inliner, Renderer, find_browser, has_quick_look, quick_look, shoot};
use std::error::Error;
use std::path::Path;
use std::process::ExitCode;
use studio_web::{AppConfig, router};
use studiod::api::{Request, Response};

const TOKEN: &str = "0123abcd";
/// Page widths, by the suffix of their file name.
const WIDTHS: [(&str, u32); 3] = [("", 1024), ("-1440", 1440), ("-phone", 390)];
/// Pages also shown in the light theme (Quick Look only).
const LIGHT: [&str; 4] = ["hq", "lead-shop", "ticket-asking", "hires"];

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Serves the web UI against `live`'s studiod, which listens on its socket;
/// returns the web app's base URL.
async fn serve(live: &studios::Live) -> Result<String> {
    let socket = live.studio.socket_path();
    tokio::spawn(studiod::serve(std::sync::Arc::clone(&live.studio)));
    for _ in 0..500 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let config = AppConfig {
        socket,
        data_dir: live.data_dir.clone(),
        token: TOKEN.into(),
        base_url: base.clone(),
    };
    tokio::spawn(async move { topcoat::serve(listener, router(config)).await });
    Ok(base)
}

async fn run(out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;

    // Nothing to render with: say so before building any studio.
    let browser = find_browser();
    if browser.is_none() && !has_quick_look() {
        println!("no Chromium browser found (set STUDIO_BROWSER); skipping the page gallery");
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let studios = studios::open(dir.path()).await?;
    let ids = &studios.ids;
    let busy = serve(&studios.busy).await?;
    let idle = serve(&studios.idle).await?;
    let empty = serve(&studios.empty).await?;
    let waiting = serve(&studios.waiting).await?;
    let leadless = serve(&studios.leadless).await?;
    let starting = serve(&studios.starting).await?;

    // One profile for every page, so the login cookie carries over (cookies
    // ignore the port). Logging in also shows whether the browser works here.
    let profile = dir.path().join("profile");
    let mut renderer = None;
    if let Some(browser) = browser {
        let login = shoot(
            &browser,
            &profile,
            &format!("{busy}/?token={TOKEN}"),
            (1024, 1024),
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
        None => Renderer::QuickLook(Inliner::new(&busy).await?),
    };

    // (name, server, path)
    let ticket = |id: &str| format!("/tickets/{id}");
    let lead = |project: &str| format!("/leads/{project}");
    let look = |project: &str, id: &str| format!("/leads/{project}?look={id}");
    let pages = [
        // HQ: the morning round-up of a busy, an idle, a waiting and an empty
        // studio.
        ("hq", &busy, "/".to_string()),
        ("hq-idle", &idle, "/".into()),
        ("hq-release-waiting", &waiting, "/".into()),
        ("hq-no-lead", &leadless, "/".into()),
        ("hq-empty", &empty, "/".into()),
        // A conversation for each phase a project can be in: the shop's is
        // written out, and the others hold what the studio itself shows.
        ("lead-shop", &busy, lead("shop")),
        ("lead-editor", &busy, lead("editor")),
        ("lead-admin", &busy, lead("admin")),
        ("lead-docs", &busy, lead("docs")),
        ("lead-idle", &idle, lead("editor")),
        // A project nobody leads: the card that gives it a lead.
        ("lead-no-lead", &leadless, lead("shop")),
        // Look closer: work that is ready, and a goal that is ready.
        ("look-task", &busy, look("shop", &ids.ready)),
        ("look-goal", &busy, look("shop", &ids.ready_goal)),
        ("office", &busy, "/office".into()),
        ("office-shop", &busy, "/office?pick=projects/shop".into()),
        ("new", &busy, "/new".into()),
        ("new-first", &empty, "/new".into()),
        // A project being set up, in each state of the setup.
        ("setup-reading", &starting, lead("wiki")),
        ("setup-checking", &starting, lead("ledger")),
        ("setup-ready", &starting, lead("garden")),
        ("setup-failed", &starting, lead("tracker")),
        ("setup-accepted", &starting, lead("journal")),
        ("studio", &busy, "/studio".into()),
        ("studio-empty", &empty, "/studio".into()),
        ("ticket-working", &busy, ticket(&ids.working)),
        ("ticket-asking", &busy, ticket(&ids.asking)),
        ("ticket-ready", &busy, ticket(&ids.ready)),
        ("ticket-failed", &busy, ticket(&ids.failed)),
        ("ticket-landed-merged", &busy, ticket(&ids.landed_merged)),
        ("ticket-landed", &busy, ticket(&ids.landed)),
        ("hires", &busy, "/hires".into()),
        ("rules", &busy, "/rules".into()),
        (
            "rules-project",
            &busy,
            "/rules?file=protect-payments&from=project/shop".into(),
        ),
        ("hire-edit", &busy, "/hires?file=dev".into()),
        // The busy office, closed by its owner: the last picture, since
        // nothing reopens it.
        ("office-closed", &busy, "/office".into()),
    ];
    for (name, base, path) in pages {
        if name == "office-closed" {
            let closed = studios
                .busy
                .studio
                .handle(Request::SetOpen { open: false })
                .await;
            let Response { ok, error, .. } = closed;
            if !ok {
                return Err(error
                    .unwrap_or_else(|| "the office would not close".into())
                    .into());
            }
        }
        let url = format!("{base}{path}");
        for (suffix, width) in WIDTHS {
            let png = out_dir.join(format!("{name}{suffix}.png"));
            match &renderer {
                Renderer::Browser(browser) => {
                    shoot(browser, &profile, &url, (width, 1024), &png).await?;
                }
                Renderer::QuickLook(inliner) => {
                    let page = inliner.page(&url, None).await?;
                    quick_look(&page, width, dir.path(), &png).await?;
                }
            }
            println!("wrote {}", png.display());
        }
        if let (Renderer::QuickLook(inliner), true) = (&renderer, LIGHT.contains(&name)) {
            let page = inliner.page(&url, Some("light")).await?;
            for (suffix, width) in &WIDTHS[..2] {
                let png = out_dir.join(format!("{name}-light{suffix}.png"));
                quick_look(&page, *width, dir.path(), &png).await?;
                println!("wrote {}", png.display());
            }
        }
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
