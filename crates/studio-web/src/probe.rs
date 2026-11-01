//! Reads rendered HTML the way the tests care about it: by tag, attribute and
//! visible text, so a test can say "the button that posts to /x says Allow"
//! without pinning classes or the order of attributes.
//!
//! Used by the crate's own tests and, through a `#[path]` include, by the
//! tests in `tests/`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// Tags that have no closing tag.
const VOID: [&str; 8] = ["br", "hr", "img", "input", "link", "meta", "use", "path"];

/// One opening tag in a page, with what follows it.
#[derive(Clone, Debug)]
pub struct Tag<'a> {
    pub name: &'a str,
    attrs: Vec<(&'a str, &'a str)>,
    /// The page from just after this opening tag on.
    after: &'a str,
}

impl<'a> Tag<'a> {
    /// An attribute's value, with its entities read.
    pub fn attr(&self, name: &str) -> Option<String> {
        self.attrs
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| unescape(v))
    }

    /// All its attributes as `name="value"` text, for looking through the
    /// scripts a tag carries.
    pub fn attributes(&self) -> String {
        self.attrs
            .iter()
            .map(|(n, v)| format!("{n}=\"{}\"", unescape(v)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether the attribute is exactly `value`.
    pub fn attr_is(&self, name: &str, value: &str) -> bool {
        self.attr(name).is_some_and(|v| v == value)
    }

    /// Whether the attribute contains `part`.
    pub fn attr_has(&self, name: &str, part: &str) -> bool {
        self.attr(name).is_some_and(|v| v.contains(part))
    }

    /// Whether the attribute is there, with any value.
    pub fn has(&self, name: &str) -> bool {
        self.attrs.iter().any(|(n, _)| *n == name)
    }

    /// The HTML between this tag and its closing tag; empty for a tag that
    /// has none.
    pub fn inner_html(&self) -> &'a str {
        if VOID.contains(&self.name) {
            return "";
        }
        let (open, close) = (format!("<{}", self.name), format!("</{}>", self.name));
        let mut depth = 1usize;
        let mut at = 0usize;
        while at < self.after.len() {
            let rest = &self.after[at..];
            let next_close = rest.find(&close);
            let next_open = rest.find(&open).filter(|&i| {
                // `<b` is not `<br`: the name must end there.
                matches!(
                    rest[i + open.len()..].chars().next(),
                    Some(' ' | '>' | '/' | '\n')
                )
            });
            match (next_open, next_close) {
                (Some(o), Some(c)) if o < c => {
                    depth += 1;
                    at += o + open.len();
                }
                (_, Some(c)) => {
                    depth -= 1;
                    if depth == 0 {
                        return &self.after[..at + c];
                    }
                    at += c + close.len();
                }
                _ => return "",
            }
        }
        ""
    }

    /// What a reader sees in it: its text with tags dropped, entities read
    /// and white space squeezed.
    pub fn text(&self) -> String {
        text_of(self.inner_html())
    }
}

/// The visible text of some HTML: tags dropped (each ends a word), entities
/// read and white space squeezed to single spaces.
pub fn text_of(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut quote = None;
    for c in html.chars() {
        match (in_tag, quote, c) {
            (false, _, '<') => {
                in_tag = true;
                out.push(' ');
            }
            (true, None, '"' | '\'') => quote = Some(c),
            (true, Some(q), c) if c == q => quote = None,
            (true, None, '>') => in_tag = false,
            (false, _, c) => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` with its HTML entities read.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Every opening tag in `html`, in order.
pub fn tags(html: &str) -> Vec<Tag<'_>> {
    let mut found = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' || !bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
            i += 1;
            continue;
        }
        let name_end = html[i + 1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .map_or(html.len(), |n| i + 1 + n);
        let name = &html[i + 1..name_end];
        // Attributes, to the first `>` outside a quoted value.
        let mut attrs = Vec::new();
        let mut at = name_end;
        loop {
            while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
                at += 1;
            }
            if at >= bytes.len() || bytes[at] == b'>' {
                break;
            }
            let key_start = at;
            while at < bytes.len() && !matches!(bytes[at], b'=' | b'>' | b' ' | b'\n' | b'/') {
                at += 1;
            }
            let key = &html[key_start..at];
            let mut value = "";
            if bytes.get(at) == Some(&b'=') {
                at += 1;
                match bytes.get(at) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let start = at + 1;
                        let end = html[start..]
                            .find(q as char)
                            .map_or(html.len(), |n| start + n);
                        value = &html[start..end];
                        at = (end + 1).min(html.len());
                    }
                    _ => {
                        let start = at;
                        while at < bytes.len()
                            && !bytes[at].is_ascii_whitespace()
                            && bytes[at] != b'>'
                        {
                            at += 1;
                        }
                        value = &html[start..at];
                    }
                }
            }
            attrs.push((key, value));
        }
        let after = &html[(at + 1).min(html.len())..];
        found.push(Tag { name, attrs, after });
        i = at + 1;
    }
    found
}

/// The tags named `name`.
pub fn named<'a>(html: &'a str, name: &str) -> Vec<Tag<'a>> {
    tags(html).into_iter().filter(|t| t.name == name).collect()
}

/// The tags whose attribute `attr` is exactly `value`.
pub fn with<'a>(html: &'a str, attr: &str, value: &str) -> Vec<Tag<'a>> {
    tags(html)
        .into_iter()
        .filter(|t| t.attr_is(attr, value))
        .collect()
}

/// The first tag whose attribute `attr` is exactly `value`; panics if none.
pub fn find<'a>(html: &'a str, attr: &str, value: &str) -> Tag<'a> {
    with(html, attr, value)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no tag with {attr}=\"{value}\" in {html}"))
}

/// The tag with this `id`; panics if none.
pub fn by_id<'a>(html: &'a str, id: &str) -> Tag<'a> {
    find(html, "id", id)
}

/// Whether some tag has this `id`.
pub fn has_id(html: &str, id: &str) -> bool {
    !with(html, "id", id).is_empty()
}

/// Whether some tag's text is exactly `text`.
pub fn shows(html: &str, text: &str) -> bool {
    tags(html).iter().any(|t| t.text() == text)
}

/// The tags `keep` accepts.
pub fn matching<'a>(html: &'a str, keep: impl Fn(&Tag<'a>) -> bool) -> Vec<Tag<'a>> {
    tags(html).into_iter().filter(|t| keep(t)).collect()
}

/// The first tag named `name` whose `attr` contains `part`; panics if none.
pub fn find_containing<'a>(html: &'a str, name: &str, attr: &str, part: &str) -> Tag<'a> {
    matching(html, |t| t.name == name && t.attr_has(attr, part))
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no <{name}> with {attr} containing {part} in {html}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<div id="a" data-x="1 > 0"><b>one</b> <br><p class='q' hidden>t &amp; <i>u</i></p><div>in</div></div><b>two</b>"#;

    #[test]
    fn finds_tags_by_attribute_whatever_the_order() {
        assert_eq!(find(PAGE, "data-x", "1 > 0").name, "div");
        assert_eq!(by_id(PAGE, "a").attr("data-x").as_deref(), Some("1 > 0"));
        assert!(find(PAGE, "class", "q").has("hidden"));
        assert!(!has_id(PAGE, "b"));
    }

    #[test]
    fn reads_the_text_inside_a_tag_with_its_nesting() {
        assert_eq!(by_id(PAGE, "a").text(), "one t & u in");
        assert_eq!(named(PAGE, "p")[0].text(), "t & u");
        assert_eq!(named(PAGE, "b").len(), 2);
        // `<b` does not mix up with `<br`.
        assert_eq!(named(PAGE, "b")[0].text(), "one");
        assert_eq!(
            named(PAGE, "div")[0].inner_html().matches("<div>").count(),
            1
        );
    }
}
