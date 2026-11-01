//! How pages become pictures: a Chromium browser, or macOS Quick Look.

use crate::{Result, TOKEN};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// The longest a page may take to render.
const PAGE_LIMIT: Duration = Duration::from_secs(30);

/// The first Chromium browser found, if any.
pub fn find_browser() -> Option<PathBuf> {
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
pub async fn shoot(
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
pub enum Renderer {
    Browser(PathBuf),
    QuickLook(Inliner),
}

/// Makes a page self-contained for Quick Look, which loads nothing: the
/// stylesheet goes inline, with the fonts in it as data URLs.
pub struct Inliner {
    css: String,
}

impl Inliner {
    pub async fn new(base: &str) -> Result<Inliner> {
        let client = reqwest::Client::new();
        let mut css = client
            .get(format!("{base}/static/app.css"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        for font in [
            "ibm-plex-sans-var",
            "ibm-plex-mono-400",
            "ibm-plex-mono-500",
        ] {
            let path = format!("/static/fonts/{font}.woff2");
            let bytes = client
                .get(format!("{base}{path}"))
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            css = css.replace(&path, &format!("data:font/woff2;base64,{}", base64(&bytes)));
        }
        Ok(Inliner { css })
    }

    /// The page at `url`, with the stylesheet inline and `theme` forced.
    pub async fn page(&self, url: &str, theme: Option<&str>) -> Result<String> {
        let html = reqwest::Client::new()
            .get(url)
            .header("cookie", format!("studio_token={TOKEN}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let start = html
            .find("<link rel=\"stylesheet\"")
            .ok_or("the page has no stylesheet")?;
        let end = start
            + html[start..]
                .find('>')
                .ok_or("the stylesheet link isn't closed")?
            + 1;
        let page = format!(
            "{}<style>{}</style>{}",
            &html[..start],
            self.css,
            &html[end..]
        );
        Ok(match theme {
            Some(theme) => page.replacen("<html", &format!("<html data-theme=\"{theme}\""), 1),
            None => page,
        })
    }
}

/// Standard base64, for the fonts' data URLs.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn attribute_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Renders `page` with Quick Look at a CSS `width`. Quick Look lays pages
/// out in a 1024px square, so the page goes in an iframe of `width` (media
/// queries see that width), scaled down to fit when it is wider. A narrower
/// page is cut to its own width.
pub async fn quick_look(page: &str, width: u32, scratch: &Path, out: &Path) -> Result<()> {
    const SIDE: u32 = 1024;
    let scale = (f64::from(SIDE) / f64::from(width)).min(1.0);
    let height = (f64::from(SIDE) / scale) as u32;
    // A narrow frame sits one pixel in, since `sips` crops from (1, 1) at the least.
    let inset = if width < SIDE { "1px 0 0 1px" } else { "0" };
    let frame = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;background:#2a2a2e;overflow:hidden}}\
         iframe{{border:0;display:block;margin:{inset};width:{width}px;height:{height}px;\
         transform:scale({scale});transform-origin:0 0}}\
         </style></head><body><iframe srcdoc=\"{}\"></iframe></body></html>",
        attribute_escape(page)
    );
    let name = out.file_stem().ok_or("bad output name")?.to_string_lossy();
    let file = scratch.join(format!("{name}.html"));
    std::fs::write(&file, frame)?;
    let status = tokio::process::Command::new("qlmanage")
        .args(["-t", "-s", &SIDE.to_string(), "-o"])
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
                "Quick Look took longer than {}s on {name}",
                PAGE_LIMIT.as_secs()
            )
        })??;
    let made = scratch.join(format!("{name}.html.png"));
    if !status.success() || !made.exists() {
        return Err(format!("Quick Look made no picture of {name}").into());
    }
    if width < SIDE {
        let status = tokio::process::Command::new("sips")
            .args(["-c", &(SIDE - 1).to_string(), &width.to_string()])
            .args(["--cropOffset", "1", "1"])
            .arg(&made)
            .arg("--out")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            return Err(format!("sips could not crop {name}").into());
        }
        std::fs::remove_file(made)?;
    } else {
        std::fs::rename(made, out)?;
    }
    Ok(())
}

pub fn has_quick_look() -> bool {
    Path::new("/usr/bin/qlmanage").exists()
}
