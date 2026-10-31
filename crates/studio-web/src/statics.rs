//! The stylesheet and Datastar, compiled into the binary so `cargo install`
//! is enough: no asset bundle to ship next to it.

use topcoat::router::{Body, response::Response, route};

pub const PREFIX: &str = "/static/";
pub const STYLESHEET: &str = "/static/app.css";
pub const DATASTAR: &str = "/static/datastar-1.0.2.js";

const CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/tailwind.css"));
// Unmodified upstream release (sha256 2837d87a…fd24a).
const DATASTAR_JS: &str = include_str!("../vendor/datastar-1.0.2.js");

#[route(GET "/static/app.css")]
async fn stylesheet() -> topcoat::Result<Response> {
    file(CSS, "text/css; charset=utf-8")
}

#[route(GET "/static/datastar-1.0.2.js")]
async fn datastar() -> topcoat::Result<Response> {
    file(DATASTAR_JS, "text/javascript; charset=utf-8")
}

fn file(body: &'static str, content_type: &str) -> topcoat::Result<Response> {
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, content_type)
        .header(http::header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))?)
}
