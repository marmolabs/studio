//! The stylesheet, Datastar, the fonts and the icons, compiled into the
//! binary so `cargo install` is enough: no asset bundle to ship next to it.

use topcoat::router::{Body, response::Response, route};

pub const PREFIX: &str = "/static/";
pub const STYLESHEET: &str = "/static/app.css";
pub const DATASTAR: &str = "/static/datastar-1.0.2.js";
/// The Plex font the page needs first, worth preloading.
pub const SANS: &str = "/static/fonts/ibm-plex-sans-var.woff2";

const CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/tailwind.css"));
// Unmodified upstream release (sha256 2837d87a…fd24a).
const DATASTAR_JS: &str = include_str!("../vendor/datastar-1.0.2.js");
/// Lucide's icons as `<symbol>`s, inlined once per page by the layout.
pub const ICONS: &str = include_str!("../vendor/lucide/icons.svg");

// IBM Plex, unmodified (SIL OFL 1.1, vendor/fonts/OFL.txt). A changed font
// needs a new file name, since browsers keep these for a year.
const PLEX_SANS: &[u8] = include_bytes!("../vendor/fonts/ibm-plex-sans-var.woff2");
const PLEX_MONO_400: &[u8] = include_bytes!("../vendor/fonts/ibm-plex-mono-400.woff2");
const PLEX_MONO_500: &[u8] = include_bytes!("../vendor/fonts/ibm-plex-mono-500.woff2");
const FONT_CACHE: &str = "public, max-age=31536000, immutable";

#[route(GET "/static/app.css")]
async fn stylesheet() -> topcoat::Result<Response> {
    file(CSS.as_bytes(), "text/css; charset=utf-8", "no-cache")
}

#[route(GET "/static/datastar-1.0.2.js")]
async fn datastar() -> topcoat::Result<Response> {
    file(
        DATASTAR_JS.as_bytes(),
        "text/javascript; charset=utf-8",
        "no-cache",
    )
}

#[route(GET "/static/fonts/ibm-plex-sans-var.woff2")]
async fn plex_sans() -> topcoat::Result<Response> {
    file(PLEX_SANS, "font/woff2", FONT_CACHE)
}

#[route(GET "/static/fonts/ibm-plex-mono-400.woff2")]
async fn plex_mono_400() -> topcoat::Result<Response> {
    file(PLEX_MONO_400, "font/woff2", FONT_CACHE)
}

#[route(GET "/static/fonts/ibm-plex-mono-500.woff2")]
async fn plex_mono_500() -> topcoat::Result<Response> {
    file(PLEX_MONO_500, "font/woff2", FONT_CACHE)
}

fn file(body: &'static [u8], content_type: &str, cache: &str) -> topcoat::Result<Response> {
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::CACHE_CONTROL, cache)
        .body(Body::from(body))?)
}
