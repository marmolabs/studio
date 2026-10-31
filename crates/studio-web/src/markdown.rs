//! Agent and reviewer text as Markdown, rendered on the server.

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, html};
use topcoat::view::Unescaped;

/// Tailwind classes that style the rendered Markdown. Put them on the wrapper.
pub const STYLE: &str = "space-y-2 [overflow-wrap:anywhere] \
    [&_h1]:text-[15px] [&_h1]:font-semibold [&_h2]:text-[15px] [&_h2]:font-semibold \
    [&_h3]:text-[15px] [&_h3]:font-medium [&_h4]:font-medium [&_strong]:font-semibold [&_strong]:text-fg \
    [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_li]:mt-0.5 \
    [&_blockquote]:border-l-2 [&_blockquote]:border-line-2 [&_blockquote]:pl-3 \
    [&_a]:text-fg [&_a]:underline [&_a]:decoration-line-3 [&_a]:underline-offset-3 \
    [&_code]:rounded [&_code]:bg-bg-3 [&_code]:px-[5px] [&_code]:py-px [&_code]:font-mono [&_code]:text-[12.5px] [&_code]:text-fg \
    [&_pre]:overflow-x-auto [&_pre]:rounded-lg [&_pre]:border [&_pre]:border-line [&_pre]:bg-bg-2 [&_pre]:px-3 [&_pre]:py-2 \
    [&_pre_code]:bg-transparent [&_pre_code]:p-0";

/// Renders Markdown to HTML. Raw HTML in the source is shown as text, images
/// become their alt text, links keep only http, https and relative URLs, and
/// a single newline stays a line break.
pub fn render(source: &str) -> Unescaped<String> {
    let mut dropped_link = false;
    let events =
        Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH).filter_map(|event| match event {
            Event::Html(text) | Event::InlineHtml(text) => Some(Event::Text(text)),
            Event::SoftBreak => Some(Event::HardBreak),
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
            Event::Start(Tag::Link { ref dest_url, .. }) if !safe_url(dest_url) => {
                dropped_link = true;
                None
            }
            Event::End(TagEnd::Link) if dropped_link => {
                dropped_link = false;
                None
            }
            event => Some(event),
        });
    let mut out = String::new();
    html::push_html(&mut out, events);
    Unescaped::new_unchecked(out)
}

/// Whether a link target is http, https or a relative path.
fn safe_url(url: &CowStr<'_>) -> bool {
    let url = url.trim().to_ascii_lowercase();
    if url.starts_with("//") {
        return false;
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return true;
    }
    let scheme_end = url.find(':');
    let path_end = url.find(['/', '?', '#']);
    match (scheme_end, path_end) {
        (None, _) => true,
        (Some(colon), Some(slash)) => slash < colon,
        (Some(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(source: &str) -> String {
        String::clone(&render(source))
    }

    #[test]
    fn renders_bold_code_and_lists() {
        assert!(html("**bold**").contains("<strong>bold</strong>"));
        assert!(html("use `cargo test`").contains("<code>cargo test</code>"));
        let list = html("- one\n- two");
        assert!(list.contains("<ul>") && list.contains("<li>one</li>"));
    }

    #[test]
    fn keeps_single_line_breaks() {
        assert!(html("fix a\nfix b").contains("fix a<br />\nfix b"));
    }

    #[test]
    fn renders_code_blocks() {
        let out = html("```rust\nlet x = 1 < 2;\n```");
        assert!(out.contains("<pre><code"), "{out}");
        assert!(out.contains("1 &lt; 2"), "{out}");
    }

    #[test]
    fn escapes_raw_html() {
        let out = html("<script>alert(1)</script>\n\nhi <img src=x onerror=alert(1)>");
        assert!(!out.contains("<script>") && !out.contains("<img"), "{out}");
        assert!(out.contains("&lt;script&gt;"), "{out}");
        assert!(out.contains("&lt;img src=x onerror=alert(1)&gt;"), "{out}");
    }

    #[test]
    fn markdown_images_become_text() {
        let out = html("![alt](http://x/y.png)");
        assert!(!out.contains("<img") && out.contains("alt"), "{out}");
    }

    #[test]
    fn keeps_only_safe_links() {
        assert!(html("[a](https://example.com)").contains(r#"href="https://example.com""#));
        assert!(html("[a](docs/x.md#top)").contains("<a href=\"docs/x.md#top\">a</a>"));
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,x",
            "//evil.test",
        ] {
            let out = html(&format!("[click](<{bad}>) after"));
            assert!(!out.contains("<a"), "{out}");
            assert!(out.contains("click"), "{out}");
        }
    }
}
