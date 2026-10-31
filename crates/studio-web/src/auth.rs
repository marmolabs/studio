use topcoat::{
    context::{Cx, app_context},
    cookie::{Cookie, Cookies, SameSite, cookies},
    router::{
        Body, Method, Next,
        error::see_other,
        request::{headers, method, uri},
        response::{IntoResponse, Response},
    },
};

use crate::AppConfig;

const COOKIE_NAME: &str = "studio_token";
const TOKEN_BYTES: usize = 32;

/// 32 random bytes, hex-encoded.
pub fn generate_token() -> std::io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}"));
    }
    out
}

/// Constant-time, so response timing doesn't leak the token.
pub fn tokens_equal(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Every request needs the token cookie, except the stylesheet and script.
/// A request carrying `?token=` gets the cookie and a redirect. POSTs from
/// another origin are refused.
#[topcoat::router::layer("/")]
pub async fn guard(cx: &Cx, body: Body, next: Next<'_>) -> topcoat::Result<Response> {
    let config: &AppConfig = app_context(cx);
    let request_uri = uri(cx);
    let path = request_uri.path();

    if path.starts_with(crate::statics::PREFIX) {
        return next.run(cx, body).await;
    }

    if *method(cx) == Method::POST
        && let Some(origin) = origin_header(cx)
        && !is_allowed_origin(&origin, &config.base_url)
    {
        return unauthorized_html("Requests from other sites are refused.");
    }

    let jar = cookies(cx);
    if jar
        .get(COOKIE_NAME)
        .is_some_and(|c| tokens_equal(c.value(), &config.token))
    {
        return next.run(cx, body).await;
    }
    let from_link = request_uri
        .query()
        .and_then(token_from_query)
        .is_some_and(|t| tokens_equal(&t, &config.token));
    if !from_link {
        return unauthorized_html("Open the link studio-web printed when it started.");
    }
    jar.add(
        Cookie::build((COOKIE_NAME, config.token.clone()))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Strict)
            .build(),
    );
    // Drop the token from the address bar and history.
    see_other(path).into_response(cx)
}

fn origin_header(cx: &Cx) -> Option<String> {
    headers(cx)
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(std::borrow::ToOwned::to_owned)
}

fn is_allowed_origin(origin: &str, base_url: &str) -> bool {
    let localhost = base_url.replace("127.0.0.1", "localhost");
    origin == base_url || origin == localhost
}

fn token_from_query(query: &str) -> Option<String> {
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        if k == "token" {
            return Some(v.into_owned());
        }
    }
    None
}

fn unauthorized_html(message: &str) -> topcoat::Result<Response> {
    let html = format!("<p>{}</p>", html_escape(message));
    let response = http::Response::builder()
        .status(http::StatusCode::UNAUTHORIZED)
        .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(html))?;
    Ok(response)
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compare_is_constant_time() {
        assert!(tokens_equal("abc", "abc"));
        assert!(!tokens_equal("abc", "abz"));
        assert!(!tokens_equal("abc", "abcd"));
    }

    #[test]
    fn allowed_origins_match_base_url_and_localhost() {
        let base = "http://127.0.0.1:4870";
        assert!(is_allowed_origin(base, base));
        assert!(is_allowed_origin("http://localhost:4870", base));
        assert!(!is_allowed_origin("http://127.0.0.1:4871", base));
        assert!(!is_allowed_origin("http://example.com", base));
    }

    #[test]
    fn token_extracted_from_query() {
        assert_eq!(token_from_query("token=abc"), Some("abc".to_owned()));
        assert_eq!(
            token_from_query("x=1&token=abc&y=2"),
            Some("abc".to_owned())
        );
        assert_eq!(token_from_query("x=1"), None);
    }

    #[test]
    fn html_escapes_special_characters() {
        assert_eq!(html_escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }
}
